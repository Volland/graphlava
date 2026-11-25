import Foundation
import GraphLavaFFI

/// A stored item's 64-bit ID. Always the full unsigned value ([[adr:0042]]):
/// the top four bits are the kind tag, so a signed type would turn most IDs
/// negative.
public typealias ID = UInt64

/// Durability level of a write ([[06-transactions-durability#Durability levels]]).
public enum Durability: Sendable {
    /// Durable when the call returns; closes the group-commit batch at once.
    case sync
    /// Durable when the call returns; shares a group commit.
    case ordered
    /// Atomic and crash-consistent; may be lost until a later durable commit.
    case relaxed

    var raw: UInt32 {
        switch self {
        case .sync: return UInt32(GRAPHLAVA_DURABILITY_SYNC)
        case .ordered: return UInt32(GRAPHLAVA_DURABILITY_ORDERED)
        case .relaxed: return UInt32(GRAPHLAVA_DURABILITY_RELAXED)
        }
    }
}

/// An RDF term as the store sees it.
public enum Term: Hashable, Sendable, CustomStringConvertible {
    case iri(String)
    case blank(String)
    /// Lexical form, datatype IRI (nil: `xsd:string`, or `rdf:langString`
    /// with a tag) and language tag.
    case literal(String, datatype: String? = nil, lang: String? = nil)
    /// An RDF 1.2 triple term in N-Triples form (output only).
    case triple(String)
    /// No value (an unbound variable, ID 0).
    case unbound

    public static let xsd = "http://www.w3.org/2001/XMLSchema#"

    public static func integer(_ v: Int64) -> Term {
        .literal(String(v), datatype: xsd + "integer")
    }

    public static func decimal(_ v: String) -> Term {
        .literal(v, datatype: xsd + "decimal")
    }

    public static func string(_ v: String) -> Term { .literal(v) }

    public var description: String {
        switch self {
        case let .iri(i): return "<\(i)>"
        case let .blank(b): return "_:\(b)"
        case let .literal(v, dt, lang):
            if let lang { return "\"\(v)\"@\(lang)" }
            if let dt { return "\"\(v)\"^^<\(dt)>" }
            return "\"\(v)\""
        case let .triple(t): return t
        case .unbound: return "UNDEF"
        }
    }

    /// The lexical form of a literal or the IRI / label of a node.
    public var value: String? {
        switch self {
        case let .iri(v), let .blank(v), let .triple(v): return v
        case let .literal(v, _, _): return v
        case .unbound: return nil
        }
    }

    init(c t: graphlava_term) {
        let s = { (p: UnsafeMutablePointer<CChar>?) in p.map { String(cString: $0) } }
        switch Int32(t.kind) {
        case GRAPHLAVA_TERM_IRI: self = .iri(s(t.value) ?? "")
        case GRAPHLAVA_TERM_BLANK: self = .blank(s(t.value) ?? "")
        case GRAPHLAVA_TERM_LITERAL:
            // A language-tagged literal reports rdf:langString; keep only the tag.
            let lang = s(t.lang)
            self = .literal(s(t.value) ?? "", datatype: lang == nil ? s(t.datatype) : nil, lang: lang)
        case GRAPHLAVA_TERM_TRIPLE: self = .triple(s(t.value) ?? "")
        default: self = .unbound
        }
    }
}

/// An argument of a write operation: a term, a stored ID, an unsigned
/// integer (counts, modes, flags), or nothing (an optional argument left out).
public enum Value: Hashable, Sendable {
    case term(Term)
    case id(ID)
    case uint(UInt64)
    case none
}

extension Term {
    /// Shorthand so operations take terms directly.
    public var v: Value { .term(self) }
}

/// How a delete treats annotations and object references.
public enum DeleteMode: UInt64, Sendable {
    case cascade = 0, keepAnnotations = 1, detach = 2
}

/// One core operation of a write request ([[16-api#Core API]]).
public struct Op: Hashable, Sendable {
    public let code: UInt32
    public let args: [Value]

    init(_ code: Int32, _ args: [Value]) {
        self.code = UInt32(code)
        self.args = args
    }

    public static func insert(_ s: Value, _ p: Value, _ o: Value, graph: Value = .none) -> Op {
        Op(GRAPHLAVA_OP_INSERT, [s, p, o, graph])
    }
    public static func insert(_ s: Term, _ p: Term, _ o: Term, graph: Term? = nil) -> Op {
        insert(s.v, p.v, o.v, graph: graph.map { .term($0) } ?? .none)
    }
    public static func set(_ s: Value, _ p: Value, _ o: Value) -> Op { Op(GRAPHLAVA_OP_SET, [s, p, o]) }
    public static func update(id: ID, to o: Value, expected: Value = .none) -> Op {
        Op(GRAPHLAVA_OP_UPDATE, [.id(id), o, expected])
    }
    public static func annotate(id: ID, _ p: Value, _ o: Value) -> Op {
        Op(GRAPHLAVA_OP_ANNOTATE, [.id(id), p, o])
    }
    public static func delete(id: ID, mode: DeleteMode = .cascade) -> Op {
        Op(GRAPHLAVA_OP_DELETE, [.id(id), .uint(mode.rawValue)])
    }
    public static func remove(_ s: Value, _ p: Value, _ o: Value, mode: DeleteMode = .cascade,
                              graph: Value = .none) -> Op {
        Op(GRAPHLAVA_OP_REMOVE, [s, p, o, .uint(mode.rawValue), graph])
    }
    public static func statementID(_ s: Value, _ p: Value, _ o: Value) -> Op {
        Op(GRAPHLAVA_OP_STATEMENT_ID, [s, p, o])
    }
    public static func declarePredicate(_ p: Value, multi: Bool = false, functional: Bool = false) -> Op {
        Op(GRAPHLAVA_OP_DECLARE_PREDICATE, [p, .uint((multi ? 1 : 0) | (functional ? 2 : 0))])
    }
    public static func createGraph(_ name: Value) -> Op { Op(GRAPHLAVA_OP_CREATE_GRAPH, [name]) }
    public static func addMember(_ item: ID, graph: Value) -> Op {
        Op(GRAPHLAVA_OP_ADD_MEMBER, [.id(item), graph])
    }
    public static func removeMember(_ item: ID, graph: Value) -> Op {
        Op(GRAPHLAVA_OP_REMOVE_MEMBER, [.id(item), graph])
    }
    public static func nest(_ child: Value, under parent: Value) -> Op { Op(GRAPHLAVA_OP_NEST, [child, parent]) }
    public static func unnest(_ child: Value, from parent: Value) -> Op { Op(GRAPHLAVA_OP_UNNEST, [child, parent]) }
    public static func dropGraph(_ graph: Value, recursive: Bool = false) -> Op {
        Op(GRAPHLAVA_OP_DROP_GRAPH, [graph, .uint(recursive ? 1 : 0)])
    }
    /// A hyperedge type: roles with their `required` and `many` flags, and
    /// attribute predicates.
    public static func declareHedgeType(_ ty: Value, roles: [(Value, required: Bool, many: Bool)],
                                        attrs: [Value] = []) -> Op {
        var a: [Value] = [ty, .uint(UInt64(roles.count))]
        for r in roles { a += [r.0, .uint((r.required ? 1 : 0) | (r.many ? 2 : 0))] }
        return Op(GRAPHLAVA_OP_DECLARE_HEDGE_TYPE, a + attrs)
    }
    public static func createHedge(_ ty: Value, graph: Value = .none, roles: [(Value, Value)],
                                   attrs: [(Value, Value)] = []) -> Op {
        var a: [Value] = [ty, graph, .uint(UInt64(roles.count))]
        for r in roles { a += [r.0, r.1] }
        for x in attrs { a += [x.0, x.1] }
        return Op(GRAPHLAVA_OP_CREATE_HEDGE, a)
    }
    public static func updateRole(hedge: ID, _ role: Value, _ filler: Value) -> Op {
        Op(GRAPHLAVA_OP_UPDATE_ROLE, [.id(hedge), role, filler])
    }

    public static func == (a: Op, b: Op) -> Bool { a.code == b.code && a.args == b.args }
    public func hash(into h: inout Hasher) {
        h.combine(code)
        h.combine(args)
    }
}

/// The result of one operation.
public enum OpResult: Equatable, Sendable {
    case done
    /// The statement ID, or nil for an unpromoted inline property.
    case id(ID?)
    case deleted([ID])
    case removed(Bool)
    case changed(Bool)
}

/// A statement or inline property matching a pattern; `id` is nil for an
/// unpromoted inline property.
public struct Quad: Hashable, Sendable {
    public let s: ID, p: ID, o: ID
    public let id: ID?
}

/// What applying an extension journal did.
public struct JournalReport: Equatable, Sendable {
    public let applied: UInt64
    public let skipped: UInt64
    public let failed: UInt64
    public let droppedBytes: UInt64
}

// ---- marshalling ------------------------------------------------------------------

/// Owns the C strings of a list of arguments for the duration of a call.
final class CArgs {
    private var strings: [UnsafeMutablePointer<CChar>] = []
    private(set) var args: [graphlava_arg] = []

    deinit { strings.forEach { free($0) } }

    private func c(_ s: String?) -> UnsafePointer<CChar>? {
        guard let s, let p = strdup(s) else { return nil }
        strings.append(p)
        return UnsafePointer(p)
    }

    func add(_ v: Value) throws {
        switch v {
        case .none:
            args.append(graphlava_arg(kind: 0, id: 0, value: nil, datatype: nil, lang: nil))
        case let .id(i):
            args.append(graphlava_arg(kind: UInt32(GRAPHLAVA_ARG_ID), id: i, value: nil, datatype: nil, lang: nil))
        case let .uint(u):
            args.append(graphlava_arg(kind: UInt32(GRAPHLAVA_ARG_UINT), id: u, value: nil, datatype: nil, lang: nil))
        case let .term(t):
            args.append(try arg(t))
        }
    }

    func arg(_ t: Term) throws -> graphlava_arg {
        switch t {
        case let .iri(i):
            return graphlava_arg(kind: UInt32(GRAPHLAVA_TERM_IRI), id: 0, value: c(i), datatype: nil, lang: nil)
        case let .blank(b):
            return graphlava_arg(kind: UInt32(GRAPHLAVA_TERM_BLANK), id: 0, value: c(b), datatype: nil, lang: nil)
        case let .literal(v, dt, lang):
            return graphlava_arg(kind: UInt32(GRAPHLAVA_TERM_LITERAL), id: 0, value: c(v), datatype: c(dt),
                                 lang: c(lang))
        case .triple, .unbound:
            throw GraphLavaError.invalidArgument("\(t) cannot be written")
        }
    }
}

/// Builds a C request from operations; the caller frees it.
func makeRequest(_ ops: [Op]) throws -> OpaquePointer {
    var req: OpaquePointer?
    try check(graphlava_request_new(&req))
    do {
        for op in ops {
            let c = CArgs()
            // Trailing `.none` arguments are optional: drop them.
            var args = op.args
            while args.last == Value.none { args.removeLast() }
            for a in args { try c.add(a) }
            try c.args.withUnsafeBufferPointer {
                try check(graphlava_request_add(req, op.code, $0.baseAddress, $0.count))
            }
        }
    } catch {
        graphlava_request_free(req)
        throw error
    }
    return req!
}

func makeTextRequest(_ text: String, datalog: Bool) throws -> OpaquePointer {
    var req: OpaquePointer?
    let lang = UInt32(datalog ? GRAPHLAVA_LANG_DATALOG : GRAPHLAVA_LANG_SPARQL)
    try check(graphlava_request_new_text(lang, text, &req))
    return req!
}

/// Reads and frees a C result.
func takeResults(_ res: OpaquePointer?) throws -> [OpResult] {
    guard let res else { return [] }
    defer { graphlava_result_free(res) }
    var n = 0
    try check(graphlava_result_len(res, &n))
    var out: [OpResult] = []
    out.reserveCapacity(n)
    for i in 0..<n {
        var kind: UInt32 = 0
        var value: UInt64 = 0
        try check(graphlava_result_get(res, i, &kind, &value))
        switch Int32(kind) {
        case GRAPHLAVA_RESULT_ID: out.append(.id(value == 0 ? nil : value))
        case GRAPHLAVA_RESULT_DELETED:
            out.append(.deleted(try fetch { buf, cap, n in graphlava_result_deleted(res, i, buf, cap, n) }))
        case GRAPHLAVA_RESULT_REMOVED: out.append(.removed(value != 0))
        case GRAPHLAVA_RESULT_CHANGED: out.append(.changed(value != 0))
        default: out.append(.done)
        }
    }
    return out
}

/// Runs a listing call with the FFI buffer convention: try a buffer, grow it
/// to the reported size, call again.
func fetch(width: Int = 1, _ call: (UnsafeMutablePointer<UInt64>?, Int, UnsafeMutablePointer<Int>) -> graphlava_status) throws -> [UInt64] {
    var cap = 64
    while true {
        var buf = [UInt64](repeating: 0, count: cap * width)
        var n = 0
        try buf.withUnsafeMutableBufferPointer { b in try check(call(b.baseAddress, cap, &n)) }
        if n <= cap { return Array(buf.prefix(n * width)) }
        cap = n
    }
}
