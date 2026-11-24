package org.graphlava

/** Durability level of a write ([[06-transactions-durability#Durability levels]]). */
enum class Durability(internal val raw: Int) {
    /** Durable when the call returns; closes the group-commit batch at once. */
    SYNC(0),
    /** Durable when the call returns; shares a group commit. */
    ORDERED(1),
    /** Atomic and crash-consistent; may be lost until a later durable commit. */
    RELAXED(2),
}

/** An RDF term. */
sealed class Term {
    data class Iri(val iri: String) : Term()
    data class Blank(val label: String) : Term()
    /** Datatype null: `xsd:string`, or `rdf:langString` with a tag. */
    data class Literal(val lexical: String, val datatype: String? = null, val lang: String? = null) : Term()
    /** An RDF 1.2 triple term in N-Triples form (output only). */
    data class Triple(val ntriples: String) : Term()
    object Unbound : Term()

    companion object {
        const val XSD = "http://www.w3.org/2001/XMLSchema#"
        fun iri(s: String) = Iri(s)
        fun integer(v: Long) = Literal(v.toString(), XSD + "integer")
        fun decimal(v: String) = Literal(v, XSD + "decimal")

        internal fun of(a: Array<String?>): Term = when (a[0]) {
            "iri" -> Iri(a[1]!!)
            "blank" -> Blank(a[1]!!)
            "literal" -> Literal(a[1]!!, a[2], a[3])
            "triple" -> Triple(a[1]!!)
            else -> Unbound
        }
    }
}

/** An argument of a write operation. IDs are the full unsigned 64-bit value. */
sealed class Value {
    data class T(val term: Term) : Value()
    data class Id(val id: ULong) : Value()
    data class UInt(val v: ULong) : Value()
    object None : Value()
}

val Term.v: Value get() = Value.T(this)

enum class DeleteMode(internal val raw: ULong) { CASCADE(0u), KEEP_ANNOTATIONS(1u), DETACH(2u) }

/** One core operation of a write request ([[16-api#Core API]]). */
data class Op internal constructor(val code: Int, val args: List<Value>) {
    companion object {
        fun insert(s: Value, p: Value, o: Value, graph: Value = Value.None) = Op(1, listOf(s, p, o, graph))
        fun insert(s: Term, p: Term, o: Term, graph: Term? = null) =
            insert(s.v, p.v, o.v, graph?.v ?: Value.None)
        fun set(s: Value, p: Value, o: Value) = Op(2, listOf(s, p, o))
        fun update(id: ULong, o: Value, expected: Value = Value.None) = Op(3, listOf(Value.Id(id), o, expected))
        fun annotate(id: ULong, p: Value, o: Value) = Op(4, listOf(Value.Id(id), p, o))
        fun delete(id: ULong, mode: DeleteMode = DeleteMode.CASCADE) =
            Op(5, listOf(Value.Id(id), Value.UInt(mode.raw)))
        fun statementId(s: Value, p: Value, o: Value) = Op(7, listOf(s, p, o))
        fun declarePredicate(p: Value, multi: Boolean = false, functional: Boolean = false) =
            Op(9, listOf(p, Value.UInt((if (multi) 1uL else 0uL) or (if (functional) 2uL else 0uL))))
        fun createGraph(name: Value) = Op(10, listOf(name))
        fun addMember(item: ULong, graph: Value) = Op(11, listOf(Value.Id(item), graph))
        fun removeMember(item: ULong, graph: Value) = Op(12, listOf(Value.Id(item), graph))
        fun nest(child: Value, parent: Value) = Op(15, listOf(child, parent))
        fun unnest(child: Value, parent: Value) = Op(16, listOf(child, parent))
        fun dropGraph(graph: Value, recursive: Boolean = false) =
            Op(17, listOf(graph, Value.UInt(if (recursive) 1u else 0u)))
        data class Role(val role: Value, val required: Boolean, val many: Boolean = false)
        fun declareHedgeType(ty: Value, roles: List<Role>, attrs: List<Value> = emptyList()): Op {
            val a = mutableListOf(ty, Value.UInt(roles.size.toULong()))
            for (r in roles) {
                a += r.role
                a += Value.UInt((if (r.required) 1uL else 0uL) or (if (r.many) 2uL else 0uL))
            }
            return Op(18, a + attrs)
        }
        fun createHedge(ty: Value, graph: Value, roles: List<Pair<Value, Value>>, attrs: List<Pair<Value, Value>> = emptyList()): Op {
            val a = mutableListOf(ty, graph, Value.UInt(roles.size.toULong()))
            for ((r, f) in roles + attrs) { a += r; a += f }
            return Op(19, a)
        }
    }
}

/** The result of one operation. */
sealed class OpResult {
    object Done : OpResult()
    /** The statement ID, or null for an unpromoted inline property. */
    data class Id(val id: ULong?) : OpResult()
    data class Deleted(val ids: List<ULong>) : OpResult()
    data class Removed(val removed: Boolean) : OpResult()
    data class Changed(val changed: Boolean) : OpResult()
}

/** A statement or inline property; `id` null for an unpromoted inline property. */
data class Quad(val s: ULong, val p: ULong, val o: ULong, val id: ULong?)

/** Errors, mapped one-to-one from the C FFI's `graphlava_status` codes. */
sealed class GraphLavaException(val status: Int, message: String) : Exception(message) {
    class InvalidArgument(m: String) : GraphLavaException(1, m)
    class Internal(m: String) : GraphLavaException(2, m)
    class Busy(m: String) : GraphLavaException(3, m)
    class Abi(m: String) : GraphLavaException(4, m)
    class Closed(m: String) : GraphLavaException(5, m)
    class Io(m: String) : GraphLavaException(10, m)
    class Corrupted(m: String) : GraphLavaException(11, m)
    class Full(m: String) : GraphLavaException(12, m)
    class FormatMismatch(m: String) : GraphLavaException(13, m)
    class Locked(m: String) : GraphLavaException(14, m)
    class Conflict(m: String) : GraphLavaException(15, m)
    class NotFound(m: String) : GraphLavaException(16, m)
    class Referenced(m: String) : GraphLavaException(17, m)
    class GraphConstraint(m: String) : GraphLavaException(18, m)
    class HyperedgeShape(m: String) : GraphLavaException(19, m)
    class CascadeLimit(m: String) : GraphLavaException(20, m)
    class WriteTimeout(m: String) : GraphLavaException(21, m)
    class ReadOnly(m: String) : GraphLavaException(22, m)
    class InvalidRequest(m: String) : GraphLavaException(23, m)
    class Aborted(m: String) : GraphLavaException(24, m)
    class Unknown(status: Int, m: String) : GraphLavaException(status, m)

    companion object {
        fun of(status: Int, m: String): GraphLavaException = when (status) {
            1 -> InvalidArgument(m); 2 -> Internal(m); 3 -> Busy(m); 4 -> Abi(m); 5 -> Closed(m)
            10 -> Io(m); 11 -> Corrupted(m); 12 -> Full(m); 13 -> FormatMismatch(m); 14 -> Locked(m)
            15 -> Conflict(m); 16 -> NotFound(m); 17 -> Referenced(m); 18 -> GraphConstraint(m)
            19 -> HyperedgeShape(m); 20 -> CascadeLimit(m); 21 -> WriteTimeout(m); 22 -> ReadOnly(m)
            23 -> InvalidRequest(m); 24 -> Aborted(m)
            else -> Unknown(status, m)
        }
    }
}

// ---- marshalling ---------------------------------------------------------------------

internal fun makeRequest(ops: List<Op>): Long {
    val out = LongArray(1)
    Native.check(Native.requestNew(out))
    val req = out[0]
    try {
        for (op in ops) {
            val args = op.args.dropLastWhile { it == Value.None }
            val n = args.size
            val kinds = IntArray(n); val ids = LongArray(n)
            val values = arrayOfNulls<String>(n); val dts = arrayOfNulls<String>(n); val langs = arrayOfNulls<String>(n)
            args.forEachIndexed { i, a ->
                when (a) {
                    Value.None -> kinds[i] = 0
                    is Value.Id -> { kinds[i] = 5; ids[i] = a.id.toLong() }
                    is Value.UInt -> { kinds[i] = 6; ids[i] = a.v.toLong() }
                    is Value.T -> when (val t = a.term) {
                        is Term.Iri -> { kinds[i] = 1; values[i] = t.iri }
                        is Term.Blank -> { kinds[i] = 2; values[i] = t.label }
                        is Term.Literal -> { kinds[i] = 3; values[i] = t.lexical; dts[i] = t.datatype; langs[i] = t.lang }
                        else -> throw GraphLavaException.InvalidArgument("$t cannot be written")
                    }
                }
            }
            Native.check(Native.requestAdd(req, op.code, kinds, ids, values, dts, langs))
        }
    } catch (e: Throwable) {
        Native.requestFree(req)
        throw e
    }
    return req
}

internal fun takeResults(res: Long): List<OpResult> {
    if (res == 0L) return emptyList()
    try {
        val all = Native.checkNotNull(Native.resultAll(res))
        return (0 until all.size / 2).map { i ->
            val v = all[2 * i + 1].toULong()
            when (all[2 * i].toInt()) {
                1 -> OpResult.Id(if (v == 0uL) null else v)
                2 -> OpResult.Deleted(Native.checkNotNull(Native.resultDeleted(res, i)).map { it.toULong() })
                3 -> OpResult.Removed(v != 0uL)
                4 -> OpResult.Changed(v != 0uL)
                else -> OpResult.Done
            }
        }
    } finally {
        Native.resultFree(res)
    }
}
