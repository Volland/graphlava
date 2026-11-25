import Foundation
import GraphLavaFFI

/// The XPC interface the owner app exports to its extensions.
@objc public protocol GraphLavaOwnerProtocol {
    /// Runs an encoded write request; replies with the `graphlava_status`
    /// code and a message.
    func submit(_ request: Data, durability: UInt32, reply: @escaping (UInt32, String?) -> Void)
}

/// Where an extension write went.
public enum Delivery: Equatable, Sendable {
    /// Applied by the running owner app over XPC.
    case owner
    /// Appended to the extension's journal with this sequence number; the
    /// owner applies it on its next open.
    case journal(seq: UInt64)
}

/// Writes from an iOS app extension (share, widget, intents), which must never
/// open the database file ([[adr:0002]], ADR 0043). A request goes to the
/// running owner app over XPC when an endpoint is available, and otherwise to
/// the extension's own journal `<appgroup>/graphlava/<extension-id>.journal`,
/// which the owner applies exactly once on its next open.
// @tg: implements:: [[openspec:mobile-bindings#iOS extension writes]]
public final class ExtensionClient: @unchecked Sendable {
    public let container: URL
    public let extensionID: String
    private let endpoint: NSXPCListenerEndpoint?
    private let timeout: TimeInterval

    /// - Parameters:
    ///   - container: the app-group container shared with the owner app.
    ///   - extensionID: the journal's name and its exactly-once key.
    ///   - endpoint: the owner's XPC endpoint, when the extension has one.
    public init(appGroupContainer container: URL, extensionID: String,
                endpoint: NSXPCListenerEndpoint? = nil, timeout: TimeInterval = 2) {
        self.container = container
        self.extensionID = extensionID
        self.endpoint = endpoint
        self.timeout = timeout
    }

    static func journalDirectory(in container: URL) -> URL {
        container.appendingPathComponent("graphlava", isDirectory: true)
    }

    public var journalURL: URL {
        Self.journalDirectory(in: container).appendingPathComponent("\(extensionID).journal")
    }

    /// Submits core operations.
    @discardableResult
    public func submit(_ ops: [Op], durability: Durability = .ordered) throws -> Delivery {
        let req = try makeRequest(ops)
        defer { graphlava_request_free(req) }
        return try deliver(req, durability: durability)
    }

    /// Submits a SPARQL Update.
    @discardableResult
    public func submit(sparqlUpdate: String, durability: Durability = .ordered) throws -> Delivery {
        let req = try makeTextRequest(sparqlUpdate, datalog: false)
        defer { graphlava_request_free(req) }
        return try deliver(req, durability: durability)
    }

    private func deliver(_ req: OpaquePointer, durability: Durability) throws -> Delivery {
        if let endpoint, try sendOverXPC(req, endpoint: endpoint, durability: durability) {
            return .owner
        }
        try FileManager.default.createDirectory(at: Self.journalDirectory(in: container),
                                                withIntermediateDirectories: true)
        var seq: UInt64 = 0
        try check(graphlava_journal_append(journalURL.path, req, &seq))
        return .journal(seq: seq)
    }

    /// Returns false when the owner is unreachable (the caller falls back to
    /// the journal); throws when the owner ran the request and it failed.
    private func sendOverXPC(_ req: OpaquePointer, endpoint: NSXPCListenerEndpoint,
                             durability: Durability) throws -> Bool {
        var len = 0
        try check(graphlava_request_encode(req, nil, 0, &len))
        var bytes = [UInt8](repeating: 0, count: len)
        try check(graphlava_request_encode(req, &bytes, len, &len))
        let conn = NSXPCConnection(listenerEndpoint: endpoint)
        conn.remoteObjectInterface = NSXPCInterface(with: GraphLavaOwnerProtocol.self)
        conn.resume()
        defer { conn.invalidate() }
        let done = DispatchSemaphore(value: 0)
        final class Box: @unchecked Sendable { var reply: (UInt32, String?)?; var failed = false }
        let box = Box()
        let proxy = conn.remoteObjectProxyWithErrorHandler { _ in
            box.failed = true
            done.signal()
        } as? GraphLavaOwnerProtocol
        guard let proxy else { return false }
        proxy.submit(Data(bytes), durability: durability.raw) { st, msg in
            box.reply = (st, msg)
            done.signal()
        }
        if done.wait(timeout: .now() + timeout) == .timedOut || box.failed { return false }
        guard let (st, msg) = box.reply else { return false }
        if st != GRAPHLAVA_OK.rawValue {
            throw GraphLavaError(status: graphlava_status(rawValue: st), message: msg ?? "")
        }
        return true
    }
}

/// The owner side of the XPC path: exports `GraphLavaOwnerProtocol` and runs
/// each request on the database. Hand `endpoint` to extensions that can
/// receive it (an anonymous listener), or create the listener with a Mach
/// service name on macOS.
public final class OwnerListener: NSObject, NSXPCListenerDelegate, GraphLavaOwnerProtocol, @unchecked Sendable {
    public let listener: NSXPCListener
    private let db: Database

    public init(database: Database, listener: NSXPCListener = .anonymous()) {
        db = database
        self.listener = listener
        super.init()
        listener.delegate = self
        listener.resume()
    }

    public var endpoint: NSXPCListenerEndpoint { listener.endpoint }

    public func invalidate() { listener.invalidate() }

    public func listener(_ listener: NSXPCListener, shouldAcceptNewConnection c: NSXPCConnection) -> Bool {
        c.exportedInterface = NSXPCInterface(with: GraphLavaOwnerProtocol.self)
        c.exportedObject = self
        c.resume()
        return true
    }

    public func submit(_ request: Data, durability: UInt32, reply: @escaping (UInt32, String?) -> Void) {
        let d: Durability = durability == UInt32(GRAPHLAVA_DURABILITY_SYNC) ? .sync
            : durability == UInt32(GRAPHLAVA_DURABILITY_RELAXED) ? .relaxed : .ordered
        let db = self.db
        Task {
            do {
                try await db.submitEncoded(request, durability: d)
                reply(GRAPHLAVA_OK.rawValue, nil)
            } catch let e as GraphLavaError {
                reply(GRAPHLAVA_ERR_INVALID_REQUEST.rawValue, e.description)
            } catch {
                reply(GRAPHLAVA_ERR_INTERNAL.rawValue, "\(error)")
            }
        }
    }
}
