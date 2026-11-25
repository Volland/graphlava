import Foundation
import XCTest
@testable import GraphLava

let EX = "http://ex.org/"

func ex(_ local: String) -> Term { .iri(EX + local) }

/// A fresh directory in the app's private container (the test bundle's
/// temporary directory, which on a simulator is inside the app sandbox).
func tempDir(_ name: String = #function) throws -> URL {
    let d = FileManager.default.temporaryDirectory
        .appendingPathComponent("graphlava-\(ProcessInfo.processInfo.processIdentifier)-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
    return d
}

/// Opens without the platform lifecycle observer; tests drive it explicitly.
func openDB(_ url: URL, appGroup: Bool = false, container: URL? = nil) async throws -> Database {
    try await Database.open(url, options: OpenOptions(appGroup: appGroup, appGroupContainer: container,
                                                      observeLifecycle: false))
}

/// Number of rows and the first row's value of `column`.
func firstValue(_ db: Database, _ q: String, _ column: String) async throws -> (Int, Term?) {
    let rows = try await db.query(q)
    var n = 0
    var first: Term?
    for try await b in rows {
        if first == nil, b.count > 0 { first = try b.term(row: 0, column: column) }
        n += b.count
    }
    return (n, first)
}

let PFX = "PREFIX : <http://ex.org/>\n"
