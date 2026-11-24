import Foundation
import GraphLavaFFI

/// Owns a C result handle; frees it (ending its snapshot) when released.
final class RowsHandle: @unchecked Sendable {
    private let lock = NSLock()
    private var handle: OpaquePointer?
    let columns: [String]

    init(_ h: OpaquePointer) {
        handle = h
        var n = 0
        _ = graphlava_rows_columns(h, &n)
        columns = (0..<n).map { i in
            var p: UnsafeMutablePointer<CChar>?
            _ = graphlava_rows_column_name(h, i, &p)
            defer { graphlava_string_free(p) }
            return p.map { String(cString: $0) } ?? ""
        }
    }

    deinit { free() }

    func free() {
        lock.lock()
        defer { lock.unlock() }
        if let h = handle { graphlava_rows_free(h) }
        handle = nil
    }

    /// The next batch, or nil when the result is exhausted.
    func next() throws -> RowBatch? {
        lock.lock()
        defer { lock.unlock() }
        guard let h = handle else { return nil }
        var ids: UnsafePointer<UInt64>?
        var rows = 0, cols = 0
        try check(graphlava_rows_next_batch(h, &ids, &rows, &cols))
        if rows == 0 { return nil }
        let copy = Array(UnsafeBufferPointer(start: ids, count: rows * cols))
        return RowBatch(handle: self, columns: columns, count: rows, ids: copy)
    }

    func decode(_ id: ID) throws -> Term {
        lock.lock()
        defer { lock.unlock() }
        guard let h = handle else { throw GraphLavaError.closed("the row stream has ended") }
        var t = graphlava_term()
        try check(graphlava_term_decode(h, id, &t))
        defer { graphlava_term_free(&t) }
        return Term(c: t)
    }
}

/// At most 1024 rows of IDs in column-major order (rule `api-ffi-rows`); terms
/// are decoded on demand.
public struct RowBatch: @unchecked Sendable {
    let handle: RowsHandle
    public let columns: [String]
    /// Number of rows.
    public let count: Int
    /// Column-major IDs: row `r` of column `c` is `ids[c * count + r]`; 0 is unbound.
    public let ids: [ID]

    public subscript(row: Int, column: Int) -> ID { ids[column * count + row] }

    public func id(row: Int, column name: String) -> ID? {
        columns.firstIndex(of: name).map { self[row, $0] }
    }

    /// Decodes an ID of this result while its snapshot is open.
    public func term(_ id: ID) throws -> Term {
        id == 0 ? .unbound : try handle.decode(id)
    }

    public func term(row: Int, column name: String) throws -> Term {
        guard let c = columns.firstIndex(of: name) else {
            throw GraphLavaError.invalidArgument("no column \(name)")
        }
        return try term(self[row, c])
    }
}

/// The rows of a query as an `AsyncSequence` of batches. The snapshot ends
/// when iteration finishes, when the iterating task is cancelled, or when the
/// sequence is released.
public struct Rows: AsyncSequence, @unchecked Sendable {
    public typealias Element = RowBatch
    let handle: RowsHandle

    init(_ h: RowsHandle) { handle = h }

    public var columns: [String] { handle.columns }

    /// Ends the snapshot now.
    public func close() { handle.free() }

    public struct AsyncIterator: AsyncIteratorProtocol {
        let handle: RowsHandle

        public mutating func next() async throws -> RowBatch? {
            if Task.isCancelled {
                handle.free()
                return nil
            }
            let b: RowBatch?
            do {
                b = try handle.next()
            } catch {
                // An ended snapshot (suspend) or a closed database: release now.
                handle.free()
                throw error
            }
            if b == nil { handle.free() }
            return b
        }
    }

    public func makeAsyncIterator() -> AsyncIterator { AsyncIterator(handle: handle) }

    /// Collects every batch (small results).
    public func all() async throws -> [RowBatch] {
        var out: [RowBatch] = []
        for try await b in self { out.append(b) }
        return out
    }
}
