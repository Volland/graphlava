package org.graphlava.android

import android.os.Handler
import android.os.Looper
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.util.concurrent.CountDownLatch
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlinx.coroutines.runBlocking
import org.graphlava.Database
import org.graphlava.Durability
import org.graphlava.Op
import org.graphlava.OpResult
import org.graphlava.Term
import org.graphlava.v
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Instrumented tests for an arm64-v8a emulator and a 16 KB-page Android 15
 * image (CI, .github/workflows/mobile.yml). They need a running device.
 */
@RunWith(AndroidJUnit4::class)
class InstrumentedTest {
    private val ctx = InstrumentationRegistry.getInstrumentation().targetContext
    private fun ex(s: String) = Term.iri("http://ex.org/$s")

    // @tg: verifies:: [[openspec:mobile-bindings#Kotlin API#Agent memory session from Kotlin]] {level: e2e}
    // @tg: verifies:: [[openspec:mobile-bindings#Mobile native packaging#16 KB page alignment]] {level: e2e}
    @Test
    fun agentMemorySessionOnTheDevice() = runBlocking {
        val path = ctx.filesDir.resolve("memory-${System.nanoTime()}.glv").path
        var db = Database.open(path)
        val works = (db.submit(listOf(Op.insert(ex("alice"), ex("worksAt"), ex("acme"))), Durability.ORDERED)[0]
            as OpResult.Id).id!!
        db.submit(listOf(Op.annotate(works, ex("confidence").v, Term.decimal("0.9").v)), Durability.ORDERED)
        db.close()
        db = Database.open(path)
        val q = "PREFIX : <http://ex.org/> SELECT ?r ?c WHERE { << :alice :worksAt :acme ~ ?r >> . ?r :confidence ?c }"
        // Batches decode only while the flow is collected.
        var seen = 0
        db.query(q).collect { b ->
            assertEquals("0.9", (b.term(0, "c") as Term.Literal).lexical)
            assertEquals(works, b[0, 0])
            seen += b.count
        }
        assertEquals(1, seen)
        db.close()
    }

    // @tg: verifies:: [[openspec:mobile-bindings#Kotlin API#Main-thread call is refused in debug builds]] {level: e2e}
    @Test
    fun mainThreadCallIsRefused() {
        GraphLavaLifecycle.install(ctx, Database.openBlocking(ctx.filesDir.resolve("guard.glv").path))
        val done = CountDownLatch(1)
        var error: Throwable? = null
        Handler(Looper.getMainLooper()).post {
            error = runCatching { Database.openBlocking(ctx.filesDir.resolve("main.glv").path) }.exceptionOrNull()
            done.countDown()
        }
        done.await()
        assertTrue(error is IllegalStateException && "main thread" in error!!.message!!, "$error")
    }
}
