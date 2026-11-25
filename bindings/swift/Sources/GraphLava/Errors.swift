import Foundation
import GraphLavaFFI

/// Every error of the package. Each case maps one-to-one from a
/// `graphlava_status` code of the C FFI and carries its message
/// ([[16-api#C FFI]]), plus `extensionProcess` for the extension guard.
// @tg: implements:: [[openspec:mobile-bindings#Swift API]]
public enum GraphLavaError: Error, Equatable, CustomStringConvertible {
    case invalidArgument(String)
    case `internal`(String)
    case busy(String)
    case abi(String)
    case closed(String)
    case io(String)
    case corrupted(String)
    case full(String)
    case formatMismatch(String)
    case locked(String)
    case conflict(String)
    case notFound(String)
    case referenced(String)
    case graphConstraint(String)
    case hyperedgeShape(String)
    case cascadeLimit(String)
    case writeTimeout(String)
    case readOnly(String)
    case invalidRequest(String)
    case aborted(String)
    /// `Database.open` was called in an app extension, which must never open
    /// the database file ([[adr:0002]]); use `ExtensionClient`.
    case extensionProcess
    /// A status this version of the package does not know.
    case unknown(status: UInt32, message: String)

    init(status: graphlava_status, message: String) {
        switch status {
        case GRAPHLAVA_ERR_INVALID_ARGUMENT: self = .invalidArgument(message)
        case GRAPHLAVA_ERR_INTERNAL: self = .internal(message)
        case GRAPHLAVA_ERR_BUSY: self = .busy(message)
        case GRAPHLAVA_ERR_ABI: self = .abi(message)
        case GRAPHLAVA_ERR_CLOSED: self = .closed(message)
        case GRAPHLAVA_ERR_IO: self = .io(message)
        case GRAPHLAVA_ERR_CORRUPTED: self = .corrupted(message)
        case GRAPHLAVA_ERR_FULL: self = .full(message)
        case GRAPHLAVA_ERR_FORMAT_MISMATCH: self = .formatMismatch(message)
        case GRAPHLAVA_ERR_LOCKED: self = .locked(message)
        case GRAPHLAVA_ERR_CONFLICT: self = .conflict(message)
        case GRAPHLAVA_ERR_NOT_FOUND: self = .notFound(message)
        case GRAPHLAVA_ERR_REFERENCED: self = .referenced(message)
        case GRAPHLAVA_ERR_GRAPH_CONSTRAINT: self = .graphConstraint(message)
        case GRAPHLAVA_ERR_HYPEREDGE_SHAPE: self = .hyperedgeShape(message)
        case GRAPHLAVA_ERR_CASCADE_LIMIT: self = .cascadeLimit(message)
        case GRAPHLAVA_ERR_WRITE_TIMEOUT: self = .writeTimeout(message)
        case GRAPHLAVA_ERR_READ_ONLY: self = .readOnly(message)
        case GRAPHLAVA_ERR_INVALID_REQUEST: self = .invalidRequest(message)
        case GRAPHLAVA_ERR_ABORTED: self = .aborted(message)
        default: self = .unknown(status: status.rawValue, message: message)
        }
    }

    public var description: String {
        switch self {
        case .extensionProcess:
            return "an app extension must not open the database; use ExtensionClient"
        case let .unknown(status, message):
            return "graphlava status \(status): \(message)"
        case let .invalidArgument(m), let .internal(m), let .busy(m), let .abi(m), let .closed(m),
             let .io(m), let .corrupted(m), let .full(m), let .formatMismatch(m), let .locked(m),
             let .conflict(m), let .notFound(m), let .referenced(m), let .graphConstraint(m),
             let .hyperedgeShape(m), let .cascadeLimit(m), let .writeTimeout(m), let .readOnly(m),
             let .invalidRequest(m), let .aborted(m):
            return m
        }
    }
}

/// Throws the error of a failing FFI call. Must run on the calling thread
/// right after the call: the message is thread-local.
@inline(__always)
func check(_ status: graphlava_status) throws {
    if status == GRAPHLAVA_OK { return }
    throw GraphLavaError(status: status, message: lastErrorMessage())
}

func lastErrorMessage() -> String {
    var p: UnsafeMutablePointer<CChar>?
    _ = graphlava_last_error(&p)
    guard let p else { return "" }
    defer { graphlava_string_free(p) }
    return String(cString: p)
}
