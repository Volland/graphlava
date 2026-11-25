package org.graphlava

import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import kotlin.test.fail
import kotlinx.coroutines.runBlocking
import org.json.JSONArray
import org.json.JSONObject
import org.junit.jupiter.api.Assumptions.assumeTrue

/**
 * L1 refinement through the Kotlin package on the JVM host: replays the Lean
 * oracle's traces (`lake exe oracle <seed> <n>`) with the abstraction function
 * of `crates/graphlava/tests/refinement.rs`. Skipped without `lake` unless
 * GRAPHLAVA_REQUIRE_REFINEMENT=1; GRAPHLAVA_REFINEMENT_SEEDS / _OPS override 8 × 2,000.
 */
class OracleReplayTest {
    private fun oracle(seed: Int, n: Int): List<JSONObject>? = try {
        val p = ProcessBuilder("lake", "exe", "oracle", "$seed", "$n")
            .directory(File(System.getProperty("graphlava.repo"), "formal")).start()
        val out = p.inputStream.bufferedReader().readText()
        if (p.waitFor() != 0) null else out.lines().filter { it.isNotBlank() }.map(::JSONObject)
    } catch (e: java.io.IOException) {
        null
    }

    // @tg: verifies:: [[openspec:mobile-bindings#Kotlin API]] {level: refinement}
    @Test
    fun oracleTraceReplayThroughKotlin() = runBlocking {
        val seeds = System.getenv("GRAPHLAVA_REFINEMENT_SEEDS")?.toInt() ?: 8
        val ops = System.getenv("GRAPHLAVA_REFINEMENT_OPS")?.toInt() ?: 2000
        if (oracle(1, 1) == null) {
            check(System.getenv("GRAPHLAVA_REQUIRE_REFINEMENT") == null) { "lake not found" }
            assumeTrue(false, "`lake` is not available")
        }
        for (seed in 1..seeds) {
            val trace = oracle(seed, ops)!!
            Database.open(tempFile("replay.glv").path).use { db ->
                val r = Replay(db, seed)
                r.schema(trace[0].getJSONObject("schema"))
                trace.subList(1, trace.size - 1).forEachIndexed { i, line -> r.step(line, i) }
                r.checkFinal(trace.last())
            }
        }
    }
}

private const val NS = "http://oracle/"
private val DEFAULT_GRAPH: ULong = (5uL shl 60) or 1uL

private class Replay(val db: Database, val seed: Int) {
    val toRust = HashMap<Long, ULong>()
    val toModel = HashMap<ULong, Long>()
    val hedges = HashSet<ULong>()

    fun graph(n: Long): Value = if (n == 0L) Value.Id(DEFAULT_GRAPH) else Term.iri("${NS}g$n").v

    fun term(v: JSONObject): Value = when {
        v.has("node") -> Term.iri("${NS}n${v.getLong("node")}").v
        v.has("lit") -> Term.integer(v.getLong("lit")).v
        else -> Value.Id(toRust[v.getLong("stmt")]!!)
    }

    fun rustId(m: Long): ULong =
        toRust[m] ?: ((4uL shl 60) or (((1uL shl 40) - 1uL) shl 20) or (m.toULong() % (1uL shl 20)))

    fun bind(expect: Long, id: ULong, ctx: String) {
        val r = toRust[expect]
        if (r != null) assertEquals(r, id, ctx) else {
            assertTrue(id !in toModel, "$ctx: an existing ID for a fresh model ID")
            toRust[expect] = id
            toModel[id] = expect
        }
    }

    suspend fun submit(ops: List<Op>): Result<List<OpResult>> =
        try { Result.success(db.submit(ops, Durability.RELAXED)) } catch (e: GraphLavaException) { Result.failure(e) }

    suspend fun schema(s: JSONObject) {
        val multi = s.getJSONArray("multi")
        for (i in 0 until multi.length()) {
            db.submit(listOf(Op.declarePredicate(Term.iri("${NS}p${multi.getLong(i)}").v, multi = true)), Durability.RELAXED)
        }
        val ht = s.getJSONArray("htypes")
        for (i in 0 until ht.length()) {
            val t = ht.getJSONObject(i)
            val req = t.getJSONArray("required").let { a -> (0 until a.length()).map { a.getLong(it) }.toSet() }
            val roles = t.getJSONArray("roles").let { a ->
                (0 until a.length()).map { a.getLong(it) }.map { Op.Companion.Role(Term.iri("${NS}r$it").v, it in req) }
            }
            db.submit(listOf(Op.declareHedgeType(Term.iri("${NS}T${t.getLong("id")}").v, roles)), Durability.RELAXED)
        }
    }

    fun removed(ids: List<ULong>) = ids.map { toModel[it]!! }.toSet()
    fun want(e: JSONObject) = e.getJSONArray("removed").let { a -> (0 until a.length()).map { a.getLong(it) }.toSet() }
    fun JSONArray.longs() = (0 until length()).map { getLong(it) }

    suspend fun step(line: JSONObject, i: Int) {
        val ctx = "seed $seed op $i: $line"
        val e = line.optJSONObject("expect") ?: JSONObject()
        val ok = if (e.has("ok")) e.getBoolean("ok") else null
        val blockedOk = e.optBoolean("blocked", false)
        when (line.getString("op")) {
            "insert" -> {
                val t = line.getJSONObject("triple")
                val (s, o) = term(t.getJSONObject("s")) to term(t.getJSONObject("o"))
                val p = Term.iri("${NS}p${t.getLong("p")}").v
                var res = db.submit(listOf(Op.insert(s, p, o, graph(line.getLong("graph")))), Durability.RELAXED)
                if (res[0] == OpResult.Id(null)) res = db.submit(listOf(Op.statementId(s, p, o)), Durability.RELAXED)
                bind(e.getLong("id"), (res[0] as OpResult.Id).id!!, ctx)
            }
            "hedge" -> {
                val roles = line.getJSONArray("roles").let { a ->
                    (0 until a.length()).map { a.getJSONArray(it) }
                        .map { Term.iri("${NS}r${it.getLong(0)}").v to term(it.getJSONObject(1)) }
                }
                val res = submit(listOf(Op.createHedge(Term.iri("${NS}T${line.getLong("type")}").v, graph(line.getLong("graph")), roles)))
                if (e.has("id")) {
                    val id = (res.getOrThrow()[0] as OpResult.Id).id!!
                    assertEquals(6uL, id shr 60, ctx)
                    hedges += id
                    bind(e.getLong("id"), id, ctx)
                } else {
                    assertTrue(res.exceptionOrNull() is GraphLavaException.HyperedgeShape, "$ctx: $res")
                }
            }
            "add_member", "remove_member" -> {
                val item = rustId(line.getLong("id"))
                if (item in hedges) { assertEquals(false, ok, ctx); return }
                val add = line.getString("op") == "add_member"
                val g = graph(line.getLong("graph"))
                val res = submit(listOf(if (add) Op.addMember(item, g) else Op.removeMember(item, g)))
                val x = res.exceptionOrNull()
                when {
                    res.isSuccess && ok == true -> if (!add) assertEquals(want(e), removed((res.getOrThrow()[0] as OpResult.Deleted).ids), ctx)
                    ok == false && x is GraphLavaException.NotFound && !e.has("blocked") -> {}
                    ok == false && !add && x is GraphLavaException.InvalidRequest && !e.has("blocked") -> {}
                    ok == false && !add && x is GraphLavaException.Referenced && blockedOk -> {}
                    else -> fail("$ctx: $res")
                }
            }
            "nest", "unnest" -> {
                val nest = line.getString("op") == "nest"
                val (c, p) = graph(line.getLong("child")) to graph(line.getLong("parent"))
                val res = submit(listOf(if (nest) Op.nest(c, p) else Op.unnest(c, p)))
                val x = res.exceptionOrNull()
                val got = when {
                    res.isSuccess -> nest || res.getOrThrow()[0] == OpResult.Changed(true)
                    nest && x is GraphLavaException.GraphConstraint -> false
                    !nest && x is GraphLavaException.InvalidRequest -> false
                    else -> fail("$ctx: $res")
                }
                assertEquals(ok, got, ctx)
            }
            "scope" -> {
                val n = line.getLong("graph")
                val got = db.read { s ->
                    val g = if (n == 0L) DEFAULT_GRAPH else s.graph("${NS}g$n")
                    g?.let { s.scope(it, line.getBoolean("direct")).map { id -> toModel[id]!! }.sorted() } ?: emptyList()
                }
                assertEquals(e.getJSONArray("ids").longs(), got, ctx)
            }
            "update" -> {
                val res = submit(listOf(Op.update(rustId(line.getLong("id")), term(line.getJSONObject("o")))))
                val x = res.exceptionOrNull()
                assertTrue((res.isSuccess && ok == true) ||
                    (ok == false && (x is GraphLavaException.Conflict || x is GraphLavaException.NotFound)), "$ctx: $res")
            }
            "delete" -> {
                val mode = if (line.getBoolean("detach")) DeleteMode.DETACH else DeleteMode.CASCADE
                val res = submit(listOf(Op.delete(rustId(line.getLong("id")), mode)))
                val x = res.exceptionOrNull()
                when {
                    res.isSuccess && ok == true -> assertEquals(want(e), removed((res.getOrThrow()[0] as OpResult.Deleted).ids), ctx)
                    ok == false && x is GraphLavaException.Referenced && blockedOk -> {}
                    ok == false && x is GraphLavaException.NotFound && !e.has("blocked") -> {}
                    else -> fail("$ctx: $res")
                }
            }
            else -> fail("unknown op $line")
        }
    }

    suspend fun checkFinal(line: JSONObject) = db.read { s ->
        fun json(id: ULong): String {
            toModel[id]?.let { if (id shr 60 == 4uL) return "s$it" }
            return when (val t = s.decode(id)) {
                is Term.Iri -> "n" + t.iri.removePrefix("${NS}n")
                is Term.Literal -> "l" + t.lexical
                else -> "?$t"
            }
        }
        fun model(v: JSONObject) = when {
            v.has("node") -> "n${v.getLong("node")}"
            v.has("lit") -> "l${v.getLong("lit")}"
            else -> "s${v.getLong("stmt")}"
        }
        val got = s.find().mapNotNull { q ->
            val p = (s.decode(q.p) as? Term.Iri)?.iri ?: return@mapNotNull null
            if (!p.startsWith("${NS}p")) return@mapNotNull null
            "${toModel[assertNotNull(q.id)]} ${json(q.s)} ${p.removePrefix("${NS}p")} ${json(q.o)}"
        }.sorted()
        val fin = line.getJSONArray("final")
        val want = (0 until fin.length()).map { fin.getJSONArray(it) }.map {
            val t = it.getJSONObject(1)
            "${it.getLong(0)} ${model(t.getJSONObject("s"))} ${t.getLong("p")} ${model(t.getJSONObject("o"))}"
        }.sorted()
        assertEquals(want, got, "seed $seed: final state")
        fun graphNo(g: ULong): Long =
            if (g == DEFAULT_GRAPH) 0 else (s.decode(g) as Term.Iri).iri.removePrefix("${NS}g").toLong()
        val members = line.getJSONArray("members")
        for (k in 0 until members.length()) {
            val m = members.getJSONArray(k)
            assertEquals(m.getJSONArray(1).longs(), s.graphsOf(toRust[m.getLong(0)]!!).map(::graphNo).sorted(), "seed $seed: memberships")
        }
        val hs = line.getJSONArray("hedges")
        for (k in 0 until hs.length()) {
            val h = hs.getJSONArray(k)
            val e = assertNotNull(s.hedge(toRust[h.getLong(0)]!!))
            val spec = h.getJSONObject(1)
            assertEquals(Term.iri("${NS}T${spec.getLong("type")}"), s.decode(e.type))
            val gotRoles = e.roles.map { (r, n) -> "${(s.decode(r) as Term.Iri).iri} ${json(n)}" }.sorted()
            val rs = spec.getJSONArray("roles")
            val wantRoles = (0 until rs.length()).map { rs.getJSONArray(it) }
                .map { "${NS}r${it.getLong(0)} ${model(it.getJSONObject(1))}" }.sorted()
            assertEquals(wantRoles, gotRoles, "seed $seed: roles")
        }
        assertEquals(hs.length(), hedges.count { s.hedge(it) != null }, "seed $seed: live hyperedges")
        assertEquals(line.getLong("next"), toRust.size + 1L, "seed $seed: allocator")
    }
}
