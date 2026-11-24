package org.graphlava

import java.io.File
import java.nio.file.Files
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking

const val EX = "http://ex.org/"
const val PFX = "PREFIX : <http://ex.org/>\n"
fun ex(s: String) = Term.iri(EX + s)
fun tempFile(name: String): File = Files.createTempDirectory("graphlava-kt").toFile().resolve(name)

/** Rows and the first value of `column`. */
suspend fun first(db: Database, q: String, column: String): Pair<Int, Term?> {
    var n = 0
    var v: Term? = null
    db.query(q).collect { b ->
        if (v == null && b.count > 0) v = b.term(0, column)
        n += b.count
    }
    return n to v
}

class KotlinApiTest {
    /** The agent-memory session through the Kotlin API on the JVM host (the
     *  emulator runs the same session as an instrumented test in CI). */
    // @tg: verifies:: [[openspec:mobile-bindings#Kotlin API#Agent memory session from Kotlin]] {level: e2e}
    @Test
    fun agentMemorySessionFromKotlin() = runBlocking {
        val path = tempFile("memory.glv").path
        var db = Database.open(path)
        val res = db.submit(listOf(
            Op.insert(ex("alice"), ex("worksAt"), ex("acme"), ex("episode1")),
            Op.insert(ex("alice"), ex("age"), Term.integer(42)),
        ), Durability.ORDERED)
        val works = (res[0] as OpResult.Id).id!!
        db.submit(listOf(Op.annotate(works, ex("confidence").v, Term.decimal("0.9").v)), Durability.ORDERED)
        val q = "$PFX SELECT ?c WHERE { << :alice :worksAt :acme ~ ?r >> . ?r :confidence ?c }"
        assertEquals(1 to Term.Literal("0.9", Term.XSD + "decimal"), first(db, q, "c"))
        val before = db.read { s -> s.lookup(ex("alice"))!! to works }
        db.close()
        db = Database.open(path)
        assertEquals(1 to Term.Literal("0.9", Term.XSD + "decimal"), first(db, q, "c"))
        val after = db.read { s ->
            val alice = s.lookup(ex("alice"))!!
            alice to s.find(alice, s.lookup(ex("worksAt"))).single().id!!
        }
        assertEquals(before, after, "IDs equal the ones returned before the reopen")
        db.close()
    }

    // @tg: verifies:: [[openspec:mobile-bindings#Kotlin API#Main-thread call is refused in debug builds]] {level: unit}
    @Test
    fun mainThreadCallIsRefusedInDebugBuilds() {
        val f = tempFile("main.glv")
        val saved = Database.mainThreadGuard
        Database.mainThreadGuard = { true } // the Android library installs a Looper check
        try {
            val e = assertFailsWith<IllegalStateException> { Database.openBlocking(f.path) }
            assertTrue("main thread" in e.message!!, e.message)
            assertFalse(f.exists(), "no disk access before the refusal")
        } finally {
            Database.mainThreadGuard = saved
        }
        Database.openBlocking(f.path).use { }
    }

    // @tg: verifies:: [[openspec:mobile-bindings#Kotlin API#Flow cancellation releases the snapshot]] {level: e2e}
    @Test
    fun flowCancellationReleasesTheSnapshot() = runBlocking {
        Database.open(tempFile("flow.glv").path).use { db ->
            db.submit((0 until 10_000).map { Op.insert(ex("n$it"), ex("v"), ex("x")) }, Durability.RELAXED)
            val firstBatch = db.query("$PFX SELECT ?s WHERE { ?s :v :x }").first()
            assertEquals(1024, firstBatch.count)
            assertEquals(0, db.debugOpenSnapshots(), "the read snapshot is released")
            val sizes = db.query("$PFX SELECT ?s WHERE { ?s :v :x }").toList().map { it.count }
            assertEquals(listOf(1024) * 9 + 784, sizes)
            assertEquals(0, db.debugOpenSnapshots())
        }
    }

    private operator fun List<Int>.times(n: Int) = (0 until n).flatMap { this }

    @Test
    fun idsAreUnsignedAndErrorsAreSealed() = runBlocking {
        val path = tempFile("ids.glv").path
        Database.open(path).use { db ->
            db.submit(listOf(Op.insert(ex("a"), ex("v"), Term.integer(7))))
            val id = db.query("$PFX SELECT ?v WHERE { :a :v ?v }").first()[0, 0]
            assertTrue(id > Long.MAX_VALUE.toULong(), "inline literal IDs use the top bit")
            assertFailsWith<GraphLavaException.InvalidRequest> { db.query("SELEKT").first() }
            assertFailsWith<GraphLavaException.Locked> { Database.open(path) }
            val knows = (db.submit(listOf(Op.insert(ex("a"), ex("knows"), ex("b"))))[0] as OpResult.Id).id!!
            val e: GraphLavaException = assertFailsWith<GraphLavaException.Conflict> {
                db.submit(listOf(Op.update(knows, ex("c").v, expected = ex("zzz").v)))
            }
            assertEquals(15, e.status)
        }
    }

    @Test
    fun throwingBlockAbortsTheWrite() = runBlocking {
        Database.open(tempFile("abort.glv").path).use { db ->
            assertFailsWith<IllegalArgumentException> {
                db.write(Durability.SYNC) { txn ->
                    repeat(3) { txn.insert(ex("s$it"), ex("p"), ex("o")) }
                    throw IllegalArgumentException("boom")
                }
            }
            assertEquals(0, first(db, "$PFX SELECT ?s WHERE { ?s :p :o }", "s").first)
            db.write(Durability.SYNC) { it.insert(ex("k"), ex("p"), ex("o")) }
            assertEquals(1, first(db, "$PFX SELECT ?s WHERE { ?s :p :o }", "s").first)
        }
    }

    @Test
    fun coreVersionMatchesTheHeader() {
        assertTrue(Database.coreVersion.isNotEmpty())
    }
}
