import Foundation
import XCTest
import GraphLava

/// Every snippet of bindings/swift/README.md, run on the host.
final class ReadmeTests: XCTestCase {
    func testReadmeSnippets() async throws {
        let url = try tempDir().appendingPathComponent("memory.glv")
        let ex = { (local: String) in Term.iri("http://example.org/\(local)") }

        // Quick start
        let db = try await Database.open(url)
        let results = try await db.submit([.insert(ex("alice"), ex("worksAt"), ex("acme"))], durability: .ordered)
        guard case let .id(works?) = results[0] else { return XCTFail("expected a statement ID") }
        try await db.submit([.annotate(id: works, ex("confidence").v, Term.literal("0.9", datatype: "http://www.w3.org/2001/XMLSchema#decimal").v)])
        try await db.submit([.update(id: works, to: ex("globex").v)])   // same ID, annotation kept

        // SPARQL
        try await db.update("""
            PREFIX : <http://example.org/>
            INSERT DATA { :a :knows :b . :b :knows :c }
            """)
        var rows = 0
        for try await batch in try await db.query("""
            PREFIX : <http://example.org/>
            SELECT ?org ?c WHERE { << :alice :worksAt ?org ~ ?r >> . ?r :confidence ?c }
            """) {
            for i in 0..<batch.count {
                let org = try batch.term(row: i, column: "org")
                XCTAssertEqual(org, ex("globex"))
                rows += 1
            }
        }
        XCTAssertEqual(rows, 1)

        // Datalog
        var reached = 0
        for try await batch in try await db.query("""
            @prefix : <http://example.org/> .
            reach(?x, ?y) :- :knows(?x, ?y) .
            reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
            ?- reach(:a, ?who) .
            """, datalog: true) {
            reached += batch.count
        }
        XCTAssertEqual(reached, 2)

        // Interactive write and snapshot reads
        try await db.write(.sync) { txn in
            _ = try txn.insert(ex("bob"), ex("knows"), ex("alice"))
        }
        let n = try await db.read { snap -> Int in
            guard let bob = try snap.lookup(ex("bob")) else { return 0 }
            return try snap.out(bob).count
        }
        XCTAssertEqual(n, 1)

        try await db.close()
    }
}
