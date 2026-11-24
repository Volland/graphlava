package org.graphlava

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlinx.coroutines.runBlocking

/** Every snippet of bindings/kotlin/README.md, run on the JVM host. */
class ReadmeTest {
    @Test
    fun readmeSnippets() = runBlocking {
        val path = tempFile("memory.glv").path
        fun iri(local: String) = Term.iri("http://example.org/$local")

        // Quick start
        val db = Database.open(path)
        val res = db.submit(listOf(Op.insert(iri("alice"), iri("worksAt"), iri("acme"))), Durability.ORDERED)
        val works = (res[0] as OpResult.Id).id!!
        db.submit(listOf(Op.annotate(works, iri("confidence").v, Term.decimal("0.9").v)))
        db.submit(listOf(Op.update(works, iri("globex").v)))   // same ID, annotation kept

        // SPARQL
        db.update("PREFIX : <http://example.org/> INSERT DATA { :a :knows :b . :b :knows :c }")
        var rows = 0
        db.query("""
            PREFIX : <http://example.org/>
            SELECT ?org ?c WHERE { << :alice :worksAt ?org ~ ?r >> . ?r :confidence ?c }
        """).collect { batch ->
            for (i in 0 until batch.count) {
                assertEquals(iri("globex"), batch.term(i, "org"))
                rows++
            }
        }
        assertEquals(1, rows)

        // Datalog
        var reached = 0
        db.query("""
            @prefix : <http://example.org/> .
            reach(?x, ?y) :- :knows(?x, ?y) .
            reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
            ?- reach(:a, ?who) .
        """, datalog = true).collect { reached += it.count }
        assertEquals(2, reached)

        // Interactive write and snapshot reads
        db.write(Durability.SYNC) { txn -> txn.insert(iri("bob"), iri("knows"), iri("alice")) }
        val n = db.read { snap ->
            val bob = snap.lookup(iri("bob")) ?: return@read 0
            snap.find(s = bob).size
        }
        assertEquals(1, n)
        db.close()
    }
}
