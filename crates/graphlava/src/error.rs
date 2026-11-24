//! The error type of the public API and of every layer below it.
//!
//! The storage variants are the closed set of [[05-storage-trait#Errors]]; the
//! others are data-model and transaction errors from [[01-data-model]] and
//! [[06-transactions-durability]].

use std::fmt;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Every error the database reports.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// OS I/O failure.
    Io(std::io::Error),
    /// Checksum or structure failure in the file.
    Corrupted(String),
    /// Disk or address space exhausted.
    Full,
    /// Unknown format version or feature flag; the file was not modified.
    FormatMismatch(String),
    /// The file is held by another owner process (or another handle).
    Locked,
    /// An update would create a duplicate `(s, p, o)`, or a compare-and-set
    /// found a different current object.
    Conflict(String),
    /// The ID does not name a live item.
    NotFound(u64),
    /// The delete is blocked: another statement has the item in object
    /// position ([[adr:0036]]).
    ReferencedByStatement { item: u64, by: u64 },
    /// The delete is blocked: the item fills a role of a hyperedge
    /// (rule `he-delete-filler`).
    FillsRole { item: u64, hedge: u64 },
    /// A nesting edge would close a cycle (rule `gr-dag-cycle`).
    GraphCycle { child: u64, parent: u64 },
    /// A nesting edge would make a path longer than the depth limit
    /// (rule `gr-dag-depth`).
    GraphDepthExceeded { limit: u32 },
    /// A hyperedge does not match its type's roles (rule `he-type-declared`).
    HyperedgeShapeMismatch(String),
    /// A cascade would delete more than the per-transaction limit.
    CascadeLimitExceeded { limit: u64 },
    /// An interactive write transaction exceeded its time limit and was aborted.
    WriteTimeout,
    /// The database is read-only after a failed batch commit; reopen it.
    ReadOnly,
    /// The request is malformed (bad term position, invalid literal, ...).
    Invalid(String),
    /// The interactive write transaction was aborted by `Database::suspend`
    /// ([[16-api#Mobile bindings]]); none of its writes were committed.
    Aborted(String),
    /// The operation needs handles or snapshots to end first (for example a
    /// suspend that must release the file lock while a snapshot is open).
    Busy(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Corrupted(m) => write!(f, "corrupted database: {m}"),
            Error::Full => write!(f, "storage full"),
            Error::FormatMismatch(m) => write!(f, "incompatible file format: {m}"),
            Error::Locked => write!(f, "database file is locked by another owner"),
            Error::Conflict(m) => write!(f, "conflict: {m}"),
            Error::NotFound(id) => write!(f, "no item with id {id:#x}"),
            Error::ReferencedByStatement { item, by } => {
                write!(
                    f,
                    "{item:#x} is the object of statement {by:#x}; delete with DETACH"
                )
            }
            Error::FillsRole { item, hedge } => {
                write!(
                    f,
                    "{item:#x} fills a role of hyperedge {hedge:#x}; delete with DETACH"
                )
            }
            Error::GraphCycle { child, parent } => {
                write!(
                    f,
                    "nesting {child:#x} under {parent:#x} would close a cycle"
                )
            }
            Error::GraphDepthExceeded { limit } => {
                write!(f, "graph nesting would exceed the depth limit of {limit}")
            }
            Error::HyperedgeShapeMismatch(m) => write!(f, "hyperedge shape mismatch: {m}"),
            Error::CascadeLimitExceeded { limit } => {
                write!(f, "cascade would delete more than {limit} items")
            }
            Error::WriteTimeout => write!(f, "write transaction exceeded its time limit"),
            Error::ReadOnly => write!(f, "database is read-only after a failed commit"),
            Error::Invalid(m) => write!(f, "invalid request: {m}"),
            Error::Aborted(m) => write!(f, "transaction aborted: {m}"),
            Error::Busy(m) => write!(f, "busy: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(msg.into()))
}

pub(crate) fn corrupted(msg: impl Into<String>) -> Error {
    Error::Corrupted(msg.into())
}
