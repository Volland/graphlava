import Foundation
import GraphLavaFFI
import XCTest
@testable import GraphLava

/// iOS suspend and resume through the lifecycle observer. The observer is
/// driven with notifications on a private center and an immediate runner, so
/// the tests run unhosted on the macOS host and the iOS simulator; the
/// `beginBackgroundTask` path and `0xdead10cc` need a device (task 3.8).
final class SuspendTests: XCTestCase {
    static let bg = Notification.Name("test.background")
    static let fg = Notification.Name("test.foreground")

    func observer(_ db: Database, _ center: NotificationCenter) -> LifecycleObserver {
        LifecycleObserver(database: db, center: center, background: [Self.bg], foreground: [Self.fg],
                          runner: ImmediateRunner())
    }

    /// Whether another owner can open the file right now (the lock is free).
    func lockIsFree(_ url: URL) -> Bool {
        var h: OpaquePointer?
        let st = graphlava_open(url.path, &h)
        if st == GRAPHLAVA_OK { _ = graphlava_close(h); return true }
        XCTAssertEqual(st, GRAPHLAVA_ERR_LOCKED)
        return false
    }

    // @tg: verifies:: [[openspec:mobile-bindings#iOS suspend and resume#Calls while suspended wait for resume]] {level: e2e}
    func testCallsWhileSuspendedWaitForResume() async throws {
        let url = try tempDir().appendingPathComponent("wait.glv")
        let db = try await openDB(url, appGroup: true)
        try await db.update(PFX + "INSERT DATA { :a :p :b }", durability: .sync)
        let center = NotificationCenter()
        let obs = observer(db, center)
        let stale = try await db.query(PFX + "SELECT ?o WHERE { :a :p ?o }")
        center.post(name: Self.bg, object: nil)
        XCTAssertTrue(db.isSuspended)
        XCTAssertTrue(lockIsFree(url), "an app-group file releases its lock")
        let started = Date()
        let reader = Task { () -> (Int, TimeInterval) in
            let (n, _) = try await firstValue(db, PFX + "SELECT ?o WHERE { :a :p ?o }", "o")
            return (n, Date().timeIntervalSince(started))
        }
        try await Task.sleep(nanoseconds: 2_000_000_000)
        center.post(name: Self.fg, object: nil)
        let (n, waited) = try await reader.value
        XCTAssertEqual(n, 1, "correct rows after resume")
        XCTAssertGreaterThanOrEqual(waited, 2.0, "the call completed only after resume")
        do {
            for try await _ in stale {}
            XCTFail("a snapshot from before the suspend must not be reused")
        } catch GraphLavaError.closed {}
        XCTAssertFalse(lockIsFree(url), "resume reacquired the lock")
        _ = obs
        try await db.close()
    }

    // @tg: verifies:: [[openspec:mobile-bindings#iOS suspend and resume#Private container keeps the lock]] {level: e2e}
    func testPrivateContainerKeepsTheLock() async throws {
        let url = try tempDir().appendingPathComponent("private.glv")
        XCTAssertFalse(Database.isAppGroupPath(url))
        let db = try await Database.open(url, options: OpenOptions(observeLifecycle: false))
        XCTAssertFalse(db.isAppGroupFile)
        try await db.update(PFX + "INSERT DATA { :a :p :b }", durability: .relaxed)
        let center = NotificationCenter()
        let obs = observer(db, center)
        center.post(name: Self.bg, object: nil)
        XCTAssertFalse(lockIsFree(url), "the lock is kept (ios-private-container)")
        center.post(name: Self.fg, object: nil)
        let (n, _) = try await firstValue(db, PFX + "SELECT ?o WHERE { :a :p ?o }", "o")
        XCTAssertEqual(n, 1, "the flushed batch is there")
        _ = obs
        try await db.close()
    }

    // @tg: verifies:: [[openspec:mobile-bindings#iOS suspend and resume#Suspend with nothing open]] {level: unit}
    func testSuspendWithNothingOpen() async throws {
        let url = try tempDir().appendingPathComponent("idle.glv")
        let db = try await openDB(url)
        let t = Date()
        try db.suspend()
        XCTAssertLessThan(Date().timeIntervalSince(t), 0.005)
        try db.suspend()
        try db.resume()
        try await db.close()
    }

    func testSuspendAbortsTheOpenWrite() async throws {
        let url = try tempDir().appendingPathComponent("abortw.glv")
        let db = try await openDB(url, appGroup: true)
        do {
            try await db.write(.sync) { txn in
                try txn.insert(ex("x"), ex("p"), ex("y"))
                try db.suspend()
                try txn.insert(ex("x"), ex("p"), ex("z"))
            }
            XCTFail("the write must be aborted")
        } catch GraphLavaError.aborted {}
        try db.resume()
        let (n, _) = try await firstValue(db, PFX + "SELECT ?o WHERE { :x :p ?o }", "o")
        XCTAssertEqual(n, 0)
        try await db.close()
    }
}
