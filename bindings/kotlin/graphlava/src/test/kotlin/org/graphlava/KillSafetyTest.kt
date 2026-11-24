package org.graphlava

import java.io.File
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlinx.coroutines.runBlocking

/**
 * Kill safety on the JVM host: a child JVM writes, runs the `ON_STOP` flush
 * the Android library performs, and is killed with SIGKILL (`kill -9`). The
 * emulator version (`am kill` then `kill -9` after backgrounding) runs as an
 * instrumented test in CI.
 */
class KillSafetyTest {
    private fun child(vararg args: String): Process {
        val java = File(System.getProperty("java.home"), "bin/java").path
        return ProcessBuilder(
            java, "-cp", System.getProperty("java.class.path"),
            "-Dgraphlava.library=" + System.getProperty("graphlava.library"),
            KillChild::class.java.name, *args,
        ).redirectErrorStream(true).start()
    }

    private fun runUntilReady(p: Process): String {
        val line = p.inputStream.bufferedReader().readLine() ?: ""
        p.destroyForcibly() // SIGKILL
        p.waitFor(10, TimeUnit.SECONDS)
        return line
    }

    // @tg: verifies:: [[openspec:mobile-bindings#Android background and kill safety#Kill after background]] {level: crash}
    // @tg: verifies:: [[21-e2e-tests#Android kill after background]] {level: crash}
    @Test
    fun killAfterBackground() = runBlocking {
        val path = tempFile("kill.glv").path
        assertEquals("ready", runUntilReady(child(path, "ordered", "100")))
        Database.open(path).use { db ->
            val (n, _) = first(db, "$PFX SELECT ?s WHERE { ?s :n ?v }", "s")
            assertEquals(100, n, "all acknowledged commits are present")
        }
    }

    // @tg: verifies:: [[openspec:mobile-bindings#Android background and kill safety#Relaxed commit may be lost]] {level: crash}
    @Test
    fun relaxedCommitMayBeLost() = runBlocking {
        val path = tempFile("relaxed.glv").path
        assertEquals("ready", runUntilReady(child(path, "relaxed", "1")))
        Database.open(path).use { db ->
            val (n, _) = first(db, "$PFX SELECT ?s WHERE { ?s :n ?v . ?s :m ?w }", "s")
            val (k, _) = first(db, "$PFX SELECT ?s WHERE { ?s ?p ?o }", "s")
            assertTrue(n == k, "the transaction is fully present or fully absent ($n of $k)")
            assertTrue(k == 0 || k == 2)
        }
    }
}

/** The child process of [KillSafetyTest]. */
object KillChild {
    @JvmStatic
    fun main(args: Array<String>) = runBlocking {
        val db = Database.open(args[0])
        when (args[1]) {
            "ordered" -> {
                repeat(args[2].toInt()) { db.update("$PFX INSERT DATA { :s$it :n $it }", Durability.ORDERED) }
                db.flush() // ON_STOP
            }
            else -> {
                // A relaxed commit acknowledged, killed before ON_STOP.
                db.submit(listOf(Op.insert(ex("r"), ex("n"), Term.integer(1)),
                    Op.insert(ex("r"), ex("m"), Term.integer(2))), Durability.RELAXED)
            }
        }
        println("ready")
        System.out.flush()
        Thread.sleep(60_000)
    }
}
