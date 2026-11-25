import Foundation
import XCTest
@testable import GraphLava

final class SwiftAPITests: XCTestCase {
    /// The agent-memory session ([[21-e2e-tests#Agent memory session]]) through the Swift API.
    // @tg: verifies:: [[openspec:mobile-bindings#Swift API#Agent memory session from Swift]] {level: e2e}
    func testAgentMemorySessionFromSwift() async throws {
        let url = try tempDir().appendingPathComponent("memory.glv")
        var db = try await openDB(url)
        let res = try await db.submit([
            .insert(ex("alice"), ex("worksAt"), ex("acme"), graph: ex("episode1")),
            .insert(ex("alice"), ex("age"), .integer(42)),
        ], durability: .ordered)
        guard case let .id(works?) = res[0] else { return XCTFail("statement ID: \(res)") }
        try await db.submit([.annotate(id: works, ex("confidence").v, Term.decimal("0.9").v)], durability: .ordered)
        try await db.update(PFX + "INSERT DATA { :episode1 <https://graphlava.org/ns#in> :agent }")
        let q = PFX + "SELECT ?r ?c WHERE { << :alice :worksAt :acme ~ ?r >> . ?r :confidence ?c }"
        let (n, c) = try await firstValue(db, q, "c")
        XCTAssertEqual(n, 1)
        XCTAssertEqual(c?.value, "0.9")
        let before: [ID] = try await db.read { s in
            [try s.lookup(ex("alice"))!, works]
        }
        try await db.close()
        db = try await openDB(url)
        let (n2, c2) = try await firstValue(db, q, "c")
        XCTAssertEqual(n2, 1)
        XCTAssertEqual(c2?.value, "0.9", "the reifier query returns the stored confidence")
        let after: [ID] = try await db.read { s in
            let alice = try s.lookup(ex("alice"))!
            let quads = try s.out(alice, try s.lookup(ex("worksAt")))
            return [alice, quads.first?.id ?? 0]
        }
        XCTAssertEqual(before, after, "IDs equal the ones returned before the reopen")
        try await db.close()
    }

    // @tg: verifies:: [[openspec:mobile-bindings#Swift API#Throwing closure aborts]] {level: e2e}
    func testThrowingClosureAborts() async throws {
        struct Boom: Error {}
        let url = try tempDir().appendingPathComponent("abort.glv")
        var db = try await openDB(url)
        do {
            try await db.write(.sync) { txn in
                for i in 0..<3 { try txn.insert(ex("s\(i)"), ex("p"), ex("o")) }
                throw Boom()
            }
            XCTFail("the error must reach the caller")
        } catch is Boom {}
        try await db.close()
        db = try await openDB(url)
        let (n, _) = try await firstValue(db, PFX + "SELECT ?s WHERE { ?s :p :o }", "s")
        XCTAssertEqual(n, 0, "none of the 3 statements exist after reopening")
        // A closure that returns commits.
        let id = try await db.write(.sync) { txn in try txn.insert(ex("k"), ex("p"), ex("o")) }
        XCTAssertNotNil(id)
        try await db.close()
    }

    func testRowsArriveIn1024RowBatchesWithUnsignedIDs() async throws {
        let url = try tempDir().appendingPathComponent("rows.glv")
        let db = try await openDB(url)
        let ops = (0..<2500).map { Op.insert(ex("n\($0)"), ex("v"), .integer(Int64($0))) }
        try await db.submit(ops, durability: .relaxed)
        var sizes: [Int] = []
        var maxID: ID = 0
        for try await b in try await db.query(PFX + "SELECT ?s ?v WHERE { ?s :v ?v }") {
            sizes.append(b.count)
            maxID = max(maxID, b.ids.max() ?? 0)
        }
        XCTAssertEqual(sizes, [1024, 1024, 452])
        XCTAssertGreaterThan(maxID, UInt64(Int64.max), "inline literal IDs use the top bit; UInt64 keeps them")
        XCTAssertEqual(try db.debugOpenSnapshots(), 0)
        try await db.close()
    }

    func testCancelledIterationReleasesTheSnapshot() async throws {
        let url = try tempDir().appendingPathComponent("cancel.glv")
        let db = try await openDB(url)
        try await db.submit((0..<3000).map { Op.insert(ex("n\($0)"), ex("v"), ex("x")) }, durability: .relaxed)
        let rows = try await db.query(PFX + "SELECT ?s WHERE { ?s :v :x }")
        XCTAssertEqual(try db.debugOpenSnapshots(), 1)
        for try await _ in rows { break }
        rows.close()
        XCTAssertEqual(try db.debugOpenSnapshots(), 0)
        try await db.close()
    }

    func testErrorsMapOneToOne() async throws {
        let url = try tempDir().appendingPathComponent("err.glv")
        let db = try await openDB(url)
        do {
            _ = try await db.query("SELEKT nothing")
            XCTFail()
        } catch let GraphLavaError.invalidRequest(m) {
            XCTAssertFalse(m.isEmpty)
        }
        do {
            _ = try await openDB(url)
            XCTFail()
        } catch GraphLavaError.locked {}
        try await db.close()
        do {
            try await db.update("INSERT DATA {}")
            XCTFail()
        } catch GraphLavaError.closed {}
    }

    func testCoreVersionMatchesTheHeader() throws {
        XCTAssertFalse(Database.coreVersion.isEmpty)
        #if os(macOS)
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
        if let v = try? String(contentsOf: root.appendingPathComponent("GraphLavaFFI.xcframework/VERSION")) {
            XCTAssertEqual(v.trimmingCharacters(in: .whitespacesAndNewlines), Database.coreVersion)
        }
        #endif
    }
}
