#if os(macOS)
import Foundation
import XCTest
@testable import GraphLava

/// L1 refinement through the Swift package: replays the Lean oracle's traces
/// (`lake exe oracle <seed> <n>`) through `Database` and checks every result
/// and the final state, with the abstraction function of
/// `crates/graphlava/tests/refinement.rs` (node n ↔ `http://oracle/n{n}`,
/// predicate p ↔ `http://oracle/p{p}`, literal n ↔ integer n, graph 0 ↔ the
/// default graph, model IDs ↔ Rust IDs by allocation order).
///
/// Skipped when `lake` is not on PATH unless GRAPHLAVA_REQUIRE_REFINEMENT=1.
/// GRAPHLAVA_REFINEMENT_SEEDS and GRAPHLAVA_REFINEMENT_OPS override 8 × 2,000.
final class OracleReplayTests: XCTestCase {
    static let NS = "http://oracle/"
    static let defaultGraph: ID = (5 << 60) | 1  // graph chunk 0, offset 1 (adr 0038)

    var root: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    }

    func oracle(seed: Int, n: Int) throws -> [[String: Any]]? {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        p.arguments = ["lake", "exe", "oracle", String(seed), String(n)]
        p.currentDirectoryURL = root.appendingPathComponent("formal")
        var env = ProcessInfo.processInfo.environment
        env["PATH"] = (env["PATH"] ?? "") + ":" + NSHomeDirectory() + "/.elan/bin"
        p.environment = env
        let out = Pipe()
        p.standardOutput = out
        p.standardError = Pipe()
        do { try p.run() } catch { return nil }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        guard p.terminationStatus == 0 else { return nil }
        return try String(decoding: data, as: UTF8.self).split(separator: "\n").map {
            try JSONSerialization.jsonObject(with: Data($0.utf8)) as! [String: Any]
        }
    }

    // @tg: verifies:: [[openspec:mobile-bindings#Swift API#Oracle trace replay through Swift]] {level: refinement}
    func testOracleTraceReplayThroughSwift() async throws {
        let env = ProcessInfo.processInfo.environment
        let seeds = Int(env["GRAPHLAVA_REFINEMENT_SEEDS"] ?? "") ?? 8
        let ops = Int(env["GRAPHLAVA_REFINEMENT_OPS"] ?? "") ?? 2000
        guard try oracle(seed: 1, n: 1) != nil else {
            if env["GRAPHLAVA_REQUIRE_REFINEMENT"] != nil { XCTFail("lake not found") }
            throw XCTSkip("`lake` is not available; run `lake build` in formal/")
        }
        for seed in 1...seeds {
            let trace = try XCTUnwrap(try oracle(seed: seed, n: ops))
            let dir = try tempDir()
            let db = try await openDB(dir.appendingPathComponent("replay.glv"))
            let r = Replay(db: db, seed: seed)
            try await r.schema(trace[0]["schema"] as! [String: Any])
            for (i, line) in trace.dropFirst().dropLast().enumerated() {
                try await r.step(line, at: i)
            }
            try await r.checkFinal(trace.last!)
            try await db.close()
        }
    }
}

private func u(_ v: Any?) -> UInt64 { (v as! NSNumber).uint64Value }

final class Replay {
    let db: Database
    let seed: Int
    var toRust: [UInt64: ID] = [:]
    var toModel: [ID: UInt64] = [:]
    var hedges: Set<ID> = []
    let NS = OracleReplayTests.NS

    init(db: Database, seed: Int) {
        self.db = db
        self.seed = seed
    }

    func graph(_ g: Any?) -> Value {
        let n = u(g)
        return n == 0 ? .id(OracleReplayTests.defaultGraph) : .term(.iri("\(NS)g\(n)"))
    }

    func term(_ v: [String: Any]) -> Value {
        if let n = v["node"] { return .term(.iri("\(NS)n\(u(n))")) }
        if let n = v["lit"] { return .term(.integer((n as! NSNumber).int64Value)) }
        return .id(toRust[u(v["stmt"])]!)
    }

    func rustID(_ model: UInt64) -> ID {
        // An ID the model never allocated maps to an ID Rust never allocates.
        toRust[model] ?? ((4 << 60) | (((1 << 40) - 1) << 20) | (model % (1 << 20)))
    }

    func bind(_ expect: UInt64, _ id: ID, _ ctx: String) {
        if let r = toRust[expect] {
            XCTAssertEqual(r, id, ctx)
        } else {
            XCTAssertNil(toModel[id], "\(ctx): an existing ID for a fresh model ID")
            toRust[expect] = id
            toModel[id] = expect
        }
    }

    func submit(_ ops: [Op]) async -> Result<[OpResult], GraphLavaError> {
        do { return .success(try await db.submit(ops, durability: .relaxed)) }
        catch let e as GraphLavaError { return .failure(e) }
        catch { return .failure(.internal("\(error)")) }
    }

    func schema(_ s: [String: Any]) async throws {
        for p in s["multi"] as! [Any] {
            try await db.submit([.declarePredicate(.term(.iri("\(NS)p\(u(p))")), multi: true)], durability: .relaxed)
        }
        for t in s["htypes"] as! [[String: Any]] {
            let required = Set((t["required"] as! [Any]).map(u))
            let roles = (t["roles"] as! [Any]).map(u).map {
                (Value.term(.iri("\(NS)r\($0)")), required: required.contains($0), many: false)
            }
            try await db.submit([.declareHedgeType(.term(.iri("\(NS)T\(u(t["id"]))")), roles: roles)],
                                durability: .relaxed)
        }
    }

    func removed(_ ids: [ID]) -> Set<UInt64> { Set(ids.map { toModel[$0]! }) }
    func want(_ e: [String: Any]) -> Set<UInt64> { Set((e["removed"] as! [Any]).map(u)) }

    func blocked(_ e: GraphLavaError) -> Bool {
        if case .referenced = e { return true }
        return false
    }

    func step(_ line: [String: Any], at i: Int) async throws {
        let ctx = "seed \(seed) op \(i): \(line)"
        let e = line["expect"] as? [String: Any] ?? [:]
        let ok = e["ok"] as? Bool
        switch line["op"] as! String {
        case "insert":
            let t = line["triple"] as! [String: Any]
            let (s, o) = (term(t["s"] as! [String: Any]), term(t["o"] as! [String: Any]))
            let p = Value.term(.iri("\(NS)p\(u(t["p"]))"))
            var res = try await db.submit([.insert(s, p, o, graph: graph(line["graph"]))], durability: .relaxed)
            if case .id(nil) = res[0] {
                res = try await db.submit([.statementID(s, p, o)], durability: .relaxed)
            }
            guard case let .id(id?) = res[0] else { return XCTFail("\(ctx): \(res)") }
            bind(u(e["id"]), id, ctx)
        case "hedge":
            let ty = Value.term(.iri("\(NS)T\(u(line["type"]))"))
            let roles = (line["roles"] as! [[Any]]).map {
                (Value.term(.iri("\(NS)r\(u($0[0]))")), term($0[1] as! [String: Any]))
            }
            let res = await submit([.createHedge(ty, graph: graph(line["graph"]), roles: roles)])
            switch (res, e["id"]) {
            case let (.success(r), expect?):
                guard case let .id(id?) = r[0] else { return XCTFail(ctx) }
                XCTAssertEqual(id >> 60, 6, ctx)
                hedges.insert(id)
                bind(u(expect), id, ctx)
            case (.failure(.hyperedgeShape), nil): break
            default: XCTFail("\(ctx): \(res)")
            }
        case "add_member", "remove_member":
            let item = rustID(u(line["id"]))
            if hedges.contains(item) {
                XCTAssertEqual(ok, false, ctx)
                return
            }
            let add = line["op"] as! String == "add_member"
            let res = await submit([add ? .addMember(item, graph: graph(line["graph"]))
                                        : .removeMember(item, graph: graph(line["graph"]))])
            switch (res, ok) {
            case (.success, true) where add: break
            case let (.success(r), true):
                guard case let .deleted(gone) = r[0] else { return XCTFail(ctx) }
                XCTAssertEqual(removed(gone), want(e), ctx)
            case (.failure(.notFound), false) where e["blocked"] == nil: break
            case (.failure(.invalidRequest), false) where !add && e["blocked"] == nil: break
            case let (.failure(x), false) where !add && blocked(x) && e["blocked"] as? Bool == true: break
            default: XCTFail("\(ctx): \(res)")
            }
        case "nest", "unnest":
            let nest = line["op"] as! String == "nest"
            let (c, p) = (graph(line["child"]), graph(line["parent"]))
            let res = await submit([nest ? .nest(c, under: p) : .unnest(c, from: p)])
            let got: Bool
            switch res {
            case .success where nest: got = true
            case let .success(r): got = r[0] == .changed(true)
            case .failure(.graphConstraint) where nest: got = false
            case .failure(.invalidRequest) where !nest: got = false
            default: return XCTFail("\(ctx): \(res)")
            }
            XCTAssertEqual(got, ok, ctx)
        case "scope":
            let n = u(line["graph"])
            let got: [UInt64] = try await db.read { s in
                let g = n == 0 ? OracleReplayTests.defaultGraph : try s.graph(named: .iri("\(self.NS)g\(n)"))
                guard let g else { return [] }
                return try s.scope(g, direct: line["direct"] as! Bool).map { self.toModel[$0]! }.sorted()
            }
            XCTAssertEqual(got, (e["ids"] as! [Any]).map(u), ctx)
        case "update":
            let res = await submit([.update(id: rustID(u(line["id"])), to: term(line["o"] as! [String: Any]))])
            switch (res, ok) {
            case (.success, true), (.failure(.conflict), false), (.failure(.notFound), false): break
            default: XCTFail("\(ctx): \(res)")
            }
        case "delete":
            let mode: DeleteMode = line["detach"] as! Bool ? .detach : .cascade
            let res = await submit([.delete(id: rustID(u(line["id"])), mode: mode)])
            switch (res, ok) {
            case let (.success(r), true):
                guard case let .deleted(gone) = r[0] else { return XCTFail(ctx) }
                XCTAssertEqual(removed(gone), want(e), ctx)
            case let (.failure(x), false) where blocked(x) && e["blocked"] as? Bool == true: break
            case (.failure(.notFound), false) where e["blocked"] == nil: break
            default: XCTFail("\(ctx): \(res)")
            }
        case let other:
            XCTFail("unknown op \(other)")
        }
    }

    /// α(final Rust state) == final model state.
    func checkFinal(_ line: [String: Any]) async throws {
        let NS = self.NS
        try await db.read { s in
            func json(_ id: ID) throws -> String {
                if let m = self.toModel[id], id >> 60 == 4 { return "s\(m)" }
                switch try s.decode(id) {
                case let .iri(i): return "n" + i.dropFirst("\(NS)n".count)
                case let .literal(v, _, _): return "l\(v)"
                case let t: return "?\(t)"
                }
            }
            func model(_ v: [String: Any]) -> String {
                if let n = v["node"] { return "n\(u(n))" }
                if let n = v["lit"] { return "l\((n as! NSNumber).int64Value)" }
                return "s\(u(v["stmt"]))"
            }
            var got: [String] = []
            for q in try s.find() {
                guard case let .iri(p) = try s.decode(q.p), p.hasPrefix("\(NS)p") else { continue }
                let id = try XCTUnwrap(q.id, "every replayed statement has an ID")
                got.append("\(self.toModel[id]!) \(try json(q.s)) \(p.dropFirst("\(NS)p".count)) \(try json(q.o))")
            }
            let want = (line["final"] as! [[Any]]).map { e -> String in
                let t = e[1] as! [String: Any]
                return "\(u(e[0])) \(model(t["s"] as! [String: Any])) \(u(t["p"])) \(model(t["o"] as! [String: Any]))"
            }
            XCTAssertEqual(got.sorted(), want.sorted(), "seed \(self.seed): final state")
            func graphNo(_ g: ID) throws -> UInt64 {
                if g == OracleReplayTests.defaultGraph { return 0 }
                guard case let .iri(n) = try s.decode(g) else { return .max }
                return UInt64(n.dropFirst("\(NS)g".count))!
            }
            for m in line["members"] as! [[Any]] {
                let id = self.toRust[u(m[0])]!
                XCTAssertEqual(try s.graphs(of: id).map(graphNo).sorted(), (m[1] as! [Any]).map(u),
                               "seed \(self.seed): memberships of \(m[0])")
            }
            var live = 0
            for h in line["hedges"] as! [[Any]] {
                let id = self.toRust[u(h[0])]!
                let e = try XCTUnwrap(try s.hedge(id), "hyperedge \(h[0]) is gone")
                let spec = h[1] as! [String: Any]
                XCTAssertEqual(try s.decode(e.type), .iri("\(NS)T\(u(spec["type"]))"))
                let gotRoles = try e.roles.map { r, n -> String in
                    guard case let .iri(role) = try s.decode(r) else { return "?" }
                    return "\(role) \(try json(n))"
                }.sorted()
                let wantRoles = (spec["roles"] as! [[Any]]).map {
                    "\(NS)r\(u($0[0])) \(model($0[1] as! [String: Any]))"
                }.sorted()
                XCTAssertEqual(gotRoles, wantRoles, "seed \(self.seed): roles of \(h[0])")
                live += 1
            }
            let liveRust = try self.hedges.filter { try s.hedge($0) != nil }.count
            XCTAssertEqual(liveRust, live, "seed \(self.seed): live hyperedges")
            XCTAssertEqual(UInt64(self.toRust.count + 1), u(line["next"]), "seed \(self.seed): allocator")
        }
    }
}
#endif
