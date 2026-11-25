package org.graphlava

import java.util.concurrent.Executors
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.withContext

/**
 * At most 1024 rows of IDs in column-major order (rule `api-ffi-rows`): row `r`
 * of column `c` is `ids[c * count + r]`, 0 is unbound. Decode while the flow
 * is being collected.
 */
class RowBatch internal constructor(
    private val rows: Long,
    val columns: List<String>,
    val count: Int,
    val ids: ULongArray,
) {
    operator fun get(row: Int, column: Int): ULong = ids[column * count + row]

    fun term(id: ULong): Term {
        if (id == 0uL) return Term.Unbound
        val out = arrayOfNulls<String>(4)
        Native.check(Native.rowsDecode(rows, id.toLong(), out))
        return Term.of(out)
    }

    fun term(row: Int, column: String): Term = term(this[row, columns.indexOf(column).also {
        require(it >= 0) { "no column $column" }
    }])
}

/**
 * The owner handle of one database file, over the C FFI
 * ([[16-api#Mobile bindings]]). Every call runs off the main thread: writes on
 * one dedicated single-thread dispatcher, reads on a pool sized to the core
 * count (rule `api-kotlin-threads`). Results are `Flow<RowBatch>`.
 */
// @tg: implements:: [[openspec:mobile-bindings#Kotlin API]]
class Database private constructor(private val handle: Long, val path: String) : AutoCloseable {
    @Volatile private var closed = false

    companion object {
        /** The dispatcher every write and lifecycle call runs on. */
        val writer: CoroutineDispatcher =
            Executors.newSingleThreadExecutor { r -> Thread(r, "graphlava-writer").apply { isDaemon = true } }
                .asCoroutineDispatcher()

        /** The bounded pool reads run on. */
        @OptIn(ExperimentalCoroutinesApi::class)
        val readers: CoroutineDispatcher =
            Dispatchers.IO.limitedParallelism(Runtime.getRuntime().availableProcessors().coerceAtLeast(2))

        /**
         * Returns true on the main thread when blocking calls must be refused
         * (debug builds). The Android library installs a `Looper` check;
         * plain JVM hosts have no main thread to protect.
         */
        @Volatile var mainThreadGuard: () -> Boolean = { false }

        /** The core version of the native library. */
        val coreVersion: String get() = Native.version()

        internal fun refuseMainThread(what: String) {
            check(!mainThreadGuard()) {
                "graphlava: $what called on the main thread; call it from a coroutine (the main thread must not do disk I/O)"
            }
        }

        /** Opens (or creates) the file on the writer dispatcher. */
        suspend fun open(path: String, appGroup: Boolean = false): Database =
            withContext(writer) { openNow(path, appGroup) }

        /**
         * Blocking open for code that cannot suspend. Throws
         * `IllegalStateException` on the main thread in debug builds, before
         * touching the disk.
         */
        fun openBlocking(path: String): Database {
            refuseMainThread("Database.openBlocking")
            return openNow(path, false)
        }

        private fun openNow(path: String, appGroup: Boolean): Database {
            val out = LongArray(1)
            Native.check(Native.open(path, if (appGroup) 1 else 0, out))
            return Database(out[0], path)
        }
    }

    /** Runs operations as one write request: all or none. */
    suspend fun submit(ops: List<Op>, durability: Durability = Durability.ORDERED): List<OpResult> =
        withContext(writer) {
            val req = makeRequest(ops)
            try {
                val out = LongArray(1)
                Native.check(Native.submit(handle, req, durability.raw, out))
                takeResults(out[0])
            } finally {
                Native.requestFree(req)
            }
        }

    suspend fun update(sparql: String, durability: Durability = Durability.ORDERED) = withContext(writer) {
        Native.check(Native.sparqlUpdate(handle, sparql, durability.raw))
    }

    /**
     * An interactive write transaction scoped to [block]: committed when it
     * returns, aborted when it throws or the coroutine is cancelled. It holds
     * the writer for at most the engine's time limit (100 ms).
     */
    suspend fun <R> write(durability: Durability = Durability.ORDERED, block: (WriteTransaction) -> R): R =
        withContext(writer) {
            val out = LongArray(1)
            Native.check(Native.writeBegin(handle, durability.raw, out))
            val txn = WriteTransaction(out[0])
            val r = try {
                block(txn)
            } catch (e: Throwable) {
                txn.live = false
                Native.abort(out[0])
                throw e
            }
            txn.live = false
            Native.check(Native.commit(out[0]))
            r
        }

    /** Core reads on one snapshot scoped to [block]. */
    suspend fun <R> read(block: (ReadSnapshot) -> R): R = withContext(readers) {
        val out = LongArray(1)
        Native.check(Native.readBegin(handle, out))
        try { block(ReadSnapshot(out[0])) } finally { Native.readEnd(out[0]) }
    }

    /**
     * A SPARQL query (or Datalog with goals) as a cold `Flow` of batches of at
     * most 1024 rows. Its snapshot is released when collection completes,
     * fails or is cancelled.
     */
    fun query(text: String, datalog: Boolean = false): Flow<RowBatch> = flow {
        // Native calls run on the read pool; `emit` runs in the collector, so
        // the snapshot stays open while the collector decodes a batch.
        val rows = withContext(readers) {
            val out = LongArray(1)
            Native.check(Native.query(handle, if (datalog) 1 else 0, text, out))
            out[0]
        }
        try {
            val columns = withContext(readers) { Native.checkNotNull(Native.rowsColumns(rows)).toList() }
            while (true) {
                val b = withContext(readers) { Native.checkNotNull(Native.rowsNextBatch(rows)) }
                val n = b[0].toInt()
                if (n == 0) break
                emit(RowBatch(rows, columns, n, b.copyOfRange(2, b.size).toULongArray()))
            }
        } finally {
            Native.rowsFree(rows)
        }
    }

    /** Commits the open batch durably now (Android `ON_STOP`). */
    suspend fun flush() = withContext(writer) { Native.check(Native.flush(handle)) }

    /** Blocking flush for lifecycle callbacks; never on the main thread in debug builds. */
    fun flushBlocking() {
        refuseMainThread("Database.flushBlocking")
        Native.check(Native.flush(handle))
    }

    /** Suspends the store (calls wait until [resumeStore]); not needed on Android. */
    fun suspendStore() = Native.check(Native.suspendDb(handle))

    fun resumeStore() = Native.check(Native.resumeDb(handle))

    /** Number of open read snapshots (diagnostics). */
    fun debugOpenSnapshots(): Int {
        val out = LongArray(1)
        Native.check(Native.openSnapshots(handle, out))
        return out[0].toInt()
    }

    /** Commits any open batch and closes the file. */
    override fun close() {
        if (closed) return
        Native.check(Native.close(handle))
        closed = true
    }
}

/** An interactive write transaction; valid only inside [Database.write]. */
class WriteTransaction internal constructor(private val handle: Long) {
    internal var live = true

    /** Runs operations in order; a failing operation undoes only itself. */
    fun run(ops: List<Op>): List<OpResult> {
        if (!live) throw GraphLavaException.Closed("the write transaction has ended")
        val req = makeRequest(ops)
        try {
            val out = LongArray(1)
            Native.check(Native.writeRun(handle, req, out))
            return takeResults(out[0])
        } finally {
            Native.requestFree(req)
        }
    }

    fun insert(s: Term, p: Term, o: Term, graph: Term? = null): ULong? =
        (run(listOf(Op.insert(s, p, o, graph))).first() as? OpResult.Id)?.id
}

/** Core reads on one snapshot; valid only inside [Database.read]. */
class ReadSnapshot internal constructor(private val handle: Long) {
    fun lookup(t: Term): ULong? {
        val out = LongArray(1)
        val st = when (t) {
            is Term.Iri -> Native.lookup(handle, 1, t.iri, null, null, out)
            is Term.Blank -> Native.lookup(handle, 2, t.label, null, null, out)
            is Term.Literal -> Native.lookup(handle, 3, t.lexical, t.datatype, t.lang, out)
            else -> throw GraphLavaException.InvalidArgument("$t cannot be looked up")
        }
        Native.check(st)
        return out[0].toULong().takeIf { it != 0uL }
    }

    fun decode(id: ULong): Term {
        val out = arrayOfNulls<String>(4)
        Native.check(Native.decode(handle, id.toLong(), out))
        return Term.of(out)
    }

    fun find(s: ULong? = null, p: ULong? = null, o: ULong? = null): List<Quad> {
        val v = Native.checkNotNull(Native.find(handle, (s ?: 0uL).toLong(), (p ?: 0uL).toLong(), (o ?: 0uL).toLong()))
        return (0 until v.size / 4).map {
            Quad(v[4 * it].toULong(), v[4 * it + 1].toULong(), v[4 * it + 2].toULong(),
                v[4 * it + 3].toULong().takeIf { x -> x != 0uL })
        }
    }

    fun graph(iri: String): ULong? {
        val out = LongArray(1)
        Native.check(Native.graph(handle, iri, out))
        return out[0].toULong().takeIf { it != 0uL }
    }

    fun graphsOf(item: ULong): List<ULong> =
        Native.checkNotNull(Native.graphsOf(handle, item.toLong())).map { it.toULong() }

    fun scope(graph: ULong, direct: Boolean): List<ULong> =
        Native.checkNotNull(Native.scope(handle, graph.toLong(), direct)).map { it.toULong() }

    data class Hedge(val type: ULong, val graph: ULong, val roles: List<Pair<ULong, ULong>>)

    fun hedge(id: ULong): Hedge? {
        val v = Native.hedge(handle, id.toLong())
        if (v == null) {
            if (Native.lastStatus() == 16) return null
            Native.check(Native.lastStatus())
            return null
        }
        return Hedge(v[0].toULong(), v[1].toULong(),
            (0 until (v.size - 2) / 2).map { v[2 + 2 * it].toULong() to v[3 + 2 * it].toULong() })
    }
}
