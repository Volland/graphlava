//! C FFI for graphlava ([[16-api#C FFI]]). Builds as `libgraphlava_ffi`
//! (cdylib and staticlib) for desktop, iOS and Android hosts; the C header is
//! `include/graphlava.h`, generated from this file by cbindgen.
//!
//! Conventions (rule `api-ffi-abi`):
//!
//! - Every function except [`graphlava_version`] returns a [`graphlava_status`];
//!   outputs go through out-parameters.
//! - No Rust panic crosses the boundary: each body runs under
//!   [`std::panic::catch_unwind`] and a panic becomes
//!   `GRAPHLAVA_ERR_INTERNAL`.
//! - A failing call stores a message in a thread-local slot that
//!   [`graphlava_last_error`] copies out.
//! - Strings returned to the caller are owned by the caller and freed with
//!   [`graphlava_string_free`]; terms with [`graphlava_term_free`].
//! - Null pointers and non-UTF-8 strings give `GRAPHLAVA_ERR_INVALID_ARGUMENT`.
//!   The `*_free` functions accept null and do nothing.
//! - Handles are opaque tokens looked up in a registry, never dereferenced:
//!   a handle used after it was closed, freed, committed or aborted gives
//!   `GRAPHLAVA_ERR_CLOSED`, and a pointer that was never a handle gives
//!   `GRAPHLAVA_ERR_INVALID_ARGUMENT`, instead of undefined behaviour
//!   (requirement `FFI misuse safety`).
//!
//! Results cross as column-major batches of `u64` IDs plus an on-demand term
//! decoder (rule `api-ffi-rows`).

// @tg: implements:: [[openspec:mobile-bindings#Complete C FFI]]
// @tg: implements:: [[openspec:mobile-bindings#FFI misuse safety]]

// The C names are the ABI; Rust naming conventions do not apply to them.
#![allow(non_camel_case_types)]

pub mod journal;
pub mod request;

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

use graphlava::sparql::IdRows;
use graphlava::{
    CoreOp, Database, Durability, Error, Id, Lang, Literal, Match, OpResult, OpenOptions, ReadTxn,
    Reader, Term, TermRef, WriteRequest, WriteTxn,
};

use request::{Arg, Request};

static VERSION: &CStr = {
    // Keep in sync with the workspace version; checked by the test below.
    match CStr::from_bytes_with_nul(concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes()) {
        Ok(s) => s,
        Err(_) => panic!("version contains a NUL byte"),
    }
};

/// Maximum number of rows in one batch of [`graphlava_rows_next_batch`].
pub const GRAPHLAVA_BATCH_ROWS: usize = 1024;

/// The ABI version of this header. Pass it to `graphlava_open_ex`; a library
/// built for another ABI version refuses with `GRAPHLAVA_ERR_ABI`. It changes
/// whenever a function signature, struct layout or constant changes meaning.
pub const GRAPHLAVA_ABI_VERSION: u32 = 1;

// ---- status codes ---------------------------------------------------------------

/// The result of every FFI call. `GRAPHLAVA_OK` is success; every other value
/// is a failure whose message [`graphlava_last_error`] returns.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum graphlava_status {
    /// Success.
    GRAPHLAVA_OK = 0,
    /// A required pointer was null, a string was not UTF-8, an enum argument
    /// (language, durability, term kind, op code, flag) or index was out of
    /// range, or a pointer is not a handle of the expected kind.
    GRAPHLAVA_ERR_INVALID_ARGUMENT = 1,
    /// A Rust panic was caught at the boundary, or the engine reported an
    /// error this ABI version does not know.
    GRAPHLAVA_ERR_INTERNAL = 2,
    /// `graphlava_close` was called while result, read or write handles of
    /// the database are still alive, or `graphlava_suspend` could not release
    /// the file lock because a snapshot is still open.
    GRAPHLAVA_ERR_BUSY = 3,
    /// `graphlava_open_ex` was called with an ABI version this library does
    /// not implement; no file was created or opened.
    GRAPHLAVA_ERR_ABI = 4,
    /// The handle was already closed, freed, committed or aborted, or (for a
    /// result or read handle) its snapshot was ended by `graphlava_suspend`.
    GRAPHLAVA_ERR_CLOSED = 5,
    /// OS I/O failure.
    GRAPHLAVA_ERR_IO = 10,
    /// Checksum or structure failure in the file.
    GRAPHLAVA_ERR_CORRUPTED = 11,
    /// Disk or address space exhausted.
    GRAPHLAVA_ERR_FULL = 12,
    /// Unknown format version or feature flag; the file was not modified.
    GRAPHLAVA_ERR_FORMAT_MISMATCH = 13,
    /// The file is held by another owner.
    GRAPHLAVA_ERR_LOCKED = 14,
    /// Duplicate statement or failed compare-and-set.
    GRAPHLAVA_ERR_CONFLICT = 15,
    /// The ID does not name a live item.
    GRAPHLAVA_ERR_NOT_FOUND = 16,
    /// A delete is blocked: the item is the object of a statement or fills a
    /// hyperedge role.
    GRAPHLAVA_ERR_REFERENCED = 17,
    /// A graph nesting edge would close a cycle or exceed the depth limit.
    GRAPHLAVA_ERR_GRAPH_CONSTRAINT = 18,
    /// A hyperedge does not match its type's roles.
    GRAPHLAVA_ERR_HYPEREDGE_SHAPE = 19,
    /// A cascade would delete more than the per-transaction limit.
    GRAPHLAVA_ERR_CASCADE_LIMIT = 20,
    /// A write transaction exceeded its time limit.
    GRAPHLAVA_ERR_WRITE_TIMEOUT = 21,
    /// The database is read-only after a failed commit; reopen it.
    GRAPHLAVA_ERR_READ_ONLY = 22,
    /// The request is malformed: a query or update that does not parse, a bad
    /// term position, an invalid literal.
    GRAPHLAVA_ERR_INVALID_REQUEST = 23,
    /// The interactive write transaction was aborted by `graphlava_suspend`;
    /// none of its writes were committed.
    GRAPHLAVA_ERR_ABORTED = 24,
}

use graphlava_status::*;

// ---- argument enums (plain integers so a bad value from C is not UB) ------------

/// Query language for [`graphlava_prepare`].
pub type graphlava_lang = u32;
/// SPARQL 1.2 query.
pub const GRAPHLAVA_LANG_SPARQL: graphlava_lang = 0;
/// Datalog program with one or more `?-` goals.
pub const GRAPHLAVA_LANG_DATALOG: graphlava_lang = 1;

/// Durability level of a write.
pub type graphlava_durability = u32;
/// Durable when the call returns; closes the group-commit batch at once.
pub const GRAPHLAVA_DURABILITY_SYNC: graphlava_durability = 0;
/// Durable when the call returns; shares a group commit.
pub const GRAPHLAVA_DURABILITY_ORDERED: graphlava_durability = 1;
/// Atomic and crash-consistent; may be lost until a later durable commit.
pub const GRAPHLAVA_DURABILITY_RELAXED: graphlava_durability = 2;

/// Kind of a [`graphlava_term`].
pub type graphlava_term_kind = u32;
/// No value: the ID was 0 (an unbound variable). All strings are NULL.
pub const GRAPHLAVA_TERM_UNBOUND: graphlava_term_kind = 0;
/// An IRI; `value` is the IRI.
pub const GRAPHLAVA_TERM_IRI: graphlava_term_kind = 1;
/// A blank node; `value` is its label.
pub const GRAPHLAVA_TERM_BLANK: graphlava_term_kind = 2;
/// A literal; `value` is the lexical form, `datatype` the datatype IRI and
/// `lang` the language tag or NULL.
pub const GRAPHLAVA_TERM_LITERAL: graphlava_term_kind = 3;
/// An RDF 1.2 triple term; `value` is its N-Triples form
/// `<<( s p o )>>`, `datatype` and `lang` are NULL.
pub const GRAPHLAVA_TERM_TRIPLE: graphlava_term_kind = 4;

/// Kind of a [`graphlava_arg`]: the term kinds `GRAPHLAVA_TERM_IRI`,
/// `GRAPHLAVA_TERM_BLANK` and `GRAPHLAVA_TERM_LITERAL`, or one of these.
pub type graphlava_arg_kind = u32;
/// No value: an optional argument left out.
pub const GRAPHLAVA_ARG_NONE: graphlava_arg_kind = 0;
/// A stored item named by its ID (`id`).
pub const GRAPHLAVA_ARG_ID: graphlava_arg_kind = 5;
/// An unsigned integer (`id` holds it): a count, mode or flag set.
pub const GRAPHLAVA_ARG_UINT: graphlava_arg_kind = 6;

/// Flags of [`graphlava_open_ex`].
pub type graphlava_open_flags = u32;
/// The file is in an iOS app-group container: `graphlava_suspend` releases
/// the file lock (rule `api-ios-suspend`). Without it the lock is kept while
/// suspended (the `ios-private-container` exception).
pub const GRAPHLAVA_OPEN_APP_GROUP: graphlava_open_flags = 1;

/// Operation codes of [`graphlava_request_add`]. Arguments in brackets are
/// optional (pass fewer arguments or `GRAPHLAVA_ARG_NONE`); a term is an IRI,
/// blank node, literal or `GRAPHLAVA_ARG_ID`.
pub type graphlava_op = u32;
/// `s, p, o [, graph]`: insert a statement (into `graph`, or the subject's home graph).
pub const GRAPHLAVA_OP_INSERT: graphlava_op = 1;
/// `s, p, o`: set the single object of a functional predicate.
pub const GRAPHLAVA_OP_SET: graphlava_op = 2;
/// `ID, o [, expected]`: update a statement's object in place (compare-and-set with `expected`).
pub const GRAPHLAVA_OP_UPDATE: graphlava_op = 3;
/// `ID, p, o`: annotate a statement.
pub const GRAPHLAVA_OP_ANNOTATE: graphlava_op = 4;
/// `ID [, UINT mode]`: delete; mode 0 cascade, 1 keep annotations, 2 detach.
pub const GRAPHLAVA_OP_DELETE: graphlava_op = 5;
/// `s, p, o [, UINT mode [, graph]]`: remove a triple (in a graph: only that membership).
pub const GRAPHLAVA_OP_REMOVE: graphlava_op = 6;
/// `s, p, o`: the statement ID of an existing triple, promoting it.
pub const GRAPHLAVA_OP_STATEMENT_ID: graphlava_op = 7;
/// `s, p, o`: an unasserted statement (a reifier target).
pub const GRAPHLAVA_OP_INSERT_UNASSERTED: graphlava_op = 8;
/// `p, UINT flags`: declare a predicate; bit 0 multi, bit 1 functional.
pub const GRAPHLAVA_OP_DECLARE_PREDICATE: graphlava_op = 9;
/// `name`: create a named graph (or get the existing one).
pub const GRAPHLAVA_OP_CREATE_GRAPH: graphlava_op = 10;
/// `ID item, graph`: add a membership.
pub const GRAPHLAVA_OP_ADD_MEMBER: graphlava_op = 11;
/// `ID item, graph`: remove a membership.
pub const GRAPHLAVA_OP_REMOVE_MEMBER: graphlava_op = 12;
/// `ID item, from, to`: move a membership.
pub const GRAPHLAVA_OP_MOVE_MEMBER: graphlava_op = 13;
/// `ID item, graph`: the statement ID of a membership, promoting it.
pub const GRAPHLAVA_OP_MEMBERSHIP_ID: graphlava_op = 14;
/// `child, parent`: nest a graph.
pub const GRAPHLAVA_OP_NEST: graphlava_op = 15;
/// `child, parent`: unnest a graph.
pub const GRAPHLAVA_OP_UNNEST: graphlava_op = 16;
/// `graph [, UINT recursive]`: drop a graph.
pub const GRAPHLAVA_OP_DROP_GRAPH: graphlava_op = 17;
/// `ty, UINT n, (role, UINT flags) * n, attr*`: declare a hyperedge type;
/// role flags bit 0 required, bit 1 many.
pub const GRAPHLAVA_OP_DECLARE_HEDGE_TYPE: graphlava_op = 18;
/// `ty, graph|NONE, UINT n, (role, filler) * n, (attr, value)*`: create a hyperedge.
pub const GRAPHLAVA_OP_CREATE_HEDGE: graphlava_op = 19;
/// `ID hedge, role, filler`: replace a role filler.
pub const GRAPHLAVA_OP_UPDATE_ROLE: graphlava_op = 20;
/// `ID hedge, role, node`: the statement ID of a role edge, promoting it.
pub const GRAPHLAVA_OP_ROLE_ID: graphlava_op = 21;

/// Kind of one operation result (see [`graphlava_result_get`]).
pub type graphlava_result_kind = u32;
/// No value.
pub const GRAPHLAVA_RESULT_DONE: graphlava_result_kind = 0;
/// `value` is an ID; 0 for an unpromoted inline property.
pub const GRAPHLAVA_RESULT_ID: graphlava_result_kind = 1;
/// `value` is the number of deleted items; `graphlava_result_deleted` lists them.
pub const GRAPHLAVA_RESULT_DELETED: graphlava_result_kind = 2;
/// `value` is 1 when a triple was removed, else 0.
pub const GRAPHLAVA_RESULT_REMOVED: graphlava_result_kind = 3;
/// `value` is 1 when the operation changed anything, else 0.
pub const GRAPHLAVA_RESULT_CHANGED: graphlava_result_kind = 4;

// ---- C structs ------------------------------------------------------------------

/// A decoded term. Strings are owned by the struct and released with
/// [`graphlava_term_free`]; unused strings are NULL.
#[repr(C)]
#[derive(Debug)]
pub struct graphlava_term {
    /// One of the `GRAPHLAVA_TERM_*` constants.
    pub kind: graphlava_term_kind,
    /// IRI, blank-node label, lexical form or N-Triples triple term.
    pub value: *mut c_char,
    /// Datatype IRI of a literal, else NULL.
    pub datatype: *mut c_char,
    /// Language tag of a literal, else NULL.
    pub lang: *mut c_char,
}

/// An input value: a term, an ID or an unsigned integer. Strings are borrowed
/// for the duration of the call.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct graphlava_arg {
    /// `GRAPHLAVA_TERM_IRI`, `GRAPHLAVA_TERM_BLANK`, `GRAPHLAVA_TERM_LITERAL`,
    /// `GRAPHLAVA_ARG_ID`, `GRAPHLAVA_ARG_UINT` or `GRAPHLAVA_ARG_NONE`.
    pub kind: graphlava_arg_kind,
    /// The ID (`GRAPHLAVA_ARG_ID`) or integer (`GRAPHLAVA_ARG_UINT`).
    pub id: u64,
    /// IRI, blank-node label or lexical form.
    pub value: *const c_char,
    /// Datatype IRI of a literal, or NULL (`xsd:string`, or `rdf:langString`
    /// with a tag).
    pub datatype: *const c_char,
    /// Language tag of a literal, or NULL.
    pub lang: *const c_char,
}

/// What [`graphlava_journal_apply`] did.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct graphlava_journal_report {
    /// Frames applied now.
    pub applied: u64,
    /// Frames already applied by an earlier, interrupted apply.
    pub skipped: u64,
    /// Frames whose request failed; recorded so they are not retried.
    pub failed: u64,
    /// Bytes of a torn tail that were dropped; non-zero means the caller
    /// should log one warning.
    pub dropped_bytes: u64,
}

// ---- opaque handle types (never dereferenced; see the registry) -----------------

/// An open database (the owner handle). It may be shared between threads, but
/// `graphlava_close` must not run concurrently with any other call on it.
#[derive(Debug)]
pub struct graphlava_db {
    _opaque: [u8; 0],
}

/// A prepared query: a language and its text. Independent of any database;
/// re-executing it hits the engine's plan cache.
#[derive(Debug)]
pub struct graphlava_prepared {
    _opaque: [u8; 0],
}

/// A query result: the snapshot it was read from, its ID rows and a cursor.
#[derive(Debug)]
pub struct graphlava_rows {
    _opaque: [u8; 0],
}

/// A read snapshot for core reads (`graphlava_find`, `graphlava_lookup`, ...).
#[derive(Debug)]
pub struct graphlava_read {
    _opaque: [u8; 0],
}

/// An interactive write transaction, in process only. It holds the writer
/// for at most the write time limit (100 ms by default).
#[derive(Debug)]
pub struct graphlava_write {
    _opaque: [u8; 0],
}

/// A write request under construction: operations, or a SPARQL Update or
/// Datalog program text.
#[derive(Debug)]
pub struct graphlava_request {
    _opaque: [u8; 0],
}

/// The per-operation results of a submitted request.
#[derive(Debug)]
pub struct graphlava_result {
    _opaque: [u8; 0],
}

// ---- handle state ---------------------------------------------------------------

/// A snapshot shared by a result or read handle and the database's list of
/// snapshots that `graphlava_suspend` ends. `None` once ended.
type SnapSlot = Mutex<Option<ReadTxn<'static>>>;

struct DbH {
    db: Database,
    flags: u32,
    /// Set from the start of `graphlava_suspend` until `graphlava_resume`; a
    /// snapshot registered meanwhile is ended at once.
    suspending: AtomicBool,
    snapshots: Mutex<Vec<Weak<SnapSlot>>>,
}

impl DbH {
    /// Registers a snapshot so suspend can end it.
    fn register(&self, slot: &Arc<SnapSlot>) {
        let mut v = lock(&self.snapshots);
        v.retain(|w| w.strong_count() > 0);
        v.push(Arc::downgrade(slot));
        drop(v);
        if self.suspending.load(Ordering::SeqCst) {
            lock(slot).take();
        }
    }

    fn end_snapshots(&self) {
        let live: Vec<Arc<SnapSlot>> = lock(&self.snapshots)
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for s in live {
            lock(&s).take();
        }
    }
}

struct RowsState {
    rows: IdRows,
    cursor: usize,
    batch: Vec<u64>,
}

struct RowsH {
    // Field order matters: the snapshot drops before the database handle.
    snap: Arc<SnapSlot>,
    state: Mutex<RowsState>,
    _db: Arc<DbH>,
}

struct ReadH {
    snap: Arc<SnapSlot>,
    _db: Arc<DbH>,
}

struct WriteH {
    txn: Mutex<Option<WriteTxn<'static>>>,
    _db: Arc<DbH>,
}

struct PreparedH {
    lang: Lang,
    text: String,
}

struct ResultH {
    results: Vec<OpResult>,
}

#[derive(Clone)]
enum Slot {
    Db(Arc<DbH>),
    Prepared(Arc<PreparedH>),
    Rows(Arc<RowsH>),
    Read(Arc<ReadH>),
    Write(Arc<WriteH>),
    Request(Arc<Mutex<Request>>),
    Result(Arc<ResultH>),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ---- handle registry ------------------------------------------------------------

/// Handles are tokens `BASE + n * STEP`, issued once and never reused.
const TOKEN_BASE: usize = 0x1000;
const TOKEN_STEP: usize = 16;
static NEXT_TOKEN: AtomicUsize = AtomicUsize::new(0);

fn handles() -> MutexGuard<'static, HashMap<usize, Slot>> {
    static H: OnceLock<Mutex<HashMap<usize, Slot>>> = OnceLock::new();
    lock(H.get_or_init(|| Mutex::new(HashMap::new())))
}

fn insert_handle<T>(slot: Slot) -> *mut T {
    let n = NEXT_TOKEN.fetch_add(1, Ordering::SeqCst);
    let tok = TOKEN_BASE + n * TOKEN_STEP;
    handles().insert(tok, slot);
    tok as *mut T
}

fn issued(tok: usize) -> bool {
    tok >= TOKEN_BASE
        && (tok - TOKEN_BASE).is_multiple_of(TOKEN_STEP)
        && (tok - TOKEN_BASE) / TOKEN_STEP < NEXT_TOKEN.load(Ordering::SeqCst)
}

fn missing<T>(tok: usize, name: &str) -> FfiResult<T> {
    if tok == 0 {
        fail(GRAPHLAVA_ERR_INVALID_ARGUMENT, format!("`{name}` is NULL"))
    } else if issued(tok) {
        fail(
            GRAPHLAVA_ERR_CLOSED,
            format!("`{name}` was already closed, freed, committed or aborted"),
        )
    } else {
        fail(
            GRAPHLAVA_ERR_INVALID_ARGUMENT,
            format!("`{name}` is not a graphlava handle"),
        )
    }
}

fn wrong<T>(name: &str, kind: &str) -> FfiResult<T> {
    fail(
        GRAPHLAVA_ERR_INVALID_ARGUMENT,
        format!("`{name}` is not a {kind} handle"),
    )
}

fn get_slot(tok: usize, name: &str) -> FfiResult<Slot> {
    match handles().get(&tok) {
        Some(s) => Ok(s.clone()),
        None => missing(tok, name),
    }
}

fn take_slot(tok: usize, name: &str) -> FfiResult<Slot> {
    match handles().remove(&tok) {
        Some(s) => Ok(s),
        None => missing(tok, name),
    }
}

macro_rules! getter {
    ($get:ident, $take:ident, $variant:ident, $ty:ty, $cty:ty, $kind:literal) => {
        fn $get(p: *const $cty, name: &str) -> FfiResult<Arc<$ty>> {
            match get_slot(p as usize, name)? {
                Slot::$variant(h) => Ok(h),
                _ => wrong(name, $kind),
            }
        }
        #[allow(dead_code)]
        fn $take(p: *const $cty, name: &str) -> FfiResult<Arc<$ty>> {
            let tok = p as usize;
            match take_slot(tok, name)? {
                Slot::$variant(h) => Ok(h),
                other => {
                    handles().insert(tok, other);
                    wrong(name, $kind)
                }
            }
        }
    };
}

getter!(get_db, take_db, Db, DbH, graphlava_db, "database");
getter!(
    get_prepared,
    take_prepared,
    Prepared,
    PreparedH,
    graphlava_prepared,
    "prepared query"
);
getter!(get_rows, take_rows, Rows, RowsH, graphlava_rows, "result");
getter!(get_read, take_read, Read, ReadH, graphlava_read, "read");
getter!(
    get_write,
    take_write,
    Write,
    WriteH,
    graphlava_write,
    "write transaction"
);
getter!(
    get_request,
    take_request,
    Request,
    Mutex<Request>,
    graphlava_request,
    "request"
);
getter!(
    get_result,
    take_result,
    Result,
    ResultH,
    graphlava_result,
    "result"
);

/// Frees any handle kind; unknown and NULL pointers are ignored.
fn free_handle(tok: usize) {
    if tok != 0 {
        let slot = handles().remove(&tok);
        drop(slot);
    }
}

// ---- error plumbing -------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// A failure on its way to C: a status and a message.
struct Fail {
    status: graphlava_status,
    msg: String,
}

type FfiResult<T = ()> = std::result::Result<T, Fail>;

fn fail<T>(status: graphlava_status, msg: impl Into<String>) -> FfiResult<T> {
    Err(Fail {
        status,
        msg: msg.into(),
    })
}

fn status_of(e: &Error) -> graphlava_status {
    match e {
        Error::Io(_) => GRAPHLAVA_ERR_IO,
        Error::Corrupted(_) => GRAPHLAVA_ERR_CORRUPTED,
        Error::Full => GRAPHLAVA_ERR_FULL,
        Error::FormatMismatch(_) => GRAPHLAVA_ERR_FORMAT_MISMATCH,
        Error::Locked => GRAPHLAVA_ERR_LOCKED,
        Error::Conflict(_) => GRAPHLAVA_ERR_CONFLICT,
        Error::NotFound(_) => GRAPHLAVA_ERR_NOT_FOUND,
        Error::ReferencedByStatement { .. } | Error::FillsRole { .. } => GRAPHLAVA_ERR_REFERENCED,
        Error::GraphCycle { .. } | Error::GraphDepthExceeded { .. } => {
            GRAPHLAVA_ERR_GRAPH_CONSTRAINT
        }
        Error::HyperedgeShapeMismatch(_) => GRAPHLAVA_ERR_HYPEREDGE_SHAPE,
        Error::CascadeLimitExceeded { .. } => GRAPHLAVA_ERR_CASCADE_LIMIT,
        Error::WriteTimeout => GRAPHLAVA_ERR_WRITE_TIMEOUT,
        Error::ReadOnly => GRAPHLAVA_ERR_READ_ONLY,
        Error::Invalid(_) => GRAPHLAVA_ERR_INVALID_REQUEST,
        Error::Aborted(_) => GRAPHLAVA_ERR_ABORTED,
        Error::Busy(_) => GRAPHLAVA_ERR_BUSY,
        _ => GRAPHLAVA_ERR_INTERNAL,
    }
}

impl From<Error> for Fail {
    fn from(e: Error) -> Fail {
        Fail {
            status: status_of(&e),
            msg: e.to_string(),
        }
    }
}

fn set_last_error(msg: &str) {
    let c = CString::new(msg.replace('\0', "\u{FFFD}")).unwrap_or_default();
    LAST_ERROR.with(|s| *s.borrow_mut() = Some(c));
}

/// Runs an FFI body: catches panics, records the error message, returns the status.
fn guard(f: impl FnOnce() -> FfiResult) -> graphlava_status {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => GRAPHLAVA_OK,
        Ok(Err(e)) => {
            set_last_error(&e.msg);
            e.status
        }
        Err(payload) => {
            let what = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic payload".to_string());
            set_last_error(&format!("internal error (panic): {what}"));
            GRAPHLAVA_ERR_INTERNAL
        }
    }
}

// ---- argument helpers -----------------------------------------------------------

/// Borrows a required NUL-terminated UTF-8 string.
///
/// # Safety
/// `p` is null or points to a NUL-terminated string valid for `'a`.
unsafe fn arg_str<'a>(p: *const c_char, name: &str) -> FfiResult<&'a str> {
    if p.is_null() {
        return fail(GRAPHLAVA_ERR_INVALID_ARGUMENT, format!("`{name}` is NULL"));
    }
    // SAFETY: non-null and NUL-terminated per the caller's contract.
    let c = unsafe { CStr::from_ptr(p) };
    c.to_str().or_else(|_| {
        fail(
            GRAPHLAVA_ERR_INVALID_ARGUMENT,
            format!("`{name}` is not valid UTF-8"),
        )
    })
}

/// Borrows an optional string: NULL gives `None`.
///
/// # Safety
/// As [`arg_str`].
unsafe fn opt_str<'a>(p: *const c_char, name: &str) -> FfiResult<Option<&'a str>> {
    if p.is_null() {
        Ok(None)
    } else {
        // SAFETY: forwarded caller contract.
        unsafe { arg_str(p, name) }.map(Some)
    }
}

/// Checks that an out-parameter is non-null.
fn out_ptr<T>(p: *mut T, name: &str) -> FfiResult<*mut T> {
    if p.is_null() {
        fail(GRAPHLAVA_ERR_INVALID_ARGUMENT, format!("`{name}` is NULL"))
    } else {
        Ok(p)
    }
}

/// Writes `v` through an out-parameter that may be NULL.
///
/// # Safety
/// `p` is null or valid for one write of `T`.
unsafe fn put_opt<T>(p: *mut T, v: T) {
    if !p.is_null() {
        // SAFETY: per the contract.
        unsafe { p.write(v) };
    }
}

fn durability(d: graphlava_durability) -> FfiResult<Durability> {
    match d {
        GRAPHLAVA_DURABILITY_SYNC => Ok(Durability::Sync),
        GRAPHLAVA_DURABILITY_ORDERED => Ok(Durability::Ordered),
        GRAPHLAVA_DURABILITY_RELAXED => Ok(Durability::Relaxed),
        _ => fail(
            GRAPHLAVA_ERR_INVALID_ARGUMENT,
            format!("unknown durability {d}"),
        ),
    }
}

fn lang_of(lang: graphlava_lang) -> FfiResult<Lang> {
    match lang {
        GRAPHLAVA_LANG_SPARQL => Ok(Lang::Sparql),
        GRAPHLAVA_LANG_DATALOG => Ok(Lang::Datalog),
        l => fail(
            GRAPHLAVA_ERR_INVALID_ARGUMENT,
            format!("unknown language {l}"),
        ),
    }
}

/// Reads one C argument into an owned [`Arg`].
///
/// # Safety
/// The strings of `a` are null or NUL-terminated.
unsafe fn read_arg(a: &graphlava_arg, i: usize) -> FfiResult<Arg> {
    let name = format!("args[{i}].value");
    // SAFETY (all string reads): per the contract.
    Ok(match a.kind {
        GRAPHLAVA_ARG_NONE => Arg::None,
        GRAPHLAVA_TERM_IRI => Arg::Iri(unsafe { arg_str(a.value, &name) }?.to_string()),
        GRAPHLAVA_TERM_BLANK => Arg::Blank(unsafe { arg_str(a.value, &name) }?.to_string()),
        GRAPHLAVA_TERM_LITERAL => Arg::Literal {
            lexical: unsafe { arg_str(a.value, &name) }?.to_string(),
            datatype: unsafe { opt_str(a.datatype, "datatype") }?.map(str::to_string),
            lang: unsafe { opt_str(a.lang, "lang") }?.map(str::to_string),
        },
        GRAPHLAVA_ARG_ID => Arg::Id(a.id),
        GRAPHLAVA_ARG_UINT => Arg::Uint(a.id),
        k => {
            return fail(
                GRAPHLAVA_ERR_INVALID_ARGUMENT,
                format!("args[{i}]: unknown argument kind {k}"),
            );
        }
    })
}

/// Reads a C argument that must be a term (IRI, blank, literal).
///
/// # Safety
/// `a` is null or a valid `graphlava_arg` whose strings are null or
/// NUL-terminated.
unsafe fn read_term(a: *const graphlava_arg, name: &str) -> FfiResult<Term> {
    // SAFETY: per the contract.
    let Some(a) = (unsafe { a.as_ref() }) else {
        return fail(GRAPHLAVA_ERR_INVALID_ARGUMENT, format!("`{name}` is NULL"));
    };
    // SAFETY: per the contract.
    match unsafe { read_arg(a, 0) }? {
        Arg::Iri(i) => Ok(Term::iri(i)),
        Arg::Blank(b) => Ok(Term::Blank(b)),
        Arg::Literal {
            lexical,
            datatype,
            lang,
        } => Ok(Term::lit(match (datatype, lang) {
            (_, Some(tag)) => Literal::lang(lexical, tag),
            (Some(dt), None) => Literal::typed(lexical, dt),
            (None, None) => Literal::string(lexical),
        })),
        _ => fail(
            GRAPHLAVA_ERR_INVALID_ARGUMENT,
            format!("`{name}` must be an IRI, blank node or literal"),
        ),
    }
}

fn to_cstring(s: &str, what: &str) -> FfiResult<CString> {
    CString::new(s).or_else(|_| {
        fail(
            GRAPHLAVA_ERR_INVALID_REQUEST,
            format!("{what} contains a NUL byte and cannot cross the C API"),
        )
    })
}

fn into_raw(s: &str, what: &str) -> FfiResult<*mut c_char> {
    to_cstring(s, what).map(CString::into_raw)
}

fn empty_term() -> graphlava_term {
    graphlava_term {
        kind: GRAPHLAVA_TERM_UNBOUND,
        value: std::ptr::null_mut(),
        datatype: std::ptr::null_mut(),
        lang: std::ptr::null_mut(),
    }
}

/// Converts a term to its C form; all strings are built before any is
/// released to C, so a failure leaks nothing.
fn c_term(term: &Term) -> FfiResult<graphlava_term> {
    let (kind, value, datatype, lang) = match term {
        Term::Iri(i) => (GRAPHLAVA_TERM_IRI, to_cstring(i, "IRI")?, None, None),
        Term::Blank(b) => (
            GRAPHLAVA_TERM_BLANK,
            to_cstring(b, "blank label")?,
            None,
            None,
        ),
        Term::Literal(l) => (
            GRAPHLAVA_TERM_LITERAL,
            to_cstring(&l.lexical, "literal")?,
            Some(to_cstring(&l.datatype, "datatype")?),
            match &l.lang {
                Some(t) => Some(to_cstring(t, "language tag")?),
                None => None,
            },
        ),
        Term::Triple(_) => (
            GRAPHLAVA_TERM_TRIPLE,
            to_cstring(&term.to_string(), "triple term")?,
            None,
            None,
        ),
        // A term kind this ABI version does not know yet.
        _ => {
            return fail(
                GRAPHLAVA_ERR_INVALID_REQUEST,
                format!("term {term} has no C representation in this ABI version"),
            );
        }
    };
    Ok(graphlava_term {
        kind,
        value: value.into_raw(),
        datatype: datatype.map_or(std::ptr::null_mut(), CString::into_raw),
        lang: lang.map_or(std::ptr::null_mut(), CString::into_raw),
    })
}

/// Copies `src` into the caller's buffer of `cap` elements and stores the
/// full length in `*out_n`. When `cap < src.len()` only `cap` elements are
/// copied; call again with a buffer of `*out_n` elements.
///
/// # Safety
/// `buf` is null (then `cap` must be 0) or valid for `cap` writes; `out_n`
/// is valid for one write.
unsafe fn fill<T: Copy>(src: &[T], buf: *mut T, cap: usize, out_n: *mut usize) -> FfiResult {
    let out_n = out_ptr(out_n, "out_n")?;
    if buf.is_null() && cap != 0 {
        return fail(
            GRAPHLAVA_ERR_INVALID_ARGUMENT,
            "`buf` is NULL but `cap` is not 0",
        );
    }
    let n = src.len().min(cap);
    if n > 0 {
        // SAFETY: `buf` holds at least `cap >= n` elements per the contract.
        unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buf, n) };
    }
    // SAFETY: non-null per the check above.
    unsafe { *out_n = src.len() };
    Ok(())
}

fn quads(ms: &[Match]) -> Vec<u64> {
    let mut v = Vec::with_capacity(ms.len() * 4);
    for m in ms {
        v.extend_from_slice(&[m.s.0, m.p.0, m.o.0, m.id.map_or(0, |i| i.0)]);
    }
    v
}

fn opt_id(v: u64) -> Option<Id> {
    (v != 0).then_some(Id(v))
}

/// Runs `f` on the live snapshot of a read handle.
fn with_read<R>(
    read: *const graphlava_read,
    f: impl FnOnce(&ReadTxn<'static>) -> graphlava::Result<R>,
) -> FfiResult<R> {
    let h = get_read(read, "read")?;
    let g = lock(&h.snap);
    match g.as_ref() {
        Some(txn) => Ok(f(txn)?),
        None => snapshot_ended(),
    }
}

fn snapshot_ended<T>() -> FfiResult<T> {
    fail(
        GRAPHLAVA_ERR_CLOSED,
        "the snapshot of this handle was ended by graphlava_suspend; start a new one",
    )
}

// ---- version and errors ---------------------------------------------------------

/// Returns the library version as a static, NUL-terminated string.
/// The caller must not free it.
#[unsafe(no_mangle)]
pub extern "C" fn graphlava_version() -> *const c_char {
    VERSION.as_ptr()
}

/// Stores the ABI version this library implements (`GRAPHLAVA_ABI_VERSION`
/// of the header it was built with) in `*out`.
///
/// # Safety
/// `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_abi_version(out: *mut u32) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = GRAPHLAVA_ABI_VERSION };
        Ok(())
    })
}

/// Copies the message of the most recent failing call on this thread into
/// `*out` (free with `graphlava_string_free`), or sets `*out` to NULL when no
/// call on this thread has failed. Successful calls do not clear the message.
///
/// # Safety
/// `out` is null or valid for one pointer write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_last_error(out: *mut *mut c_char) -> graphlava_status {
    if out.is_null() {
        return GRAPHLAVA_ERR_INVALID_ARGUMENT;
    }
    let copy = LAST_ERROR.with(|s| s.borrow().clone());
    // SAFETY: `out` is non-null and writable per the contract.
    unsafe { *out = copy.map_or(std::ptr::null_mut(), CString::into_raw) };
    GRAPHLAVA_OK
}

/// Frees a string returned by this library. NULL is a no-op.
///
/// # Safety
/// `s` is null or a string returned by this library and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: `s` came from `CString::into_raw` in this library.
        drop(unsafe { CString::from_raw(s) });
    }
}

// ---- lifecycle ------------------------------------------------------------------

/// Opens (or creates) the database file at `path` with default options and
/// takes the owner lock. On success `*out` is the handle; on failure NULL.
/// Equivalent to `graphlava_open_ex(GRAPHLAVA_ABI_VERSION, path, 0, out)`.
///
/// # Safety
/// `path` is null or a NUL-terminated string; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_open(
    path: *const c_char,
    out: *mut *mut graphlava_db,
) -> graphlava_status {
    // SAFETY: forwarded contract.
    unsafe { graphlava_open_ex(GRAPHLAVA_ABI_VERSION, path, 0, out) }
}

/// Opens (or creates) the database file at `path` after checking that the
/// caller was built against this library's ABI: `abi_version` must equal
/// `GRAPHLAVA_ABI_VERSION`, else the call fails with `GRAPHLAVA_ERR_ABI`
/// before touching the file system. `flags` is 0 or
/// `GRAPHLAVA_OPEN_APP_GROUP`.
///
/// # Safety
/// `path` is null or a NUL-terminated string; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_open_ex(
    abi_version: u32,
    path: *const c_char,
    flags: graphlava_open_flags,
    out: *mut *mut graphlava_db,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: `out` is non-null and writable per the contract.
        unsafe { *out = std::ptr::null_mut() };
        if abi_version != GRAPHLAVA_ABI_VERSION {
            return fail(
                GRAPHLAVA_ERR_ABI,
                format!(
                    "caller built for ABI version {abi_version}, this library implements {GRAPHLAVA_ABI_VERSION}"
                ),
            );
        }
        if flags & !GRAPHLAVA_OPEN_APP_GROUP != 0 {
            return fail(
                GRAPHLAVA_ERR_INVALID_ARGUMENT,
                format!("unknown open flags {flags:#x}"),
            );
        }
        // SAFETY: per the contract.
        let path = unsafe { arg_str(path, "path") }?;
        let db = Database::open(path, OpenOptions::default())?;
        let h = Arc::new(DbH {
            db,
            flags,
            suspending: AtomicBool::new(false),
            snapshots: Mutex::new(Vec::new()),
        });
        // SAFETY: as above.
        unsafe { *out = insert_handle(Slot::Db(h)) };
        Ok(())
    })
}

/// Commits any open batch, closes the file and frees the handle.
///
/// Fails with `GRAPHLAVA_ERR_BUSY` (and keeps the handle valid) while any
/// result, read or write handle of this database is alive. On any other
/// status the handle is freed; using it again gives `GRAPHLAVA_ERR_CLOSED`.
///
/// # Safety
/// `db` is null or a handle from `graphlava_open`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_close(db: *mut graphlava_db) -> graphlava_status {
    guard(|| {
        let tok = db as usize;
        let h = take_db(db, "db")?;
        match Arc::try_unwrap(h) {
            Ok(h) => {
                h.db.close()?;
                Ok(())
            }
            Err(h) => {
                let live = Arc::strong_count(&h) - 1;
                handles().insert(tok, Slot::Db(h));
                fail(
                    GRAPHLAVA_ERR_BUSY,
                    format!(
                        "{live} result handle(s) still alive; free them with graphlava_rows_free (and end read and write handles) before graphlava_close"
                    ),
                )
            }
        }
    })
}

/// Suspends the database for an app moving to the background
/// ([[16-api#Mobile bindings]], rule `api-ios-suspend`): ends the snapshots of
/// every result and read handle (their next call gives `GRAPHLAVA_ERR_CLOSED`),
/// aborts the open interactive write transaction (`GRAPHLAVA_ERR_ABORTED`),
/// commits the open batch with a flush and, for a database opened with
/// `GRAPHLAVA_OPEN_APP_GROUP`, releases the file lock. Calls made while
/// suspended wait until `graphlava_resume`. Suspending twice is a no-op.
///
/// # Safety
/// `db` is null or a database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_suspend(db: *mut graphlava_db) -> graphlava_status {
    guard(|| {
        let h = get_db(db, "db")?;
        h.suspending.store(true, Ordering::SeqCst);
        h.end_snapshots();
        h.db.suspend(h.flags & GRAPHLAVA_OPEN_APP_GROUP != 0)?;
        Ok(())
    })
}

/// Ends a suspension: reacquires the file lock if suspend released it, and
/// lets waiting calls run on fresh snapshots. Fails with
/// `GRAPHLAVA_ERR_LOCKED` (still suspended) when another owner took the file.
/// A no-op when not suspended.
///
/// # Safety
/// `db` is null or a database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_resume(db: *mut graphlava_db) -> graphlava_status {
    guard(|| {
        let h = get_db(db, "db")?;
        h.db.resume()?;
        h.suspending.store(false, Ordering::SeqCst);
        Ok(())
    })
}

/// Commits the open group-commit batch with a flush now, so every
/// acknowledged commit, `relaxed` included, survives a process kill (Android
/// `ON_STOP`). Does not suspend and does not touch an open write transaction.
///
/// # Safety
/// `db` is null or a database handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_flush(db: *mut graphlava_db) -> graphlava_status {
    guard(|| {
        get_db(db, "db")?.db.flush()?;
        Ok(())
    })
}

/// Stores 1 in `*out` while the database is suspended, else 0.
///
/// # Safety
/// `db` is null or a database handle; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_is_suspended(
    db: *const graphlava_db,
    out: *mut u32,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        let h = get_db(db, "db")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = u32::from(h.db.is_suspended()) };
        Ok(())
    })
}

/// Stores the number of open read snapshots of the database in `*out`
/// (diagnostics: a cancelled result stream must leave 0).
///
/// # Safety
/// `db` is null or a database handle; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_open_snapshots(
    db: *const graphlava_db,
    out: *mut usize,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        let h = get_db(db, "db")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = h.db.open_snapshots() };
        Ok(())
    })
}

// ---- writes ---------------------------------------------------------------------

/// Runs a SPARQL 1.2 Update as one write request at the given durability.
///
/// # Safety
/// `db` is null or a live handle; `text` is null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_sparql_update(
    db: *const graphlava_db,
    text: *const c_char,
    durability_level: graphlava_durability,
) -> graphlava_status {
    guard(|| {
        let h = get_db(db, "db")?;
        // SAFETY: per the contract.
        let text = unsafe { arg_str(text, "text") }?;
        h.db.sparql_update(text, durability(durability_level)?)?;
        Ok(())
    })
}

/// Runs a Datalog program in one write request: its `@assert` heads are
/// written, all or nothing.
///
/// # Safety
/// As [`graphlava_sparql_update`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_datalog_update(
    db: *const graphlava_db,
    text: *const c_char,
    durability_level: graphlava_durability,
) -> graphlava_status {
    guard(|| {
        let h = get_db(db, "db")?;
        // SAFETY: per the contract.
        let text = unsafe { arg_str(text, "text") }?;
        h.db.datalog_update(text, durability(durability_level)?)?;
        Ok(())
    })
}

/// Inserts one statement `(s_iri, p_iri, o)` through the core API.
///
/// The object is described by `o_kind`: `GRAPHLAVA_TERM_IRI` (`o_value` is the
/// IRI), `GRAPHLAVA_TERM_BLANK` (`o_value` is a request-scoped label) or
/// `GRAPHLAVA_TERM_LITERAL` (`o_value` is the lexical form; `o_datatype` the
/// datatype IRI or NULL; `o_lang` a language tag or NULL). A literal with
/// neither datatype nor tag is an `xsd:string`.
///
/// `*out_id` (optional, may be NULL) receives the statement ID, or 0 when the
/// statement was stored as an unpromoted inline property.
///
/// # Safety
/// `db` is null or a live handle; strings are null or NUL-terminated;
/// `out_id` is null or writable.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn graphlava_insert(
    db: *const graphlava_db,
    s_iri: *const c_char,
    p_iri: *const c_char,
    o_kind: graphlava_term_kind,
    o_value: *const c_char,
    o_datatype: *const c_char,
    o_lang: *const c_char,
    durability_level: graphlava_durability,
    out_id: *mut u64,
) -> graphlava_status {
    guard(|| {
        let h = get_db(db, "db")?;
        // SAFETY (all argument reads below): per the contract.
        let s = unsafe { arg_str(s_iri, "s_iri") }?;
        let p = unsafe { arg_str(p_iri, "p_iri") }?;
        let v = unsafe { arg_str(o_value, "o_value") }?;
        let dt = unsafe { opt_str(o_datatype, "o_datatype") }?;
        let lang = unsafe { opt_str(o_lang, "o_lang") }?;
        let o = match o_kind {
            GRAPHLAVA_TERM_IRI => Term::iri(v),
            GRAPHLAVA_TERM_BLANK => Term::Blank(v.to_string()),
            GRAPHLAVA_TERM_LITERAL => Term::lit(match (dt, lang) {
                (_, Some(tag)) => Literal::lang(v, tag),
                (Some(dt), None) => Literal::typed(v, dt),
                (None, None) => Literal::string(v),
            }),
            k => {
                return fail(
                    GRAPHLAVA_ERR_INVALID_ARGUMENT,
                    format!("object kind {k} is not IRI, BLANK or LITERAL"),
                );
            }
        };
        let req = WriteRequest {
            ops: vec![CoreOp::Insert {
                s: TermRef::Term(Term::iri(s)),
                p: TermRef::Term(Term::iri(p)),
                o: TermRef::Term(o),
                graph: None,
            }],
        };
        let ticket = h.db.submit(req, durability(durability_level)?)?;
        let id = match ticket.results.first() {
            Some(OpResult::Id(id)) => id.map_or(0, |i| i.0),
            _ => 0,
        };
        // SAFETY: null or writable per the contract.
        unsafe { put_opt(out_id, id) };
        Ok(())
    })
}

// ---- write requests -------------------------------------------------------------

/// Creates an empty operation request; add operations with
/// `graphlava_request_add`. Free with `graphlava_request_free`.
///
/// # Safety
/// `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_request_new(
    out: *mut *mut graphlava_request,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        let h = insert_handle(Slot::Request(Arc::new(Mutex::new(
            Request::Ops(Vec::new()),
        ))));
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = h };
        Ok(())
    })
}

/// Creates a request from text: a SPARQL Update (`GRAPHLAVA_LANG_SPARQL`) or
/// a Datalog program whose `@assert` heads write (`GRAPHLAVA_LANG_DATALOG`).
///
/// # Safety
/// `text` is null or NUL-terminated; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_request_new_text(
    lang: graphlava_lang,
    text: *const c_char,
    out: *mut *mut graphlava_request,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = std::ptr::null_mut() };
        let lang = lang_of(lang)?;
        // SAFETY: per the contract.
        let text = unsafe { arg_str(text, "text") }?.to_string();
        let r = match lang {
            Lang::Sparql => Request::Sparql(text),
            Lang::Datalog => Request::Datalog(text),
        };
        // SAFETY: as above.
        unsafe { *out = insert_handle(Slot::Request(Arc::new(Mutex::new(r)))) };
        Ok(())
    })
}

/// Appends one operation (`GRAPHLAVA_OP_*`) with `nargs` arguments to an
/// operation request. The arity and argument kinds are checked now; a bad
/// operation gives `GRAPHLAVA_ERR_INVALID_ARGUMENT` and leaves the request
/// unchanged.
///
/// # Safety
/// `req` is null or a request handle; `args` is null (with `nargs == 0`) or
/// points to `nargs` arguments whose strings are null or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_request_add(
    req: *mut graphlava_request,
    op: graphlava_op,
    args: *const graphlava_arg,
    nargs: usize,
) -> graphlava_status {
    guard(|| {
        let h = get_request(req, "req")?;
        if args.is_null() && nargs != 0 {
            return fail(GRAPHLAVA_ERR_INVALID_ARGUMENT, "`args` is NULL");
        }
        let raw: &[graphlava_arg] = if nargs == 0 {
            &[]
        } else {
            // SAFETY: `args` points to `nargs` elements per the contract.
            unsafe { std::slice::from_raw_parts(args, nargs) }
        };
        let mut owned = Vec::with_capacity(nargs);
        for (i, a) in raw.iter().enumerate() {
            // SAFETY: per the contract.
            owned.push(unsafe { read_arg(a, i) }?);
        }
        if let Err(e) = request::to_core(op, &owned) {
            return fail(GRAPHLAVA_ERR_INVALID_ARGUMENT, e.to_string());
        }
        match &mut *lock(&h) {
            Request::Ops(ops) => ops.push((op, owned)),
            _ => {
                return fail(
                    GRAPHLAVA_ERR_INVALID_ARGUMENT,
                    "operations cannot be added to a text request",
                );
            }
        }
        Ok(())
    })
}

/// Encodes a request as bytes (the payload of an extension journal frame)
/// into `buf` of `cap` bytes; `*out_len` receives the full length. When
/// `cap` is too small nothing is written; call again with `*out_len` bytes.
///
/// # Safety
/// `req` is null or a request handle; `buf` is null (with `cap == 0`) or
/// valid for `cap` bytes; `out_len` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_request_encode(
    req: *const graphlava_request,
    buf: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> graphlava_status {
    guard(|| {
        out_ptr(out_len, "out_len")?;
        let h = get_request(req, "req")?;
        let bytes = lock(&h).encode();
        let cap = if cap < bytes.len() { 0 } else { cap };
        // SAFETY: per the contract.
        unsafe { fill(&bytes, buf, cap, out_len) }
    })
}

/// Decodes bytes from `graphlava_request_encode` into a new request.
/// Malformed bytes give `GRAPHLAVA_ERR_INVALID_REQUEST`.
///
/// # Safety
/// `buf` is null (with `len == 0`) or valid for `len` bytes; `out` is null
/// or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_request_decode(
    buf: *const u8,
    len: usize,
    out: *mut *mut graphlava_request,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = std::ptr::null_mut() };
        if buf.is_null() && len != 0 {
            return fail(GRAPHLAVA_ERR_INVALID_ARGUMENT, "`buf` is NULL");
        }
        let bytes: &[u8] = if len == 0 {
            &[]
        } else {
            // SAFETY: per the contract.
            unsafe { std::slice::from_raw_parts(buf, len) }
        };
        let r = Request::decode(bytes)?;
        r.to_body()?;
        // SAFETY: as above.
        unsafe { *out = insert_handle(Slot::Request(Arc::new(Mutex::new(r)))) };
        Ok(())
    })
}

/// Frees a request. NULL and already-freed handles are ignored.
///
/// # Safety
/// `req` is null or a pointer from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_request_free(req: *mut graphlava_request) {
    let _ = catch_unwind(|| free_handle(req as usize));
}

fn store_result(results: Vec<OpResult>, out: *mut *mut graphlava_result) {
    if !out.is_null() {
        let h = insert_handle(Slot::Result(Arc::new(ResultH { results })));
        // SAFETY: the caller checked `out` is writable (contract of the API).
        unsafe { *out = h };
    }
}

/// Runs a request as one write request ([[16-api#Write requests]]): all of its
/// operations apply, or none, and the call returns once the durability level
/// holds. `*out` (optional) receives the per-operation results; free them
/// with `graphlava_result_free`.
///
/// # Safety
/// `db` and `req` are null or handles; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_submit(
    db: *const graphlava_db,
    req: *const graphlava_request,
    durability_level: graphlava_durability,
    out: *mut *mut graphlava_result,
) -> graphlava_status {
    guard(|| {
        // SAFETY: null or writable per the contract.
        unsafe { put_opt(out, std::ptr::null_mut()) };
        let h = get_db(db, "db")?;
        let r = get_request(req, "req")?;
        let d = durability(durability_level)?;
        let body = lock(&r).to_body()?;
        let results = match body {
            graphlava::RequestBody::Ops(w) => h.db.submit(w, d)?.results,
            graphlava::RequestBody::SparqlUpdate(t) => {
                h.db.sparql_update(&t, d)?;
                Vec::new()
            }
            graphlava::RequestBody::Datalog(t) => {
                h.db.datalog_update(&t, d)?;
                Vec::new()
            }
        };
        store_result(results, out);
        Ok(())
    })
}

/// Stores the number of operation results in `*out_n`.
///
/// # Safety
/// `res` is null or a result handle; `out_n` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_result_len(
    res: *const graphlava_result,
    out_n: *mut usize,
) -> graphlava_status {
    guard(|| {
        let out_n = out_ptr(out_n, "out_n")?;
        let h = get_result(res, "res")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out_n = h.results.len() };
        Ok(())
    })
}

/// Stores the kind (`GRAPHLAVA_RESULT_*`) and value of result `i`.
///
/// # Safety
/// `res` is null or a result handle; the out-pointers are null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_result_get(
    res: *const graphlava_result,
    i: usize,
    out_kind: *mut graphlava_result_kind,
    out_value: *mut u64,
) -> graphlava_status {
    guard(|| {
        let out_kind = out_ptr(out_kind, "out_kind")?;
        let out_value = out_ptr(out_value, "out_value")?;
        let h = get_result(res, "res")?;
        let Some(r) = h.results.get(i) else {
            return fail(
                GRAPHLAVA_ERR_INVALID_ARGUMENT,
                format!("result {i} out of range ({} results)", h.results.len()),
            );
        };
        let (k, v) = match r {
            OpResult::Id(id) => (GRAPHLAVA_RESULT_ID, id.map_or(0, |i| i.0)),
            OpResult::Deleted(ids) => (GRAPHLAVA_RESULT_DELETED, ids.len() as u64),
            OpResult::Removed(b) => (GRAPHLAVA_RESULT_REMOVED, u64::from(*b)),
            OpResult::Changed(b) => (GRAPHLAVA_RESULT_CHANGED, u64::from(*b)),
            _ => (GRAPHLAVA_RESULT_DONE, 0),
        };
        // SAFETY: non-null and writable per the contract.
        unsafe {
            *out_kind = k;
            *out_value = v;
        }
        Ok(())
    })
}

/// Copies the IDs deleted by operation `i` (a `GRAPHLAVA_RESULT_DELETED`
/// result; empty otherwise) into `buf`; see the buffer convention of
/// `graphlava_find`.
///
/// # Safety
/// `res` is null or a result handle; `buf` is null (with `cap == 0`) or
/// valid for `cap` writes; `out_n` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_result_deleted(
    res: *const graphlava_result,
    i: usize,
    buf: *mut u64,
    cap: usize,
    out_n: *mut usize,
) -> graphlava_status {
    guard(|| {
        let h = get_result(res, "res")?;
        let Some(r) = h.results.get(i) else {
            return fail(
                GRAPHLAVA_ERR_INVALID_ARGUMENT,
                format!("result {i} out of range ({} results)", h.results.len()),
            );
        };
        let ids: Vec<u64> = match r {
            OpResult::Deleted(ids) => ids.iter().map(|i| i.0).collect(),
            _ => Vec::new(),
        };
        // SAFETY: per the contract.
        unsafe { fill(&ids, buf, cap, out_n) }
    })
}

/// Frees a result. NULL and already-freed handles are ignored.
///
/// # Safety
/// `res` is null or a pointer from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_result_free(res: *mut graphlava_result) {
    let _ = catch_unwind(|| free_handle(res as usize));
}

// ---- transactions ---------------------------------------------------------------

/// Starts a read snapshot for core reads. End it with `graphlava_read_end`;
/// `graphlava_suspend` ends it early (its next call gives
/// `GRAPHLAVA_ERR_CLOSED`). Waits while the database is suspended.
///
/// # Safety
/// `db` is null or a database handle; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_read_begin(
    db: *const graphlava_db,
    out: *mut *mut graphlava_read,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = std::ptr::null_mut() };
        let h = get_db(db, "db")?;
        let txn = h.db.read()?;
        // SAFETY: `txn` borrows the `Database` inside `h`, an `Arc` that the
        // new handle keeps alive (field `_db`) until after the snapshot slot
        // is dropped; `graphlava_close` refuses while the handle exists.
        let txn: ReadTxn<'static> =
            unsafe { std::mem::transmute::<ReadTxn<'_>, ReadTxn<'static>>(txn) };
        let snap = Arc::new(Mutex::new(Some(txn)));
        h.register(&snap);
        let r = insert_handle(Slot::Read(Arc::new(ReadH { snap, _db: h })));
        // SAFETY: as above.
        unsafe { *out = r };
        Ok(())
    })
}

/// Ends a read snapshot and frees the handle. NULL and already-ended handles
/// are ignored.
///
/// # Safety
/// `read` is null or a pointer from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_read_end(read: *mut graphlava_read) {
    let _ = catch_unwind(|| free_handle(read as usize));
}

/// Starts an interactive write transaction in this process. It sees its own
/// writes, holds the writer for at most the write time limit, and ends with
/// `graphlava_commit` or `graphlava_abort` (both free the handle). Prefer
/// `graphlava_submit` for one-shot writes.
///
/// # Safety
/// `db` is null or a database handle; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_write_begin(
    db: *const graphlava_db,
    durability_level: graphlava_durability,
    out: *mut *mut graphlava_write,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = std::ptr::null_mut() };
        let h = get_db(db, "db")?;
        let d = durability(durability_level)?;
        let txn = h.db.write(d)?;
        // SAFETY: as in `graphlava_read_begin`: the handle keeps the `Arc`
        // that owns the `Database` alive until after `txn` is dropped.
        let txn: WriteTxn<'static> =
            unsafe { std::mem::transmute::<WriteTxn<'_>, WriteTxn<'static>>(txn) };
        let w = insert_handle(Slot::Write(Arc::new(WriteH {
            txn: Mutex::new(Some(txn)),
            _db: h,
        })));
        // SAFETY: as above.
        unsafe { *out = w };
        Ok(())
    })
}

/// Runs a request inside a write transaction: operations run in order, each
/// all or nothing (a failing operation undoes only itself and the
/// transaction stays open); a text request runs as one call. `*out`
/// (optional) receives the per-operation results.
///
/// # Safety
/// `write` and `req` are null or handles; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_write_run(
    write: *mut graphlava_write,
    req: *const graphlava_request,
    out: *mut *mut graphlava_result,
) -> graphlava_status {
    guard(|| {
        // SAFETY: null or writable per the contract.
        unsafe { put_opt(out, std::ptr::null_mut()) };
        let w = get_write(write, "write")?;
        let r = get_request(req, "req")?;
        let body = lock(&r).to_body()?;
        let mut g = lock(&w.txn);
        let Some(txn) = g.as_mut() else {
            return fail(GRAPHLAVA_ERR_CLOSED, "the write transaction has ended");
        };
        let results = match body {
            graphlava::RequestBody::Ops(req) => req
                .ops
                .iter()
                .map(|op| txn.run(op))
                .collect::<graphlava::Result<Vec<_>>>()?,
            graphlava::RequestBody::SparqlUpdate(t) => {
                txn.sparql_update(&t)?;
                Vec::new()
            }
            graphlava::RequestBody::Datalog(t) => {
                txn.datalog(&t)?;
                Vec::new()
            }
        };
        store_result(results, out);
        Ok(())
    })
}

/// Commits a write transaction and frees its handle (on success and on
/// failure); returns once the durability level holds. A second commit or a
/// commit after abort gives `GRAPHLAVA_ERR_CLOSED`.
///
/// # Safety
/// `write` is null or a pointer from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_commit(write: *mut graphlava_write) -> graphlava_status {
    guard(|| {
        let w = take_write(write, "write")?;
        let txn = lock(&w.txn).take();
        match txn {
            Some(t) => {
                t.commit()?;
                Ok(())
            }
            None => fail(GRAPHLAVA_ERR_CLOSED, "the write transaction has ended"),
        }
    })
}

/// Aborts a write transaction, discarding its writes, and frees its handle.
///
/// # Safety
/// `write` is null or a pointer from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_abort(write: *mut graphlava_write) -> graphlava_status {
    guard(|| {
        let w = take_write(write, "write")?;
        if let Some(t) = lock(&w.txn).take() {
            t.abort();
        }
        Ok(())
    })
}

// ---- core reads -----------------------------------------------------------------

/// The ID of a term without interning it: `*out_id` is 0 when the term is not
/// stored.
///
/// # Safety
/// `read` is null or a read handle; `term` is null or a valid argument;
/// `out_id` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_lookup(
    read: *const graphlava_read,
    term: *const graphlava_arg,
    out_id: *mut u64,
) -> graphlava_status {
    guard(|| {
        let out_id = out_ptr(out_id, "out_id")?;
        // SAFETY: per the contract.
        let t = unsafe { read_term(term, "term") }?;
        let id = with_read(read, |r| r.lookup(&t))?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out_id = id.map_or(0, |i| i.0) };
        Ok(())
    })
}

/// Decodes any stored or inline ID into `*out` (free with
/// `graphlava_term_free`); ID 0 decodes to `GRAPHLAVA_TERM_UNBOUND`.
///
/// # Safety
/// `read` is null or a read handle; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_decode(
    read: *const graphlava_read,
    id: u64,
    out: *mut graphlava_term,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { out.write(empty_term()) };
        let t = with_read(read, |r| match id {
            0 => Ok(None),
            _ => r.decode(Id(id)).map(Some),
        })?;
        if let Some(t) = t {
            let c = c_term(&t)?;
            // SAFETY: as above.
            unsafe { out.write(c) };
        }
        Ok(())
    })
}

/// The triple of statement `id` into `out_spo[0..3]`; `GRAPHLAVA_ERR_NOT_FOUND`
/// when it does not exist.
///
/// # Safety
/// `read` is null or a read handle; `out_spo` is null or valid for 3 writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_stmt(
    read: *const graphlava_read,
    id: u64,
    out_spo: *mut u64,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out_spo, "out_spo")?;
        let t = with_read(read, |r| r.stmt(Id(id)))?;
        let Some(t) = t else {
            return fail(GRAPHLAVA_ERR_NOT_FOUND, format!("no statement {id:#x}"));
        };
        // SAFETY: valid for 3 writes per the contract.
        unsafe {
            *out = t.s.0;
            *out.add(1) = t.p.0;
            *out.add(2) = t.o.0;
        }
        Ok(())
    })
}

/// Every statement and inline property matching a pattern (0 = any) as
/// quads `(s, p, o, id)` (id 0 for an unpromoted inline property).
///
/// Buffer convention of every listing call: `buf` holds `cap` entries (here
/// quads, `4 * cap` integers); `*out_n` receives the total number of
/// entries; only `min(cap, *out_n)` are written, so call again with a larger
/// buffer when `*out_n > cap`. The snapshot keeps repeated calls consistent.
///
/// # Safety
/// `read` is null or a read handle; `buf` is null (with `cap == 0`) or valid
/// for `4 * cap` writes; `out_n` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_find(
    read: *const graphlava_read,
    s: u64,
    p: u64,
    o: u64,
    buf: *mut u64,
    cap: usize,
    out_n: *mut usize,
) -> graphlava_status {
    guard(|| {
        out_ptr(out_n, "out_n")?;
        let ms = with_read(read, |r| r.find(opt_id(s), opt_id(p), opt_id(o)))?;
        // SAFETY: per the contract (quads are written as 4 integers each).
        unsafe { fill_quads(&ms, buf, cap, out_n) }
    })
}

/// # Safety
/// As [`graphlava_find`].
unsafe fn fill_quads(ms: &[Match], buf: *mut u64, cap: usize, out_n: *mut usize) -> FfiResult {
    let q = quads(ms);
    let mut n = 0usize;
    // SAFETY: `buf` holds `4 * cap` integers per the contract.
    unsafe { fill(&q, buf, cap.saturating_mul(4), &mut n) }?;
    // SAFETY: checked non-null by the caller.
    unsafe { *out_n = ms.len() };
    Ok(())
}

/// Statements with subject `s` and predicate `p` (0 = any), SPO prefix, plus
/// the subject's inline properties; quads as `graphlava_find`.
///
/// # Safety
/// As [`graphlava_find`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_out(
    read: *const graphlava_read,
    s: u64,
    p: u64,
    buf: *mut u64,
    cap: usize,
    out_n: *mut usize,
) -> graphlava_status {
    guard(|| {
        out_ptr(out_n, "out_n")?;
        let ms = with_read(read, |r| r.out(Id(s), opt_id(p)))?;
        // SAFETY: per the contract.
        unsafe { fill_quads(&ms, buf, cap, out_n) }
    })
}

/// Statements with object `o` and predicate `p` (0 = any); quads as
/// `graphlava_find`.
///
/// # Safety
/// As [`graphlava_find`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_in(
    read: *const graphlava_read,
    o: u64,
    p: u64,
    buf: *mut u64,
    cap: usize,
    out_n: *mut usize,
) -> graphlava_status {
    guard(|| {
        out_ptr(out_n, "out_n")?;
        let ms = with_read(read, |r| r.inc(Id(o), opt_id(p)))?;
        // SAFETY: per the contract.
        unsafe { fill_quads(&ms, buf, cap, out_n) }
    })
}

/// The inline properties of a node (one record read); quads as
/// `graphlava_find`.
///
/// # Safety
/// As [`graphlava_find`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_props(
    read: *const graphlava_read,
    node: u64,
    buf: *mut u64,
    cap: usize,
    out_n: *mut usize,
) -> graphlava_status {
    guard(|| {
        out_ptr(out_n, "out_n")?;
        let ms = with_read(read, |r| r.props(Id(node)))?;
        // SAFETY: per the contract.
        unsafe { fill_quads(&ms, buf, cap, out_n) }
    })
}

/// The graph named by `name`, without creating it: `*out_id` is 0 when it
/// does not exist.
///
/// # Safety
/// `read` is null or a read handle; `name` is null or a valid argument;
/// `out_id` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_graph(
    read: *const graphlava_read,
    name: *const graphlava_arg,
    out_id: *mut u64,
) -> graphlava_status {
    guard(|| {
        let out_id = out_ptr(out_id, "out_id")?;
        // SAFETY: per the contract.
        let t = unsafe { read_term(name, "name") }?;
        let g = with_read(read, |r| r.graph(&t))?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out_id = g.map_or(0, |i| i.0) };
        Ok(())
    })
}

/// The graphs an item is a direct member of; IDs with the buffer convention
/// of `graphlava_find` (one integer per entry).
///
/// # Safety
/// `read` is null or a read handle; `buf` is null (with `cap == 0`) or valid
/// for `cap` writes; `out_n` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_graphs_of(
    read: *const graphlava_read,
    item: u64,
    buf: *mut u64,
    cap: usize,
    out_n: *mut usize,
) -> graphlava_status {
    guard(|| {
        out_ptr(out_n, "out_n")?;
        let ids = with_read(read, |r| r.graphs_of(Id(item)))?;
        let ids: Vec<u64> = ids.iter().map(|i| i.0).collect();
        // SAFETY: per the contract.
        unsafe { fill(&ids, buf, cap, out_n) }
    })
}

/// The asserted statements in the scope of `graph` (`direct` 0: the graph and
/// every graph nested under it; 1: the graph only), each once, in ID order;
/// IDs with the buffer convention of `graphlava_graphs_of`.
///
/// # Safety
/// As [`graphlava_graphs_of`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_scope(
    read: *const graphlava_read,
    graph: u64,
    direct: u32,
    buf: *mut u64,
    cap: usize,
    out_n: *mut usize,
) -> graphlava_status {
    guard(|| {
        out_ptr(out_n, "out_n")?;
        let ids = with_read(read, |r| r.scope_statements(Id(graph), direct != 0))?;
        let ids: Vec<u64> = ids.iter().map(|i| i.0).collect();
        // SAFETY: per the contract.
        unsafe { fill(&ids, buf, cap, out_n) }
    })
}

/// One hyperedge: its type and home graph, and its `(role, filler)` pairs
/// with the buffer convention of `graphlava_find` (two integers per entry).
/// `GRAPHLAVA_ERR_NOT_FOUND` when it does not exist.
///
/// # Safety
/// `read` is null or a read handle; `out_ty` and `out_graph` are null or
/// writable; `buf` is null (with `cap == 0`) or valid for `2 * cap` writes;
/// `out_n` is null or writable.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn graphlava_hedge(
    read: *const graphlava_read,
    hedge: u64,
    out_ty: *mut u64,
    out_graph: *mut u64,
    buf: *mut u64,
    cap: usize,
    out_n: *mut usize,
) -> graphlava_status {
    guard(|| {
        let out_n = out_ptr(out_n, "out_n")?;
        let h = with_read(read, |r| r.hedge(Id(hedge)))?;
        let Some(h) = h else {
            return fail(GRAPHLAVA_ERR_NOT_FOUND, format!("no hyperedge {hedge:#x}"));
        };
        let pairs: Vec<u64> = h.roles.iter().flat_map(|(r, n)| [r.0, n.0]).collect();
        let mut n = 0usize;
        // SAFETY: per the contract.
        unsafe { fill(&pairs, buf, cap.saturating_mul(2), &mut n) }?;
        // SAFETY: null or writable per the contract.
        unsafe {
            put_opt(out_ty, h.ty.0);
            put_opt(out_graph, h.graph.0);
            *out_n = h.roles.len();
        }
        Ok(())
    })
}

// ---- queries --------------------------------------------------------------------

/// Prepares a query: `lang` is `GRAPHLAVA_LANG_SPARQL` or
/// `GRAPHLAVA_LANG_DATALOG`. The handle stores the language and text and is
/// independent of any database; parse errors surface at `graphlava_query`.
/// Free with `graphlava_prepared_free`.
///
/// # Safety
/// `text` is null or NUL-terminated; `out` is null or writable. `db` is
/// accepted for API symmetry and may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_prepare(
    db: *const graphlava_db,
    lang: graphlava_lang,
    text: *const c_char,
    out: *mut *mut graphlava_prepared,
) -> graphlava_status {
    let _ = db;
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = std::ptr::null_mut() };
        let lang = lang_of(lang)?;
        // SAFETY: per the contract.
        let text = unsafe { arg_str(text, "text") }?.to_string();
        // SAFETY: as above.
        unsafe { *out = insert_handle(Slot::Prepared(Arc::new(PreparedH { lang, text }))) };
        Ok(())
    })
}

/// Frees a prepared query. NULL and already-freed handles are ignored.
///
/// # Safety
/// `p` is null or a pointer from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_prepared_free(p: *mut graphlava_prepared) {
    let _ = catch_unwind(|| free_handle(p as usize));
}

/// Runs a prepared query on a fresh snapshot. `*out` receives the result
/// handle, which keeps the snapshot open (and `graphlava_close` refused) until
/// `graphlava_rows_free`. Waits while the database is suspended.
///
/// # Safety
/// `db` and `prepared` are null or live handles; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_query(
    db: *const graphlava_db,
    prepared: *const graphlava_prepared,
    out: *mut *mut graphlava_rows,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = std::ptr::null_mut() };
        let h = get_db(db, "db")?;
        let q = get_prepared(prepared, "prepared")?;
        let (txn, rows) = h.db.query_ids(q.lang, &q.text)?;
        // SAFETY: `txn` borrows the `Database` inside `h`. The result handle
        // keeps `h` (an `Arc`) alive in its `_db` field, which is dropped
        // after the snapshot slot (field order), and `graphlava_close` refuses
        // (GRAPHLAVA_ERR_BUSY) while the handle holds the `Arc`. So the
        // database outlives the snapshot even though the lifetime is erased.
        let txn: ReadTxn<'static> =
            unsafe { std::mem::transmute::<ReadTxn<'_>, ReadTxn<'static>>(txn) };
        let snap = Arc::new(Mutex::new(Some(txn)));
        h.register(&snap);
        let r = insert_handle(Slot::Rows(Arc::new(RowsH {
            snap,
            state: Mutex::new(RowsState {
                rows,
                cursor: 0,
                batch: Vec::new(),
            }),
            _db: h,
        })));
        // SAFETY: as above.
        unsafe { *out = r };
        Ok(())
    })
}

/// Number of result columns.
///
/// # Safety
/// `rows` is null or a live handle; `out_n` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_rows_columns(
    rows: *const graphlava_rows,
    out_n: *mut usize,
) -> graphlava_status {
    guard(|| {
        let out_n = out_ptr(out_n, "out_n")?;
        let r = get_rows(rows, "rows")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out_n = lock(&r.state).rows.vars.len() };
        Ok(())
    })
}

/// Name of column `i` (without `?`), copied into `*out`; free with
/// `graphlava_string_free`.
///
/// # Safety
/// `rows` is null or a live handle; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_rows_column_name(
    rows: *const graphlava_rows,
    i: usize,
    out: *mut *mut c_char,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: non-null and writable per the contract.
        unsafe { *out = std::ptr::null_mut() };
        let r = get_rows(rows, "rows")?;
        let st = lock(&r.state);
        let Some(name) = st.rows.vars.get(i) else {
            return fail(
                GRAPHLAVA_ERR_INVALID_ARGUMENT,
                format!("column {i} out of range ({} columns)", st.rows.vars.len()),
            );
        };
        let s = into_raw(name, "column name")?;
        // SAFETY: as above.
        unsafe { *out = s };
        Ok(())
    })
}

/// Returns the next batch of at most 1024 rows as a column-major array of IDs:
/// the ID of row `r`, column `c` is `ids[c * out_rows + r]`, and 0 means
/// unbound. The array is owned by `rows` and valid until the next call on it
/// or `graphlava_rows_free`. `*out_rows` is 0 when the result is exhausted.
/// `*out_cols` equals `graphlava_rows_columns`; with zero columns `*out_ids`
/// is non-null but must not be read. After `graphlava_suspend` ended the
/// result's snapshot this gives `GRAPHLAVA_ERR_CLOSED`.
///
/// # Safety
/// `rows` is null or a live handle; the out-pointers are null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_rows_next_batch(
    rows: *mut graphlava_rows,
    out_ids: *mut *const u64,
    out_rows: *mut usize,
    out_cols: *mut usize,
) -> graphlava_status {
    guard(|| {
        let out_ids = out_ptr(out_ids, "out_ids")?;
        let out_rows = out_ptr(out_rows, "out_rows")?;
        let out_cols = out_ptr(out_cols, "out_cols")?;
        let r = get_rows(rows, "rows")?;
        if lock(&r.snap).is_none() {
            return snapshot_ended();
        }
        let mut st = lock(&r.state);
        let st = &mut *st;
        let cols = st.rows.vars.len();
        let start = st.cursor.min(st.rows.rows.len());
        let n = (st.rows.rows.len() - start).min(GRAPHLAVA_BATCH_ROWS);
        st.batch.clear();
        st.batch.resize(n * cols, 0);
        for (ri, row) in st.rows.rows[start..start + n].iter().enumerate() {
            for c in 0..cols {
                st.batch[c * n + ri] = row.get(c).map_or(0, |id| id.0);
            }
        }
        st.cursor = start + n;
        // SAFETY: the out-pointers are non-null and writable per the contract.
        // The buffer lives in the handle until the next call or free.
        unsafe {
            *out_ids = st.batch.as_ptr();
            *out_rows = n;
            *out_cols = cols;
        }
        Ok(())
    })
}

/// Decodes an ID of this result into `*out` (free with `graphlava_term_free`).
/// ID 0 decodes to `GRAPHLAVA_TERM_UNBOUND`. On failure `*out` is zeroed.
///
/// # Safety
/// `rows` is null or a live handle; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_term_decode(
    rows: *const graphlava_rows,
    id: u64,
    out: *mut graphlava_term,
) -> graphlava_status {
    guard(|| {
        let out = out_ptr(out, "out")?;
        // SAFETY: non-null and writable per the contract; the previous
        // content is not read (it may be uninitialised).
        unsafe { out.write(empty_term()) };
        let r = get_rows(rows, "rows")?;
        if id == 0 {
            return Ok(());
        }
        let term = {
            let snap = lock(&r.snap);
            let Some(txn) = snap.as_ref() else {
                return snapshot_ended();
            };
            let st = lock(&r.state);
            txn.decode_row_id(&st.rows, Id(id))?
        };
        let t = c_term(&term)?;
        // SAFETY: as above.
        unsafe { out.write(t) };
        Ok(())
    })
}

/// Frees the strings of a decoded term and sets them to NULL. NULL is a no-op.
///
/// # Safety
/// `t` is null or a term filled by `graphlava_term_decode` (or zeroed).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_term_free(t: *mut graphlava_term) {
    // SAFETY: per the contract.
    let Some(t) = (unsafe { t.as_mut() }) else {
        return;
    };
    for p in [&mut t.value, &mut t.datatype, &mut t.lang] {
        // SAFETY: each non-null string came from `CString::into_raw`.
        unsafe { graphlava_string_free(*p) };
        *p = std::ptr::null_mut();
    }
    t.kind = GRAPHLAVA_TERM_UNBOUND;
}

/// Frees a result handle, ending its snapshot. NULL and already-freed handles
/// are ignored.
///
/// # Safety
/// `rows` is null or a pointer from this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_rows_free(rows: *mut graphlava_rows) {
    let _ = catch_unwind(|| free_handle(rows as usize));
}

// ---- administration -------------------------------------------------------------

/// Runs a full dictionary mark-and-sweep; terms interned less than
/// `window_ms` ago are kept. `*out_deleted` (optional) receives the number of
/// terms deleted.
///
/// # Safety
/// `db` is null or a database handle; `out_deleted` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_sweep(
    db: *const graphlava_db,
    window_ms: u64,
    out_deleted: *mut u64,
) -> graphlava_status {
    guard(|| {
        let h = get_db(db, "db")?;
        let rep = h.db.admin().sweep(Duration::from_millis(window_ms))?;
        // SAFETY: null or writable per the contract.
        unsafe { put_opt(out_deleted, rep.deleted) };
        Ok(())
    })
}

/// Writes a consistent copy of the database file to `path` (writes wait for
/// the copy). The copy opens like any database file.
///
/// # Safety
/// `db` is null or a database handle; `path` is null or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_export_snapshot(
    db: *const graphlava_db,
    path: *const c_char,
) -> graphlava_status {
    guard(|| {
        let h = get_db(db, "db")?;
        // SAFETY: per the contract.
        let path = unsafe { arg_str(path, "path") }?;
        h.db.export_snapshot(path)?;
        Ok(())
    })
}

// ---- iOS extension journal ------------------------------------------------------

/// Appends a request to the extension journal at `path` (created if absent)
/// and syncs it; `*out_seq` (optional) receives the frame's sequence number.
/// Called by an app extension, which never opens the database file
/// ([[16-api#Mobile bindings]]).
///
/// # Safety
/// `path` is null or NUL-terminated; `req` is null or a request handle;
/// `out_seq` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_journal_append(
    path: *const c_char,
    req: *const graphlava_request,
    out_seq: *mut u64,
) -> graphlava_status {
    guard(|| {
        // SAFETY: per the contract.
        let path = unsafe { arg_str(path, "path") }?;
        let r = get_request(req, "req")?;
        let r = lock(&r).clone();
        r.to_body()?;
        let seq = journal::append(std::path::Path::new(path), &r.encode())?;
        // SAFETY: null or writable per the contract.
        unsafe { put_opt(out_seq, seq) };
        Ok(())
    })
}

/// Applies the extension journal at `path` to the database: each complete
/// frame runs once as one write request, recorded under `key` (the extension
/// identifier) in the same transaction; a torn tail is dropped; the journal is
/// then truncated. A missing journal is empty. `*out` (optional) receives
/// the counts.
///
/// # Safety
/// `db` is null or a database handle; `path` and `key` are null or
/// NUL-terminated; `out` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn graphlava_journal_apply(
    db: *const graphlava_db,
    path: *const c_char,
    key: *const c_char,
    out: *mut graphlava_journal_report,
) -> graphlava_status {
    guard(|| {
        let h = get_db(db, "db")?;
        // SAFETY: per the contract.
        let path = unsafe { arg_str(path, "path") }?;
        // SAFETY: per the contract.
        let key = unsafe { arg_str(key, "key") }?;
        let rep = journal::apply(&h.db, std::path::Path::new(path), key)?;
        // SAFETY: null or writable per the contract.
        unsafe {
            put_opt(
                out,
                graphlava_journal_report {
                    applied: rep.applied,
                    skipped: rep.skipped,
                    failed: rep.failed,
                    dropped_bytes: rep.dropped_bytes,
                },
            )
        };
        Ok(())
    })
}

/// Applies a journal like [`graphlava_journal_apply`] and calls `after_frame`
/// after each frame commits (crash tests kill the process there). Rust only.
#[doc(hidden)]
pub fn journal_apply_hooked(
    db: *const graphlava_db,
    path: &std::path::Path,
    key: &str,
    after_frame: impl FnMut(u64),
) -> std::result::Result<journal::Report, graphlava_status> {
    let h = get_db(db, "db").map_err(|f| f.status)?;
    journal::apply_with(&h.db, path, key, after_frame).map_err(|e| status_of(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_core() {
        // SAFETY: graphlava_version returns a pointer to a static C string.
        let v = unsafe { CStr::from_ptr(graphlava_version()) };
        assert_eq!(v.to_str().unwrap(), graphlava::VERSION);
    }

    #[test]
    fn panic_becomes_internal_status() {
        let st = guard(|| panic!("boom"));
        assert_eq!(st, GRAPHLAVA_ERR_INTERNAL);
        let mut p = std::ptr::null_mut();
        // SAFETY: valid out-pointer; the string is freed once.
        unsafe {
            assert_eq!(graphlava_last_error(&mut p), GRAPHLAVA_OK);
            let msg = CStr::from_ptr(p).to_str().unwrap().to_string();
            graphlava_string_free(p);
            assert!(msg.contains("panic") && msg.contains("boom"), "{msg}");
        }
    }

    #[test]
    fn tokens_are_never_reused_and_unknown_pointers_are_rejected() {
        let a: *mut graphlava_request = insert_handle(Slot::Result(Arc::new(ResultH {
            results: Vec::new(),
        })));
        free_handle(a as usize);
        assert!(matches!(
            get_slot(a as usize, "a"),
            Err(Fail {
                status: GRAPHLAVA_ERR_CLOSED,
                ..
            })
        ));
        assert!(matches!(
            get_slot(7, "x"),
            Err(Fail {
                status: GRAPHLAVA_ERR_INVALID_ARGUMENT,
                ..
            })
        ));
        let b: *mut graphlava_request = insert_handle(Slot::Result(Arc::new(ResultH {
            results: Vec::new(),
        })));
        assert_ne!(a as usize, b as usize);
        // Wrong kind.
        assert!(matches!(
            get_request(b, "b"),
            Err(Fail {
                status: GRAPHLAVA_ERR_INVALID_ARGUMENT,
                ..
            })
        ));
        free_handle(b as usize);
    }
}
