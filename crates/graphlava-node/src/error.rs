//! Errors on their way to JavaScript ([[16-api#Node.js binding]]).
//!
//! A native call fails with a plain `Error` whose message carries a stable
//! code, the variant's fields and the human message, separated by U+001F.
//! The JS layer (`bindings/node/lib/errors.js`) turns it into a
//! `GraphlavaError` with `code` and `bigint` ID fields. Every Rust
//! [`graphlava::Error`] variant has its own code; a panic becomes `INTERNAL`.

use std::any::Any;

use graphlava::Error;

/// Marks a message produced by this crate, so the JS layer never mistakes an
/// unrelated error for a coded one.
pub(crate) const MARK: &str = "\u{1f}GLV\u{1f}";

/// A field of an error: an ID (`bigint` in JS) or a count (`number`).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Field {
    Id(u64),
    Num(u64),
}

/// A failure with its stable code.
#[derive(Debug)]
pub(crate) struct Fail {
    pub code: &'static str,
    pub msg: String,
    pub fields: Vec<(&'static str, Field)>,
}

impl Fail {
    pub fn new(code: &'static str, msg: impl Into<String>) -> Fail {
        Fail {
            code,
            msg: msg.into(),
            fields: Vec::new(),
        }
    }

    pub fn invalid(msg: impl Into<String>) -> Fail {
        Fail::new("INVALID", msg)
    }

    pub fn closed() -> Fail {
        Fail::new("CLOSED", "the database is closed")
    }

    pub fn panic(payload: Box<dyn Any + Send>) -> Fail {
        let what = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".to_string());
        Fail::new("INTERNAL", format!("internal error (panic): {what}"))
    }

    fn with(mut self, k: &'static str, v: Field) -> Fail {
        self.fields.push((k, v));
        self
    }

    /// The message the JS layer parses.
    pub fn encode(&self) -> String {
        let fields = self
            .fields
            .iter()
            .map(|(k, v)| match v {
                Field::Id(i) => format!("{k}=n{i}"),
                Field::Num(n) => format!("{k}=#{n}"),
            })
            .collect::<Vec<_>>()
            .join(";");
        format!("{MARK}{}\u{1f}{fields}\u{1f}{}", self.code, self.msg)
    }

    pub fn into_napi(self) -> napi::Error {
        napi::Error::new(napi::Status::GenericFailure, self.encode())
    }
}

impl From<Fail> for napi::Error {
    fn from(f: Fail) -> napi::Error {
        f.into_napi()
    }
}

const TIMEOUT_HINT: &str = "write transaction exceeded its time limit (default 100 ms from \
     the moment write() resolved, including time spent awaiting other work); keep \
     interactive transactions short, or send the whole change with db.submit(ops)";

// @tg: implements:: [[openspec:node-binding#Node errors]]
impl From<Error> for Fail {
    fn from(e: Error) -> Fail {
        let msg = e.to_string();
        match e {
            Error::Io(_) => Fail::new("IO", msg),
            Error::Corrupted(_) => Fail::new("CORRUPTED", msg),
            Error::Full => Fail::new("FULL", msg),
            Error::FormatMismatch(_) => Fail::new("FORMAT_MISMATCH", msg),
            Error::Locked => Fail::new("LOCKED", msg),
            Error::Conflict(_) => Fail::new("CONFLICT", msg),
            Error::NotFound(id) => Fail::new("NOT_FOUND", msg).with("id", Field::Id(id)),
            Error::ReferencedByStatement { item, by } => Fail::new("REFERENCED_BY_STATEMENT", msg)
                .with("item", Field::Id(item))
                .with("by", Field::Id(by)),
            Error::FillsRole { item, hedge } => Fail::new("FILLS_ROLE", msg)
                .with("item", Field::Id(item))
                .with("hedge", Field::Id(hedge)),
            Error::GraphCycle { child, parent } => Fail::new("GRAPH_CYCLE", msg)
                .with("child", Field::Id(child))
                .with("parent", Field::Id(parent)),
            Error::GraphDepthExceeded { limit } => {
                Fail::new("GRAPH_DEPTH_EXCEEDED", msg).with("limit", Field::Num(u64::from(limit)))
            }
            Error::HyperedgeShapeMismatch(_) => Fail::new("HYPEREDGE_SHAPE_MISMATCH", msg),
            Error::CascadeLimitExceeded { limit } => {
                Fail::new("CASCADE_LIMIT_EXCEEDED", msg).with("limit", Field::Num(limit))
            }
            Error::WriteTimeout => Fail::new("WRITE_TIMEOUT", TIMEOUT_HINT),
            Error::ReadOnly => Fail::new("READ_ONLY", msg),
            Error::Invalid(_) => Fail::new("INVALID", msg),
            _ => Fail::new("INTERNAL", msg),
        }
    }
}

/// The `WRITE_TIMEOUT` failure of a transaction the writer thread already
/// ended at its deadline.
pub(crate) fn write_timeout() -> Fail {
    Fail::new("WRITE_TIMEOUT", TIMEOUT_HINT)
}

pub(crate) type R<T> = std::result::Result<T, Fail>;
