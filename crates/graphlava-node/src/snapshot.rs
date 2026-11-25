//! Read snapshots and row sets ([[16-api#Node.js binding]]).
//!
//! A [`Snap`] keeps one read snapshot of an [`Owned`] database alive; the
//! process-wide counter behind `graphlava._debug.openSnapshots()` counts them.
//! A [`NativeRows`] holds the ID rows of one query and its snapshot, hands
//! out batches of at most 1024 rows with one `BigUint64Array` per column, and
//! decodes terms only for the columns a caller asks for (rule
//! `api-rows-late`). Releasing it ends the snapshot at once.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use graphlava::sparql::IdRows;
use graphlava::{Id, ReadTxn};
use napi::Env;
use napi::bindgen_prelude::{BigUint64Array, Object};
use napi_derive::napi;

use crate::conv::{self, Out, ReadIn};
use crate::db::{Live, Owned, read_call};
use crate::error::{Fail, R};
use crate::pool::{guarded, on_pool};

/// Rows per batch, the engine's batch size (rule `api-ffi-rows`).
pub(crate) const BATCH_ROWS: usize = 1024;

static OPEN_SNAPSHOTS: AtomicUsize = AtomicUsize::new(0);

/// Read snapshots alive in this process (debug counter).
pub(crate) fn open_snapshots() -> usize {
    OPEN_SNAPSHOTS.load(Ordering::SeqCst)
}

/// One read snapshot of an open database.
pub(crate) struct Snap {
    // Field order matters: `txn` drops before `live` releases the database.
    txn: ReadTxn<'static>,
    _live: Live,
}

impl std::fmt::Debug for Snap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Snap")
    }
}

impl Snap {
    pub fn new(o: &Arc<Owned>) -> R<Arc<Snap>> {
        let live = Live::new(o);
        let txn = live.owned().db.read()?;
        // SAFETY: `txn` borrows the `Database` inside the `Owned` that `live`
        // keeps alive through its `Arc`. The `Owned` sits in that `Arc`'s heap
        // allocation and never moves, and it is never closed or dropped while
        // `live` holds a reference: `close()` refuses while the live count is
        // non-zero, and the `Arc` keeps it from being dropped. `txn` drops
        // before `live` (field order), so the database outlives the snapshot
        // even though the lifetime is erased here.
        let txn: ReadTxn<'static> =
            unsafe { std::mem::transmute::<ReadTxn<'_>, ReadTxn<'static>>(txn) };
        OPEN_SNAPSHOTS.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Snap { txn, _live: live }))
    }
}

impl Drop for Snap {
    fn drop(&mut self) {
        OPEN_SNAPSHOTS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A read snapshot handle (`db.read()` in JS).
#[napi]
#[derive(Debug)]
pub struct NativeSnapshot {
    snap: Mutex<Option<Arc<Snap>>>,
}

impl NativeSnapshot {
    pub(crate) fn new(s: Arc<Snap>) -> NativeSnapshot {
        NativeSnapshot {
            snap: Mutex::new(Some(s)),
        }
    }

    fn get(&self) -> R<Arc<Snap>> {
        self.snap
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| Fail::invalid("the snapshot is closed"))
    }
}

#[napi]
impl NativeSnapshot {
    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn read<'e>(&self, env: &'e Env, req: ReadIn) -> napi::Result<Object<'e>> {
        let s = self.get();
        on_pool(env, move || read_call(&s?.txn, &req))
    }

    #[napi]
    pub fn read_sync(&self, req: ReadIn) -> napi::Result<Out> {
        let s = self.get()?;
        Ok(guarded(|| read_call(&s.txn, &req))?)
    }

    /// Runs a query on this snapshot; the rows share it.
    #[napi(ts_return_type = "Promise<NativeRows>")]
    pub fn query<'e>(&self, env: &'e Env, lang: String, text: String) -> napi::Result<Object<'e>> {
        let s = self.get();
        on_pool(env, move || {
            Ok(Out::Rows(NativeRows::run(s?, &lang, &text)?))
        })
    }

    #[napi]
    pub fn query_sync(&self, lang: String, text: String) -> napi::Result<NativeRows> {
        let s = self.get()?;
        Ok(guarded(|| NativeRows::run(s, &lang, &text))?)
    }

    /// Ends this handle's hold on the snapshot (row sets keep their own).
    #[napi]
    pub fn release(&self) {
        drop(self.snap.lock().unwrap_or_else(|p| p.into_inner()).take());
    }
}

struct RowsState {
    snap: Option<Arc<Snap>>,
    rows: Arc<IdRows>,
    cursor: usize,
}

// @tg: implements:: [[openspec:node-binding#Node row streaming]]
/// The rows of one query and the snapshot that decodes them.
#[napi]
pub struct NativeRows {
    st: Arc<Mutex<RowsState>>,
    vars: Vec<String>,
    len: usize,
}

impl std::fmt::Debug for NativeRows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeRows")
            .field("vars", &self.vars)
            .field("len", &self.len)
            .finish()
    }
}

fn next_batch(st: &Mutex<RowsState>, decode: bool) -> R<Out> {
    let mut g = st.lock().unwrap_or_else(|p| p.into_inner());
    if g.cursor >= g.rows.rows.len() {
        g.snap = None;
        return Ok(Out::Obj(vec![
            ("n", Out::Num(0.0)),
            ("ids", Out::Arr(Vec::new())),
        ]));
    }
    let Some(snap) = g.snap.clone() else {
        return Err(Fail::invalid("the rows are closed"));
    };
    let rows = Arc::clone(&g.rows);
    let from = g.cursor;
    let to = (from + BATCH_ROWS).min(rows.rows.len());
    g.cursor = to;
    let width = rows.vars.len();
    let mut cols: Vec<Vec<u64>> = vec![Vec::with_capacity(to - from); width];
    for r in &rows.rows[from..to] {
        for (c, id) in r.iter().enumerate() {
            cols[c].push(id.0);
        }
    }
    let terms = if decode {
        let mut out = Vec::with_capacity(width);
        for col in &cols {
            out.push(decode_ids(&snap, &rows, col.iter().copied())?);
        }
        Some(out)
    } else {
        None
    };
    // Row-by-row reading needs no snapshot after the last batch.
    if decode && to == rows.rows.len() {
        g.snap = None;
    }
    drop(g);
    let mut f = vec![
        ("n", Out::Num((to - from) as f64)),
        ("ids", Out::Arr(cols.into_iter().map(Out::Ids).collect())),
    ];
    if let Some(t) = terms {
        f.push(("terms", Out::Arr(t)));
    }
    Ok(Out::Obj(f))
}

fn decode_ids(snap: &Snap, rows: &IdRows, ids: impl Iterator<Item = u64>) -> R<Out> {
    let mut out = Vec::new();
    for i in ids {
        out.push(if i == 0 {
            Out::Undefined
        } else {
            conv::term_out(&snap.txn.decode_row_id(rows, Id(i))?)
        });
    }
    Ok(Out::Arr(out))
}

fn decode_col(st: &Mutex<RowsState>, ids: &[u64]) -> R<Out> {
    let g = st.lock().unwrap_or_else(|p| p.into_inner());
    let Some(snap) = g.snap.clone() else {
        return Err(Fail::invalid(
            "the rows are closed: decode a batch before the iteration ends",
        ));
    };
    let rows = Arc::clone(&g.rows);
    drop(g);
    decode_ids(&snap, &rows, ids.iter().copied())
}

impl NativeRows {
    pub(crate) fn run(snap: Arc<Snap>, lang: &str, text: &str) -> R<NativeRows> {
        let rows = snap.txn.query_ids(conv::lang(lang)?, text)?;
        Ok(NativeRows {
            vars: rows.vars.clone(),
            len: rows.rows.len(),
            st: Arc::new(Mutex::new(RowsState {
                snap: Some(snap),
                rows: Arc::new(rows),
                cursor: 0,
            })),
        })
    }
}

#[napi]
impl NativeRows {
    #[napi(getter)]
    pub fn vars(&self) -> Vec<String> {
        self.vars.clone()
    }

    #[napi(getter)]
    pub fn length(&self) -> u32 {
        self.len as u32
    }

    /// The next batch: `{n, ids: BigUint64Array[], terms?}`; `n == 0` at the end.
    #[napi(ts_return_type = "Promise<unknown>")]
    pub fn next_batch<'e>(&self, env: &'e Env, decode: bool) -> napi::Result<Object<'e>> {
        let st = Arc::clone(&self.st);
        on_pool(env, move || next_batch(&st, decode))
    }

    #[napi]
    pub fn next_batch_sync(&self, decode: bool) -> napi::Result<Out> {
        Ok(guarded(|| next_batch(&self.st, decode))?)
    }

    /// Decodes one column of IDs from this result.
    #[napi(ts_return_type = "Promise<unknown[]>")]
    pub fn decode<'e>(&self, env: &'e Env, ids: BigUint64Array) -> napi::Result<Object<'e>> {
        let st = Arc::clone(&self.st);
        let ids = ids.to_vec();
        on_pool(env, move || decode_col(&st, &ids))
    }

    #[napi]
    pub fn decode_sync(&self, ids: BigUint64Array) -> napi::Result<Out> {
        Ok(guarded(|| decode_col(&self.st, &ids))?)
    }

    /// Ends the snapshot now; later batches and decodes fail.
    #[napi]
    pub fn release(&self) {
        let snap = self
            .st
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .snap
            .take();
        drop(snap);
    }
}
