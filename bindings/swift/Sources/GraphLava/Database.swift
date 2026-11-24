import Foundation
import GraphLavaFFI
#if canImport(os)
import os
#endif

/// Runs every job of one `Database` actor on one dedicated thread, so FFI
/// calls (which may block on the writer or on a suspension) never occupy the
/// cooperative pool or the main thread ([[16-api#Mobile bindings]], design D3).
final class DedicatedThreadExecutor: SerialExecutor, @unchecked Sendable {
    private final class Queue: @unchecked Sendable {
        let cond = NSCondition()
        var jobs: [UnownedJob] = []
        var stopped = false
        var executor: UnownedSerialExecutor?
    }

    private let queue = Queue()

    init(name: String) {
        let q = queue
        let thread = Thread { [q] in
            while true {
                q.cond.lock()
                while q.jobs.isEmpty && !q.stopped { q.cond.wait() }
                if q.jobs.isEmpty && q.stopped {
                    q.cond.unlock()
                    return
                }
                let job = q.jobs.removeFirst()
                q.cond.unlock()
                job.runSynchronously(on: q.executor!)
            }
        }
        thread.name = name
        thread.qualityOfService = .userInitiated
        thread.start()
    }

    func enqueue(_ job: UnownedJob) {
        queue.cond.lock()
        if queue.executor == nil { queue.executor = asUnownedSerialExecutor() }
        queue.jobs.append(job)
        queue.cond.signal()
        queue.cond.unlock()
    }

    func asUnownedSerialExecutor() -> UnownedSerialExecutor {
        UnownedSerialExecutor(ordinary: self)
    }

    deinit {
        queue.cond.lock()
        queue.stopped = true
        queue.cond.signal()
        queue.cond.unlock()
    }
}

/// Settings for `Database.open`.
public struct OpenOptions: Sendable {
    /// Whether the file is in an app-group container, so suspend must release
    /// the file lock (`0xdead10cc`). Nil: decided from the path.
    public var appGroup: Bool?
    /// The app-group container whose `graphlava/*.journal` files are applied
    /// on open (iOS extension writes). Nil: no journals.
    public var appGroupContainer: URL?
    /// Suspend on background and resume on foreground automatically (iOS).
    public var observeLifecycle: Bool

    public init(appGroup: Bool? = nil, appGroupContainer: URL? = nil, observeLifecycle: Bool = true) {
        self.appGroup = appGroup
        self.appGroupContainer = appGroupContainer
        self.observeLifecycle = observeLifecycle
    }
}

/// The owner handle of one database file ([[16-api#Rust API]] through the C
/// FFI). An actor on a dedicated thread: every method is `async` from
/// outside. Transactions are closure-scoped: `try await db.write(.sync) {
/// txn in ... }` commits when the closure returns and aborts when it throws.
// @tg: implements:: [[openspec:mobile-bindings#Swift API]]
public actor Database {
    nonisolated(unsafe) let handle: OpaquePointer
    nonisolated let executor: DedicatedThreadExecutor
    /// The database file.
    public nonisolated let url: URL
    /// Whether suspend releases the file lock (an app-group file).
    public nonisolated let isAppGroupFile: Bool
    /// The journals applied on open, by extension identifier.
    public private(set) var journalReports: [String: JournalReport] = [:]
    nonisolated(unsafe) var lifecycle: LifecycleObserver?
    private var closed = false

    public nonisolated var unownedExecutor: UnownedSerialExecutor {
        executor.asUnownedSerialExecutor()
    }

    /// Overrides the extension-process check (tests).
    nonisolated(unsafe) static var isExtensionProcess: () -> Bool = {
        Bundle.main.bundleURL.pathExtension == "appex"
    }

    /// The core version of the C library, equal to the header the package
    /// was built from.
    public static var coreVersion: String { String(cString: graphlava_version()) }

    private init(handle: OpaquePointer, url: URL, appGroup: Bool) {
        self.handle = handle
        self.url = url
        self.isAppGroupFile = appGroup
        self.executor = DedicatedThreadExecutor(name: "graphlava \(url.lastPathComponent)")
    }

    /// Opens (or creates) the database file and takes the owner lock. Throws
    /// `extensionProcess` in an app extension without touching the file.
    /// Applies pending extension journals before returning.
    public static func open(_ url: URL, options: OpenOptions = OpenOptions()) async throws -> Database {
        if isExtensionProcess() { throw GraphLavaError.extensionProcess }
        var abi: UInt32 = 0
        try check(graphlava_abi_version(&abi))
        guard abi == UInt32(GRAPHLAVA_ABI_VERSION) else {
            throw GraphLavaError.abi("library ABI \(abi), package built for \(GRAPHLAVA_ABI_VERSION)")
        }
        let appGroup = options.appGroup ?? isAppGroupPath(url)
        var h: OpaquePointer?
        let flags = appGroup ? UInt32(GRAPHLAVA_OPEN_APP_GROUP) : 0
        try check(graphlava_open_ex(UInt32(GRAPHLAVA_ABI_VERSION), url.path, flags, &h))
        let db = Database(handle: h!, url: url, appGroup: appGroup)
        if let container = options.appGroupContainer {
            try await db.applyJournals(in: container)
        }
        if options.observeLifecycle {
            db.lifecycle = LifecycleObserver.standard(for: db)
        }
        return db
    }

    /// Whether a path lies in an app-group container (`.../Shared/AppGroup/...`).
    public static func isAppGroupPath(_ url: URL) -> Bool {
        url.standardizedFileURL.path.contains("/Shared/AppGroup/")
    }

    deinit {
        lifecycle = nil
        if !closed { _ = graphlava_close(handle) }
    }

    /// Commits any open batch and closes the file. Throws `busy` while row
    /// streams or snapshots are alive.
    public func close() throws {
        guard !closed else { return }
        try check(graphlava_close(handle))
        closed = true
        lifecycle = nil
    }

    // ---- writes ------------------------------------------------------------------

    /// Runs operations as one write request: all or none, acknowledged once
    /// the durability level holds.
    @discardableResult
    public func submit(_ ops: [Op], durability: Durability = .ordered) throws -> [OpResult] {
        let req = try makeRequest(ops)
        defer { graphlava_request_free(req) }
        var res: OpaquePointer?
        try check(graphlava_submit(handle, req, durability.raw, &res))
        return try takeResults(res)
    }

    /// Runs a SPARQL 1.2 Update as one write request.
    public func update(_ sparql: String, durability: Durability = .ordered) throws {
        try check(graphlava_sparql_update(handle, sparql, durability.raw))
    }

    /// Runs a Datalog program whose `@assert` heads write, as one request.
    public func datalogUpdate(_ program: String, durability: Durability = .ordered) throws {
        try check(graphlava_datalog_update(handle, program, durability.raw))
    }

    /// An interactive write transaction scoped to `body`: committed when it
    /// returns, aborted when it throws. It holds the writer for at most the
    /// engine's write time limit (100 ms); prefer `submit` for one-shot
    /// writes.
    public func write<R>(_ durability: Durability = .ordered,
                         _ body: (WriteTransaction) throws -> R) throws -> R {
        var w: OpaquePointer?
        try check(graphlava_write_begin(handle, durability.raw, &w))
        let txn = WriteTransaction(w!)
        let r: R
        do {
            r = try body(txn)
        } catch {
            txn.end()
            _ = graphlava_abort(w)
            throw error
        }
        txn.end()
        try check(graphlava_commit(w))
        return r
    }

    // ---- reads -------------------------------------------------------------------

    /// Core reads on one snapshot scoped to `body`.
    public func read<R>(_ body: (ReadSnapshot) throws -> R) throws -> R {
        var r: OpaquePointer?
        try check(graphlava_read_begin(handle, &r))
        let snap = ReadSnapshot(r!)
        defer { snap.end() }
        return try body(snap)
    }

    /// Runs a SPARQL query (or a Datalog program with goals) on a fresh
    /// snapshot. Rows arrive as an `AsyncSequence` of batches of at most 1024
    /// rows; the snapshot ends when the sequence is exhausted, cancelled or
    /// dropped.
    public func query(_ text: String, datalog: Bool = false) throws -> Rows {
        var p: OpaquePointer?
        try check(graphlava_prepare(handle, UInt32(datalog ? GRAPHLAVA_LANG_DATALOG : GRAPHLAVA_LANG_SPARQL),
                                    text, &p))
        defer { graphlava_prepared_free(p) }
        var rows: OpaquePointer?
        try check(graphlava_query(handle, p, &rows))
        return Rows(RowsHandle(rows!))
    }

    // ---- lifecycle ----------------------------------------------------------------

    /// Suspends for the background (rule `api-ios-suspend`): ends snapshots,
    /// aborts the open write transaction, flushes the batch and, for an
    /// app-group file, releases the file lock. Calls made meanwhile wait for
    /// `resume`. Not isolated: it must run while the actor is busy.
    public nonisolated func suspend() throws {
        try check(graphlava_suspend(handle))
    }

    /// Ends a suspension; reacquires the file lock when suspend released it.
    public nonisolated func resume() throws {
        try check(graphlava_resume(handle))
    }

    /// Commits the open batch durably now, without suspending.
    public nonisolated func flush() throws {
        try check(graphlava_flush(handle))
    }

    public nonisolated var isSuspended: Bool {
        var s: UInt32 = 0
        return graphlava_is_suspended(handle, &s) == GRAPHLAVA_OK && s != 0
    }

    /// Number of open read snapshots (diagnostics).
    public nonisolated func debugOpenSnapshots() throws -> Int {
        var n = 0
        try check(graphlava_open_snapshots(handle, &n))
        return n
    }

    // ---- administration ---------------------------------------------------------

    /// A full dictionary sweep; returns the number of terms deleted.
    @discardableResult
    public func sweep(window: TimeInterval = 0) throws -> UInt64 {
        var n: UInt64 = 0
        try check(graphlava_sweep(handle, UInt64(window * 1000), &n))
        return n
    }

    /// Writes a consistent copy of the database file.
    public func exportSnapshot(to url: URL) throws {
        try check(graphlava_export_snapshot(handle, url.path))
    }

    // ---- extension journals -------------------------------------------------------

    /// Applies every `<container>/graphlava/<extension-id>.journal`, each frame
    /// exactly once, dropping a torn tail with one warning.
    @discardableResult
    public func applyJournals(in container: URL) throws -> [String: JournalReport] {
        let dir = ExtensionClient.journalDirectory(in: container)
        let files = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []
        for f in files.sorted(by: { $0.path < $1.path }) where f.pathExtension == "journal" {
            let key = f.deletingPathExtension().lastPathComponent
            var rep = graphlava_journal_report()
            try check(graphlava_journal_apply(handle, f.path, key, &rep))
            let r = JournalReport(applied: rep.applied, skipped: rep.skipped, failed: rep.failed,
                                  droppedBytes: rep.dropped_bytes)
            if r.droppedBytes > 0 { Self.warn("dropped a torn tail of \(r.droppedBytes) bytes from \(f.lastPathComponent)") }
            journalReports[key] = r
        }
        return journalReports
    }

    /// Applies an encoded write request received from an extension over XPC.
    func submitEncoded(_ bytes: Data, durability: Durability) throws {
        var req: OpaquePointer?
        try bytes.withUnsafeBytes { b in
            try check(graphlava_request_decode(b.bindMemory(to: UInt8.self).baseAddress, b.count, &req))
        }
        defer { graphlava_request_free(req) }
        try check(graphlava_submit(handle, req, durability.raw, nil))
    }

    /// Warnings go to the unified log (and stderr in tests).
    nonisolated(unsafe) static var warn: (String) -> Void = { msg in
        #if canImport(os)
        Logger(subsystem: "org.graphlava", category: "journal").warning("\(msg, privacy: .public)")
        #else
        FileHandle.standardError.write(Data((msg + "\n").utf8))
        #endif
    }
}

/// An interactive write transaction; valid only inside `Database.write`.
public final class WriteTransaction {
    private var handle: OpaquePointer?

    init(_ h: OpaquePointer) { handle = h }

    func end() { handle = nil }

    private func live() throws -> OpaquePointer {
        guard let handle else { throw GraphLavaError.closed("the write transaction has ended") }
        return handle
    }

    /// Runs operations in order; a failing operation undoes only itself.
    @discardableResult
    public func run(_ ops: [Op]) throws -> [OpResult] {
        let h = try live()
        let req = try makeRequest(ops)
        defer { graphlava_request_free(req) }
        var res: OpaquePointer?
        try check(graphlava_write_run(h, req, &res))
        return try takeResults(res)
    }

    /// Inserts one statement; returns its ID (nil for an unpromoted inline property).
    @discardableResult
    public func insert(_ s: Term, _ p: Term, _ o: Term, graph: Term? = nil) throws -> ID? {
        guard case let .id(id)? = try run([.insert(s, p, o, graph: graph)]).first else { return nil }
        return id
    }

    /// Runs a SPARQL Update inside the transaction.
    public func update(_ sparql: String) throws {
        let h = try live()
        let req = try makeTextRequest(sparql, datalog: false)
        defer { graphlava_request_free(req) }
        try check(graphlava_write_run(h, req, nil))
    }
}

/// Core reads on one snapshot; valid only inside `Database.read`.
public final class ReadSnapshot {
    private var handle: OpaquePointer?

    init(_ h: OpaquePointer) { handle = h }

    func end() {
        if let h = handle { graphlava_read_end(h) }
        handle = nil
    }

    private func live() throws -> OpaquePointer {
        guard let handle else { throw GraphLavaError.closed("the snapshot has ended") }
        return handle
    }

    /// The ID of a term without interning it.
    public func lookup(_ t: Term) throws -> ID? {
        let h = try live()
        let c = CArgs()
        var a = try c.arg(t)
        var id: UInt64 = 0
        try check(graphlava_lookup(h, &a, &id))
        return id == 0 ? nil : id
    }

    public func decode(_ id: ID) throws -> Term {
        let h = try live()
        var t = graphlava_term()
        try check(graphlava_decode(h, id, &t))
        defer { graphlava_term_free(&t) }
        return Term(c: t)
    }

    private func quads(_ call: (UnsafeMutablePointer<UInt64>?, Int, UnsafeMutablePointer<Int>) -> graphlava_status) throws -> [Quad] {
        let v = try fetch(width: 4, call)
        return stride(from: 0, to: v.count, by: 4).map { Quad(s: v[$0], p: v[$0 + 1], o: v[$0 + 2], id: v[$0 + 3] == 0 ? nil : v[$0 + 3]) }
    }

    /// Statements and inline properties matching a pattern (nil = any).
    public func find(_ s: ID? = nil, _ p: ID? = nil, _ o: ID? = nil) throws -> [Quad] {
        let h = try live()
        return try quads { graphlava_find(h, s ?? 0, p ?? 0, o ?? 0, $0, $1, $2) }
    }

    public func out(_ s: ID, _ p: ID? = nil) throws -> [Quad] {
        let h = try live()
        return try quads { graphlava_out(h, s, p ?? 0, $0, $1, $2) }
    }

    public func incoming(_ o: ID, _ p: ID? = nil) throws -> [Quad] {
        let h = try live()
        return try quads { graphlava_in(h, o, p ?? 0, $0, $1, $2) }
    }

    public func props(_ node: ID) throws -> [Quad] {
        let h = try live()
        return try quads { graphlava_props(h, node, $0, $1, $2) }
    }

    /// The triple of a statement, or nil.
    public func stmt(_ id: ID) throws -> (s: ID, p: ID, o: ID)? {
        let h = try live()
        var spo = [UInt64](repeating: 0, count: 3)
        let st = graphlava_stmt(h, id, &spo)
        if st == GRAPHLAVA_ERR_NOT_FOUND { return nil }
        try check(st)
        return (spo[0], spo[1], spo[2])
    }

    /// The graph named by a term, without creating it.
    public func graph(named t: Term) throws -> ID? {
        let h = try live()
        let c = CArgs()
        var a = try c.arg(t)
        var id: UInt64 = 0
        try check(graphlava_graph(h, &a, &id))
        return id == 0 ? nil : id
    }

    public func graphs(of item: ID) throws -> [ID] {
        let h = try live()
        return try fetch { graphlava_graphs_of(h, item, $0, $1, $2) }
    }

    /// The asserted statements in a scope (`direct`: the graph only).
    public func scope(_ graph: ID, direct: Bool) throws -> [ID] {
        let h = try live()
        return try fetch { graphlava_scope(h, graph, direct ? 1 : 0, $0, $1, $2) }
    }

    /// A hyperedge: its type, home graph and `(role, filler)` pairs, or nil.
    public func hedge(_ id: ID) throws -> (type: ID, graph: ID, roles: [(ID, ID)])? {
        let h = try live()
        var ty: UInt64 = 0, g: UInt64 = 0
        var notFound = false
        let v = try fetch(width: 2) { buf, cap, n in
            let st = graphlava_hedge(h, id, &ty, &g, buf, cap, n)
            if st == GRAPHLAVA_ERR_NOT_FOUND { notFound = true; n.pointee = 0; return GRAPHLAVA_OK }
            return st
        }
        if notFound { return nil }
        return (ty, g, stride(from: 0, to: v.count, by: 2).map { (v[$0], v[$0 + 1]) })
    }
}
