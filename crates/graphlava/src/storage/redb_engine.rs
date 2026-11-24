//! The redb adapter ([[05-storage-trait#redb adapter]]). This is the only module
//! that names redb types.
//!
//! Fixed-width keys and values map to `&[u8; N]` tables and variable ones to
//! `&[u8]` (rule `pl-v1-plain`); empty values map to `()`. `Flush` is
//! `Durability::Immediate` and `NoFlush` is `Durability::None`
//! (rule `st-durability-map`, still provisional per `st-redb-durability-verify`).

// @tg: implements:: [[openspec:transactions#Single owner process]]
// @tg: implements:: [[openspec:transactions#Snapshot reads and one writer]]

use std::ops::Bound;
use std::path::Path;

use redb::{ReadableTable, TableDefinition};

use super::{EngineDurability, Entry, KvRead, KvWrite, Storage, TableDef, Width};
use crate::error::{Error, Result, corrupted};

fn map_storage(e: redb::StorageError) -> Error {
    match e {
        redb::StorageError::Io(io) => {
            if io.raw_os_error() == Some(28) {
                Error::Full
            } else {
                Error::Io(io)
            }
        }
        redb::StorageError::Corrupted(m) => Error::Corrupted(m),
        other => corrupted(other.to_string()),
    }
}

fn map_db(e: redb::DatabaseError) -> Error {
    match e {
        redb::DatabaseError::DatabaseAlreadyOpen => Error::Locked,
        redb::DatabaseError::UpgradeRequired(v) => {
            Error::FormatMismatch(format!("redb file format {v} needs an upgrade"))
        }
        redb::DatabaseError::Storage(s) => map_storage(s),
        other => corrupted(other.to_string()),
    }
}

fn map_tx(e: redb::TransactionError) -> Error {
    match e {
        redb::TransactionError::Storage(s) => map_storage(s),
        other => corrupted(other.to_string()),
    }
}

fn map_table(e: redb::TableError) -> Error {
    match e {
        redb::TableError::Storage(s) => map_storage(s),
        redb::TableError::TableTypeMismatch { table, key, value } => Error::FormatMismatch(
            format!("table {table} has types ({key:?}, {value:?}) the format does not expect"),
        ),
        other => corrupted(other.to_string()),
    }
}

fn map_commit(e: redb::CommitError) -> Error {
    match e {
        redb::CommitError::Storage(s) => map_storage(s),
        other => corrupted(other.to_string()),
    }
}

/// Pads a short bound of a fixed-width key so it can be decoded as a key,
/// preserving its meaning (prefix scans pass prefixes shorter than the key).
fn pad(b: Bound<&[u8]>, w: Width, is_hi: bool) -> Bound<Vec<u8>> {
    let n = match w {
        Width::Fixed(n) => n,
        Width::Var => {
            return match b {
                Bound::Included(x) => Bound::Included(x.to_vec()),
                Bound::Excluded(x) => Bound::Excluded(x.to_vec()),
                Bound::Unbounded => Bound::Unbounded,
            };
        }
    };
    let fill = |x: &[u8], byte: u8| {
        let mut v = x.to_vec();
        v.resize(n, byte);
        v
    };
    match (b, is_hi) {
        (Bound::Unbounded, _) => Bound::Unbounded,
        (Bound::Included(x), _) | (Bound::Excluded(x), _) if x.len() > n => {
            // Longer than the key width cannot occur for well-formed callers.
            Bound::Included(x[..n].to_vec())
        }
        (Bound::Included(x), false) => Bound::Included(fill(x, 0)),
        (Bound::Excluded(x), false) if x.len() < n => Bound::Included(fill(x, 0)),
        (Bound::Excluded(x), false) => Bound::Excluded(x.to_vec()),
        (Bound::Included(x), true) => Bound::Included(fill(x, 0xFF)),
        (Bound::Excluded(x), true) => Bound::Excluded(fill(x, 0)),
    }
}

fn borrow<'a, K: redb::Key + 'static>(b: &'a Bound<Vec<u8>>) -> Bound<K::SelfType<'a>> {
    match b {
        Bound::Included(v) => Bound::Included(K::from_bytes(v)),
        Bound::Excluded(v) => Bound::Excluded(K::from_bytes(v)),
        Bound::Unbounded => Bound::Unbounded,
    }
}

fn kbytes<K: redb::Key + 'static>(k: &K::SelfType<'_>) -> Vec<u8> {
    K::as_bytes(k).as_ref().to_vec()
}

fn vbytes<V: redb::Value + 'static>(v: &V::SelfType<'_>) -> Vec<u8> {
    V::as_bytes(v).as_ref().to_vec()
}

/// Calls `$f::<K, V>(args)` with the redb types of the table's layout.
macro_rules! dispatch {
    ($t:expr, $f:ident ( $($arg:expr),* )) => {
        match ($t.key, $t.value) {
            (Width::Fixed(2), Width::Var) => $f::<&[u8; 2], &[u8]>($($arg),*),
            (Width::Fixed(4), Width::Var) => $f::<&[u8; 4], &[u8]>($($arg),*),
            (Width::Fixed(8), Width::Var) => $f::<&[u8; 8], &[u8]>($($arg),*),
            (Width::Fixed(8), Width::Fixed(8)) => $f::<&[u8; 8], &[u8; 8]>($($arg),*),
            (Width::Fixed(8), Width::Fixed(16)) => $f::<&[u8; 8], &[u8; 16]>($($arg),*),
            (Width::Fixed(8), Width::Fixed(26)) => $f::<&[u8; 8], &[u8; 26]>($($arg),*),
            (Width::Fixed(16), Width::Fixed(0)) => $f::<&[u8; 16], ()>($($arg),*),
            (Width::Fixed(16), Width::Fixed(9)) => $f::<&[u8; 16], &[u8; 9]>($($arg),*),
            (Width::Fixed(24), Width::Fixed(0)) => $f::<&[u8; 24], ()>($($arg),*),
            (Width::Fixed(24), Width::Fixed(8)) => $f::<&[u8; 24], &[u8; 8]>($($arg),*),
            (Width::Fixed(32), Width::Fixed(0)) => $f::<&[u8; 32], ()>($($arg),*),
            (Width::Var, Width::Var) => $f::<&[u8], &[u8]>($($arg),*),
            (Width::Var, Width::Fixed(2)) => $f::<&[u8], &[u8; 2]>($($arg),*),
            (Width::Var, Width::Fixed(4)) => $f::<&[u8], &[u8; 4]>($($arg),*),
            (Width::Var, Width::Fixed(8)) => $f::<&[u8], &[u8; 8]>($($arg),*),
            (k, v) => unreachable!("table {} has unsupported layout {k:?} -> {v:?}", $t.name),
        }
    };
}

fn def<K: redb::Key + 'static, V: redb::Value + 'static>(
    t: &'static TableDef,
) -> TableDefinition<'static, K, V> {
    TableDefinition::new(t.name)
}

fn scan_table<K: redb::Key + 'static, V: redb::Value + 'static, T: ReadableTable<K, V>>(
    table: &T,
    t: &'static TableDef,
    lo: Bound<&[u8]>,
    hi: Bound<&[u8]>,
    rev: bool,
    limit: usize,
) -> Result<Vec<Entry>> {
    let lo = pad(lo, t.key, false);
    let hi = pad(hi, t.key, true);
    // An empty interval would make redb panic on an inverted range.
    if let (Bound::Included(a) | Bound::Excluded(a), Bound::Included(b) | Bound::Excluded(b)) =
        (&lo, &hi)
    {
        let empty = a > b
            || (a == b && !(matches!(lo, Bound::Included(_)) && matches!(hi, Bound::Included(_))));
        if empty {
            return Ok(Vec::new());
        }
    }
    let range = table
        .range::<K::SelfType<'_>>((borrow::<K>(&lo), borrow::<K>(&hi)))
        .map_err(map_storage)?;
    let mut out = Vec::new();
    let mut push = |r: std::result::Result<
        (redb::AccessGuard<'_, K>, redb::AccessGuard<'_, V>),
        redb::StorageError,
    >|
     -> Result<()> {
        let (k, v) = r.map_err(map_storage)?;
        out.push((kbytes::<K>(&k.value()), vbytes::<V>(&v.value())));
        Ok(())
    };
    if rev {
        for r in range.rev().take(limit) {
            push(r)?;
        }
    } else {
        for r in range.take(limit) {
            push(r)?;
        }
    }
    Ok(out)
}

fn read_get<K: redb::Key + 'static, V: redb::Value + 'static>(
    txn: &redb::ReadTransaction,
    t: &'static TableDef,
    key: &[u8],
) -> Result<Option<Vec<u8>>> {
    let table = match txn.open_table(def::<K, V>(t)) {
        Ok(tb) => tb,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(e) => return Err(map_table(e)),
    };
    let got = table.get(K::from_bytes(key)).map_err(map_storage)?;
    Ok(got.map(|g| vbytes::<V>(&g.value())))
}

fn read_scan<K: redb::Key + 'static, V: redb::Value + 'static>(
    txn: &redb::ReadTransaction,
    t: &'static TableDef,
    lo: Bound<&[u8]>,
    hi: Bound<&[u8]>,
    rev: bool,
    limit: usize,
) -> Result<Vec<Entry>> {
    let table = match txn.open_table(def::<K, V>(t)) {
        Ok(tb) => tb,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
        Err(e) => return Err(map_table(e)),
    };
    scan_table::<K, V, _>(&table, t, lo, hi, rev, limit)
}

fn write_get<K: redb::Key + 'static, V: redb::Value + 'static>(
    txn: &redb::WriteTransaction,
    t: &'static TableDef,
    key: &[u8],
) -> Result<Option<Vec<u8>>> {
    let table = txn.open_table(def::<K, V>(t)).map_err(map_table)?;
    let got = table.get(K::from_bytes(key)).map_err(map_storage)?;
    Ok(got.map(|g| vbytes::<V>(&g.value())))
}

fn write_scan<K: redb::Key + 'static, V: redb::Value + 'static>(
    txn: &redb::WriteTransaction,
    t: &'static TableDef,
    lo: Bound<&[u8]>,
    hi: Bound<&[u8]>,
    rev: bool,
    limit: usize,
) -> Result<Vec<Entry>> {
    let table = txn.open_table(def::<K, V>(t)).map_err(map_table)?;
    scan_table::<K, V, _>(&table, t, lo, hi, rev, limit)
}

fn write_put<K: redb::Key + 'static, V: redb::Value + 'static>(
    txn: &redb::WriteTransaction,
    t: &'static TableDef,
    key: &[u8],
    value: &[u8],
) -> Result<()> {
    let mut table = txn.open_table(def::<K, V>(t)).map_err(map_table)?;
    table
        .insert(K::from_bytes(key), V::from_bytes(value))
        .map_err(map_storage)?;
    Ok(())
}

fn write_del<K: redb::Key + 'static, V: redb::Value + 'static>(
    txn: &redb::WriteTransaction,
    t: &'static TableDef,
    key: &[u8],
) -> Result<bool> {
    let mut table = txn.open_table(def::<K, V>(t)).map_err(map_table)?;
    let old = table.remove(K::from_bytes(key)).map_err(map_storage)?;
    Ok(old.is_some())
}

/// A read snapshot.
pub struct RedbRead(redb::ReadTransaction);

impl std::fmt::Debug for RedbRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RedbRead")
    }
}

impl KvRead for RedbRead {
    fn get(&self, t: &'static TableDef, key: &[u8]) -> Result<Option<Vec<u8>>> {
        t.check(key, None);
        dispatch!(t, read_get(&self.0, t, key))
    }
    fn scan(
        &self,
        t: &'static TableDef,
        lo: Bound<&[u8]>,
        hi: Bound<&[u8]>,
        rev: bool,
        limit: usize,
    ) -> Result<Vec<Entry>> {
        dispatch!(t, read_scan(&self.0, t, lo, hi, rev, limit))
    }
}

/// An engine write transaction (one group-commit batch, [[adr:0025]]).
pub struct RedbWrite(redb::WriteTransaction);

impl std::fmt::Debug for RedbWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RedbWrite")
    }
}

impl KvRead for RedbWrite {
    fn get(&self, t: &'static TableDef, key: &[u8]) -> Result<Option<Vec<u8>>> {
        t.check(key, None);
        dispatch!(t, write_get(&self.0, t, key))
    }
    fn scan(
        &self,
        t: &'static TableDef,
        lo: Bound<&[u8]>,
        hi: Bound<&[u8]>,
        rev: bool,
        limit: usize,
    ) -> Result<Vec<Entry>> {
        dispatch!(t, write_scan(&self.0, t, lo, hi, rev, limit))
    }
}

impl KvWrite for RedbWrite {
    fn as_read(&self) -> &dyn KvRead {
        self
    }
    fn put(&mut self, t: &'static TableDef, key: &[u8], value: &[u8]) -> Result<()> {
        t.check(key, Some(value));
        dispatch!(t, write_put(&self.0, t, key, value))
    }
    fn del(&mut self, t: &'static TableDef, key: &[u8]) -> Result<bool> {
        t.check(key, None);
        dispatch!(t, write_del(&self.0, t, key))
    }
}

/// Options for opening the engine.
#[derive(Debug, Clone)]
pub struct EngineOptions {
    pub cache_bytes: usize,
    /// Save the allocator state with every durable commit so that opening
    /// after a crash skips the full repair (redb quick repair).
    pub quick_repair: bool,
    /// Fault injection for the crash harness; `None` in production.
    pub fault: Option<super::fault::FaultPlan>,
}

/// The redb engine for one database file.
///
/// The redb handle can be released and reacquired while the owner process
/// keeps its `Database` (suspend and resume, [[16-api#Mobile bindings]]):
/// releasing drops the handle and with it the OS file lock.
pub struct RedbStorage {
    db: std::sync::RwLock<Option<redb::Database>>,
    /// The backend under `db`, shared so a snapshot export can read the file
    /// through the handle that holds the OS lock (on Windows, any other handle
    /// is refused). Dropped together with `db` on release.
    backend: std::sync::RwLock<Option<std::sync::Arc<dyn redb::StorageBackend>>>,
    path: std::path::PathBuf,
    opts: EngineOptions,
    reopens: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for RedbStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedbStorage")
            .field("path", &self.path)
            .finish()
    }
}

impl RedbStorage {
    /// Opens or creates the file. The OS lock is redb's `flock` / `LockFile`
    /// on the file, held for the life of the handle (rule `api-single-owner`).
    pub fn open(path: &Path, opts: &EngineOptions) -> Result<Self> {
        let (db, backend) = Self::open_db(path, opts)?;
        Ok(RedbStorage {
            db: std::sync::RwLock::new(Some(db)),
            backend: std::sync::RwLock::new(Some(backend)),
            path: path.to_path_buf(),
            opts: opts.clone(),
            reopens: std::sync::atomic::AtomicU64::new(0),
        })
    }

    fn open_db(
        path: &Path,
        opts: &EngineOptions,
    ) -> Result<(redb::Database, std::sync::Arc<dyn redb::StorageBackend>)> {
        let mut builder = redb::Builder::new();
        builder
            .set_cache_size(opts.cache_bytes)
            .create_with_file_format_v3(true);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let backend = redb::backends::FileBackend::new(file).map_err(map_db)?;
        let shared: std::sync::Arc<dyn redb::StorageBackend> = match &opts.fault {
            None => std::sync::Arc::new(backend),
            Some(plan) => {
                std::sync::Arc::new(super::fault::FaultBackend::new(backend, plan.clone()))
            }
        };
        let db = builder
            .create_with_backend(SharedBackend(shared.clone()))
            .map_err(map_db)?;
        Ok((db, shared))
    }

    fn handle(&self) -> std::sync::RwLockReadGuard<'_, Option<redb::Database>> {
        self.db.read().unwrap_or_else(|p| p.into_inner())
    }

    /// Drops the redb handle, releasing the OS file lock. The caller ensures
    /// that no engine transaction is open. Idempotent.
    pub fn release(&self) {
        let db = self.db.write().unwrap_or_else(|p| p.into_inner()).take();
        drop(db);
        // The last reference: dropping it closes the file and its lock.
        let backend = self
            .backend
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        drop(backend);
    }

    /// Opens the file again after [`RedbStorage::release`]; a no-op while the
    /// handle is held. Fails with `Locked` when another owner took the file.
    pub fn reacquire(&self) -> Result<()> {
        let mut g = self.db.write().unwrap_or_else(|p| p.into_inner());
        if g.is_none() {
            let (db, backend) = Self::open_db(&self.path, &self.opts)?;
            *self.backend.write().unwrap_or_else(|p| p.into_inner()) = Some(backend);
            *g = Some(db);
            self.reopens
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }

    /// Whether the handle (and the file lock) is currently released.
    pub fn is_released(&self) -> bool {
        self.handle().is_none()
    }

    /// How often the file was reopened by [`RedbStorage::reacquire`].
    pub fn reopens(&self) -> u64 {
        self.reopens.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Copies the file, byte for byte, to `to` by reading through the open
    /// backend. The caller keeps writers out and has made every commit
    /// durable, so the file is quiescent.
    pub fn copy_to(&self, to: &Path) -> Result<()> {
        use std::io::Write;
        const CHUNK: u64 = 4 << 20;
        let g = self.backend.read().unwrap_or_else(|p| p.into_inner());
        let backend = g.as_ref().ok_or_else(released)?;
        let len = backend.len()?;
        let mut out = std::fs::File::create(to)?;
        let mut off = 0;
        while off < len {
            let n = CHUNK.min(len - off);
            out.write_all(&backend.read(off, n as usize)?)?;
            off += n;
        }
        out.sync_all()?;
        Ok(())
    }
}

/// A [`redb::StorageBackend`] shared between redb and [`RedbStorage::copy_to`].
#[derive(Debug)]
struct SharedBackend(std::sync::Arc<dyn redb::StorageBackend>);

impl redb::StorageBackend for SharedBackend {
    fn len(&self) -> std::result::Result<u64, std::io::Error> {
        self.0.len()
    }
    fn read(&self, offset: u64, len: usize) -> std::result::Result<Vec<u8>, std::io::Error> {
        self.0.read(offset, len)
    }
    fn set_len(&self, len: u64) -> std::result::Result<(), std::io::Error> {
        self.0.set_len(len)
    }
    fn sync_data(&self, eventual: bool) -> std::result::Result<(), std::io::Error> {
        self.0.sync_data(eventual)
    }
    fn write(&self, offset: u64, data: &[u8]) -> std::result::Result<(), std::io::Error> {
        self.0.write(offset, data)
    }
}

fn released() -> Error {
    Error::Io(std::io::Error::other(
        "the database file is released (suspended); resume first",
    ))
}

impl Storage for RedbStorage {
    type Read = RedbRead;
    type Write = RedbWrite;

    fn begin_read(&self) -> Result<RedbRead> {
        match self.handle().as_ref() {
            Some(db) => db.begin_read().map(RedbRead).map_err(map_tx),
            None => Err(released()),
        }
    }

    fn begin_write(&self) -> Result<RedbWrite> {
        match self.handle().as_ref() {
            Some(db) => db.begin_write().map(RedbWrite).map_err(map_tx),
            None => Err(released()),
        }
    }

    fn commit(&self, w: RedbWrite, durability: EngineDurability) -> Result<()> {
        let mut txn = w.0;
        if self.opts.quick_repair && matches!(durability, EngineDurability::Flush) {
            txn.set_quick_repair(true);
        }
        txn.set_durability(match durability {
            EngineDurability::Flush => redb::Durability::Immediate,
            EngineDurability::NoFlush => redb::Durability::None,
        });
        txn.commit().map_err(map_commit)
    }

    fn abort(&self, w: RedbWrite) -> Result<()> {
        w.0.abort().map_err(map_storage)
    }

    fn file_size(&self) -> Result<u64> {
        Ok(std::fs::metadata(&self.path)?.len())
    }
}
