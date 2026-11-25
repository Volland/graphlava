import Foundation
import XCTest
@testable import GraphLava

/// iOS extension writes: the journal path and the XPC path. The app-group
/// container is a temporary directory here; on a device it is
/// `containerURL(forSecurityApplicationGroupIdentifier:)`.
final class ExtensionTests: XCTestCase {
    override func tearDown() {
        Database.isExtensionProcess = { Bundle.main.bundleURL.pathExtension == "appex" }
    }

    func chain() -> [String] {
        [
            PFX + "INSERT DATA { :n :v 1 }",
            PFX + "DELETE { :n :v 1 } INSERT { :n :v 2 } WHERE { :n :v 1 }",
            PFX + "DELETE { :n :v 2 } INSERT { :n :v 3 } WHERE { :n :v 2 }",
        ]
    }

    // @tg: verifies:: [[openspec:mobile-bindings#iOS extension writes#Share extension write while app is closed]] {level: e2e}
    func testShareExtensionWriteWhileAppIsClosed() async throws {
        let group = try tempDir()
        let url = group.appendingPathComponent("memory.glv")
        try await openDB(url, appGroup: true).close()
        // The share extension, with the app not running.
        let client = ExtensionClient(appGroupContainer: group, extensionID: "share")
        for (i, u) in chain().enumerated() {
            XCTAssertEqual(try client.submit(sparqlUpdate: u), .journal(seq: UInt64(i + 1)))
        }
        // The app launches.
        var db = try await openDB(url, appGroup: true, container: group)
        let rep = await db.journalReports["share"]
        XCTAssertEqual(rep, JournalReport(applied: 3, skipped: 0, failed: 0, droppedBytes: 0))
        let (n, v) = try await firstValue(db, PFX + "SELECT ?v WHERE { :n :v ?v }", "v")
        XCTAssertEqual(n, 1)
        XCTAssertEqual(v?.value, "3", "applied in order")
        let size = try FileManager.default.attributesOfItem(atPath: client.journalURL.path)[.size] as? Int
        XCTAssertEqual(size, 16, "the journal is empty")
        try await db.close()
        db = try await openDB(url, appGroup: true, container: group)
        let again = await db.journalReports["share"]
        XCTAssertEqual(again?.applied, 0, "relaunching does not apply them again")
        try await db.close()
    }

    // @tg: verifies:: [[openspec:mobile-bindings#iOS extension writes#Extension tries to open the file]] {level: unit}
    func testExtensionTriesToOpenTheFile() async throws {
        let url = try tempDir().appendingPathComponent("ext.glv")
        Database.isExtensionProcess = { true }
        do {
            _ = try await openDB(url)
            XCTFail("an extension must not open the database")
        } catch GraphLavaError.extensionProcess {}
        XCTAssertFalse(FileManager.default.fileExists(atPath: url.path), "the file is not created or locked")
    }

    // @tg: verifies:: [[openspec:mobile-bindings#iOS extension writes#Corrupt journal entry]] {level: e2e}
    func testCorruptJournalEntry() async throws {
        let group = try tempDir()
        let url = group.appendingPathComponent("memory.glv")
        let client = ExtensionClient(appGroupContainer: group, extensionID: "share")
        for u in chain() { try client.submit(sparqlUpdate: u) }
        // The extension was killed mid-append: the last entry's checksum is bad.
        var bytes = try Data(contentsOf: client.journalURL)
        bytes[bytes.count - 1] ^= 0xFF
        try bytes.write(to: client.journalURL)
        var warnings: [String] = []
        let saved = Database.warn
        Database.warn = { warnings.append($0) }
        defer { Database.warn = saved }
        let db = try await openDB(url, appGroup: true, container: group)
        let rep = await db.journalReports["share"]
        XCTAssertEqual(rep?.applied, 2, "the earlier entries are applied")
        XCTAssertGreaterThan(rep?.droppedBytes ?? 0, 0, "the torn tail is dropped")
        XCTAssertEqual(warnings.count, 1, "one warning")
        let (_, v) = try await firstValue(db, PFX + "SELECT ?v WHERE { :n :v ?v }", "v")
        XCTAssertEqual(v?.value, "2")
        try await db.close()
    }

    func testXPCPathWhileTheAppRuns() async throws {
        let group = try tempDir()
        let db = try await openDB(group.appendingPathComponent("memory.glv"), appGroup: true)
        let owner = OwnerListener(database: db)
        defer { owner.invalidate() }
        let client = ExtensionClient(appGroupContainer: group, extensionID: "share", endpoint: owner.endpoint)
        let d = try await Task.detached { try client.submit([.insert(ex("x"), ex("shared"), ex("y"))]) }.value
        XCTAssertEqual(d, .owner)
        let (n, _) = try await firstValue(db, PFX + "SELECT ?o WHERE { :x :shared ?o }", "o")
        XCTAssertEqual(n, 1)
        XCTAssertFalse(FileManager.default.fileExists(atPath: client.journalURL.path), "no journal needed")
        try await db.close()
    }
}
