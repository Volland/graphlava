//! libmdbx adapter (crate `libmdbx` 0.9, C libmdbx from `mdbx-sys`).
//! Durability: the environment is opened in SAFE_NOSYNC mode; `commit(NoFlush)` is a plain
//! commit, `commit(Flush)` is commit + `mdbx_env_sync_ex(force=true)`, which on Apple issues
//! `fcntl(F_FULLFSYNC)` (mdbx.c osal_fsync, MDBX_APPLE_SPEED_INSTEADOF_DURABILITY=0 default).

use crate::engine::*;
use libmdbx::{
    Database, DatabaseOptions, Mode, NoWriteMap, ReadWriteOptions, SyncMode, Table, TableFlags,
    Transaction, WriteFlags, RO, RW,
};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// 0 = SAFE_NOSYNC (+ env_sync for Flush), 1 = UTTERLY_NOSYNC (+ env_sync for Flush),
/// 2 = DURABLE (every commit synced, NoFlush impossible).
pub static MDBX_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub struct MdbxEngine {
    db: Database<NoWriteMap>,
    dbis: Vec<Table<'static>>, // indexed by Tab::idx
    path: PathBuf,
}

impl MdbxEngine {
    pub fn open(dir: &Path, _tables: &[Tab]) -> Self {
        std::fs::create_dir_all(dir).unwrap();
        let opts = DatabaseOptions {
            max_tables: Some(16),
            max_readers: Some(256),
            mode: Mode::ReadWrite(ReadWriteOptions {
                sync_mode: match MDBX_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                    0 => SyncMode::SafeNoSync,
                    1 => SyncMode::UtterlyNoSync,
                    _ => SyncMode::Durable,
                },
                min_size: None,
                max_size: Some(512 << 30),
                growth_step: Some(256 << 20),
                shrink_threshold: None,
            }),
            page_size: std::env::var("MDBX_PAGE").ok().map(|v| libmdbx::PageSize::Set(v.parse().unwrap())),
            ..Default::default()
        };
        let db = Database::<NoWriteMap>::open_with_options(dir, opts).expect("mdbx open");
        let dbis = {
            let txn = db.begin_rw_txn().unwrap();
            for t in Tab::ALL {
                let tb = txn.create_table(Some(t.name()), TableFlags::default()).unwrap();
                txn.prime_for_permaopen(tb);
            }
            let (_, tabs) = txn.commit_and_rebind_open_dbs().unwrap();
            // commit_and_rebind_open_dbs returns handles in hash-set order; reopen by name
            drop(tabs);
            let txn = db.begin_ro_txn().unwrap();
            let mut v = Vec::new();
            for t in Tab::ALL {
                let tb = txn.open_table(Some(t.name())).unwrap();
                let tb: Table<'static> = unsafe { std::mem::transmute(tb) };
                v.push(tb);
            }
            // dbi handles stay valid for the environment's lifetime once a txn that opened
            // them commits; a read-only txn's open dbis are kept by libmdbx on txn end.
            txn.commit().unwrap();
            v
        };
        let ps = std::env::var("MDBX_PAGE").ok();
        let _ = ps;
        MdbxEngine { db, dbis, path: dir.to_path_buf() }
    }
    fn dbi(&self, t: Tab) -> &Table<'static> {
        &self.dbis[t.idx()]
    }
}

fn per_txn_dbi() -> bool {
    std::env::var("MDBX_DBI_PER_TXN").map(|v| v == "1").unwrap_or(false)
}

macro_rules! dbi_of {
    ($self:expr, $t:expr, $slot:ident) => {{
        if per_txn_dbi() {
            $slot = Some($self.txn.open_table(Some($t.name())).unwrap());
            $slot.as_ref().unwrap()
        } else {
            $self.e.dbi($t)
        }
    }};
}
impl MdbxEngine {
}

pub struct R<'a> {
    txn: Transaction<'a, RO, NoWriteMap>,
    e: &'a MdbxEngine,
}
pub struct W<'a> {
    txn: Transaction<'a, RW, NoWriteMap>,
    e: &'a MdbxEngine,
}

macro_rules! mdbx_read_impl {
    ($ty:ident) => {
        impl<'a> ReadOps for $ty<'a> {
            fn get(&self, t: Tab, k: &[u8], out: &mut Vec<u8>) -> bool {
                let mut slot: Option<Table<'_>> = None;
                let dbi = dbi_of!(self, t, slot);
                match self.txn.get::<Cow<[u8]>>(dbi, k).unwrap() {
                    Some(v) => {
                        out.clear();
                        out.extend_from_slice(&v);
                        true
                    }
                    None => false,
                }
            }
            fn scan(&self, t: Tab, lo: &[u8], hi: &[u8], f: &mut dyn FnMut(&[u8], &[u8]) -> bool) {
                let mut slot: Option<Table<'_>> = None;
                let dbi = dbi_of!(self, t, slot);
                let mut c = self.txn.cursor(dbi).unwrap();
                let mut cur = c.set_range::<Cow<[u8]>, Cow<[u8]>>(lo).unwrap();
                while let Some((k, v)) = cur {
                    if &k[..] > hi {
                        break;
                    }
                    if !f(&k, &v) {
                        break;
                    }
                    cur = c.next::<Cow<[u8]>, Cow<[u8]>>().unwrap();
                }
            }
        }
    };
}
mdbx_read_impl!(R);
mdbx_read_impl!(W);

impl<'a> WriteOps for W<'a> {
    fn insert(&mut self, t: Tab, k: &[u8], v: &[u8]) {
        let mut slot: Option<Table<'_>> = None;
        let dbi = dbi_of!(self, t, slot);
        self.txn.put(dbi, k, v, WriteFlags::UPSERT).unwrap();
    }
    fn remove(&mut self, t: Tab, k: &[u8]) -> bool {
        let mut slot: Option<Table<'_>> = None;
        let dbi = dbi_of!(self, t, slot);
        self.txn.del(dbi, k, None).unwrap()
    }
    fn commit(self, d: Dur) {
        let e = self.e;
        self.txn.commit().unwrap();
        if d == Dur::Flush && MDBX_MODE.load(std::sync::atomic::Ordering::Relaxed) != 2 {
            e.db.sync(true).unwrap();
        }
    }
}

impl Engine for MdbxEngine {
    type R<'a> = R<'a>;
    type W<'a> = W<'a>;
    fn name(&self) -> String {
        "libmdbx-0.9.0".into()
    }
    fn begin_read(&self) -> R<'_> {
        R { txn: self.db.begin_ro_txn().unwrap(), e: self }
    }
    fn begin_write(&self) -> W<'_> {
        W { txn: self.db.begin_rw_txn().unwrap(), e: self }
    }
    fn files(&self) -> Vec<PathBuf> {
        vec![self.path.join("mdbx.dat"), self.path.join("mdbx.lck")]
    }
    fn info(&self) -> serde_json::Value {
        let st = self.db.stat().unwrap();
        serde_json::json!({"page_size": st.page_size(), "sync_mode": (["SAFE_NOSYNC + env_sync(force) for Flush", "UTTERLY_NOSYNC + env_sync(force) for Flush", "DURABLE"][MDBX_MODE.load(std::sync::atomic::Ordering::Relaxed) as usize]),
            "write_map": false, "flush_call": "F_FULLFSYNC (libmdbx default on Apple)"})
    }
}
