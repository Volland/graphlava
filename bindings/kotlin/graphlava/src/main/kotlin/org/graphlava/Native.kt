package org.graphlava

/**
 * The JNI surface over the C FFI (`graphlava.h`); see `src/main/cpp/graphlava_jni.c`.
 * Handles cross as `Long`, statuses as `Int`. Internal: use [Database].
 */
internal object Native {
    init {
        val explicit = System.getProperty("graphlava.library")
        if (explicit != null) System.load(explicit) else System.loadLibrary("graphlava")
        check(abiVersion() == headerAbiVersion()) { "graphlava: library and JNI shim ABI differ" }
    }

    @JvmStatic external fun lastStatus(): Int
    @JvmStatic external fun lastError(): String?
    @JvmStatic external fun version(): String
    @JvmStatic external fun abiVersion(): Int
    @JvmStatic external fun headerAbiVersion(): Int

    @JvmStatic external fun open(path: String, flags: Int, out: LongArray): Int
    @JvmStatic external fun close(db: Long): Int
    @JvmStatic external fun suspendDb(db: Long): Int
    @JvmStatic external fun resumeDb(db: Long): Int
    @JvmStatic external fun flush(db: Long): Int
    @JvmStatic external fun openSnapshots(db: Long, out: LongArray): Int

    @JvmStatic external fun sparqlUpdate(db: Long, text: String, durability: Int): Int
    @JvmStatic external fun requestNew(out: LongArray): Int
    @JvmStatic external fun requestNewText(lang: Int, text: String, out: LongArray): Int
    @JvmStatic external fun requestAdd(
        req: Long, op: Int, kinds: IntArray, ids: LongArray,
        values: Array<String?>, datatypes: Array<String?>, langs: Array<String?>,
    ): Int
    @JvmStatic external fun requestFree(req: Long)
    @JvmStatic external fun submit(db: Long, req: Long, durability: Int, out: LongArray): Int
    @JvmStatic external fun resultAll(res: Long): LongArray?
    @JvmStatic external fun resultDeleted(res: Long, i: Int): LongArray?
    @JvmStatic external fun resultFree(res: Long)

    @JvmStatic external fun readBegin(db: Long, out: LongArray): Int
    @JvmStatic external fun readEnd(read: Long)
    @JvmStatic external fun writeBegin(db: Long, durability: Int, out: LongArray): Int
    @JvmStatic external fun writeRun(w: Long, req: Long, out: LongArray): Int
    @JvmStatic external fun commit(w: Long): Int
    @JvmStatic external fun abort(w: Long): Int

    @JvmStatic external fun lookup(read: Long, kind: Int, value: String?, dt: String?, lang: String?, out: LongArray): Int
    @JvmStatic external fun graph(read: Long, iri: String, out: LongArray): Int
    @JvmStatic external fun decode(read: Long, id: Long, out: Array<String?>): Int
    @JvmStatic external fun find(read: Long, s: Long, p: Long, o: Long): LongArray?
    @JvmStatic external fun graphsOf(read: Long, item: Long): LongArray?
    @JvmStatic external fun scope(read: Long, graph: Long, direct: Boolean): LongArray?
    @JvmStatic external fun hedge(read: Long, e: Long): LongArray?

    @JvmStatic external fun query(db: Long, lang: Int, text: String, out: LongArray): Int
    @JvmStatic external fun rowsColumns(rows: Long): Array<String>?
    @JvmStatic external fun rowsNextBatch(rows: Long): LongArray?
    @JvmStatic external fun rowsDecode(rows: Long, id: Long, out: Array<String?>): Int
    @JvmStatic external fun rowsFree(rows: Long)

    /** Throws the error of a failing call; must run on the calling thread. */
    fun check(status: Int) {
        if (status != 0) throw GraphLavaException.of(status, lastError() ?: "")
    }

    fun <T> checkNotNull(v: T?): T = v ?: throw GraphLavaException.of(lastStatus(), lastError() ?: "")
}
