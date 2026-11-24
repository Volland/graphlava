//! Sanakirja 1.4.3 adapter (vendored with a one-flag patch: `SKIP_MSYNC`).
//! Sanakirja B-trees are multimaps over (key, value) pairs, so map-semantics `insert` is
//! `del(k) + put(k, v)`. Durability: stock `commit()` always msyncs (MS_SYNC), which is
//! not a device-cache flush on macOS. Here NoFlush = commit with msync skipped; Flush =
//! stock commit (msync) + fcntl(F_FULLFSYNC) on the data file, to match redb/mdbx.

use crate::engine::*;
use sanakirja::btree::{self, page_unsized, Db, Db_};
use sanakirja::{direct_repr, Commit, Env, MutTxn, RootDb, Storable, Txn, UnsizedStorable};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(C)]
pub struct K24(pub [u8; 24]);
direct_repr!(K24);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(C)]
pub struct K8(pub [u8; 8]);
direct_repr!(K8);
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(C)]
pub struct V26(pub [u8; 26]);
direct_repr!(V26);

type OrdDb = Db<K24, K8>;
type StmtDb = Db<K8, V26>;
type PropsDb = Db_<K8, [u8], page_unsized::Page<K8, [u8]>>;

pub struct SanaEngine {
    env: Env,
    path: PathBuf,
    fd_file: std::fs::File,
}

impl SanaEngine {
    pub fn open(path: &Path, _tables: &[Tab]) -> Self {
        let env = Env::new(path, 1 << 30, 4).expect("sanakirja open");
        let fd_file = std::fs::OpenOptions::new().read(true).write(true).open(path).unwrap();
        let e = SanaEngine { env, path: path.to_path_buf(), fd_file };
        // create all trees once
        let mut w = e.begin_write();
        w.touch_all();
        w.commit(Dur::Flush);
        e
    }
}

fn k24(k: &[u8]) -> K24 {
    K24(k.try_into().unwrap())
}
fn k8(k: &[u8]) -> K8 {
    K8(k.try_into().unwrap())
}

pub struct R<'a> {
    txn: Txn<&'a Env>,
}

fn rget<T: sanakirja::LoadPage>(txn: &T, t: Tab, k: &[u8], out: &mut Vec<u8>, root: impl Fn(usize) -> Option<u64>) -> bool
where
    T::Error: std::fmt::Debug,
{
    let Some(r) = root(t.idx()) else { return false };
    match t {
        Tab::Stmt => {
            let db: StmtDb = unsafe { Db_::from_page(r) };
            let key = k8(k);
            match btree::get(txn, &db, &key, None).unwrap() {
                Some((kk, v)) if *kk == key => {
                    out.clear();
                    out.extend_from_slice(&v.0);
                    true
                }
                _ => false,
            }
        }
        Tab::Props => {
            let db: PropsDb = unsafe { Db_::from_page(r) };
            let key = k8(k);
            match btree::get(txn, &db, &key, None).unwrap() {
                Some((kk, v)) if *kk == key => {
                    out.clear();
                    out.extend_from_slice(v);
                    true
                }
                _ => false,
            }
        }
        _ => {
            let db: OrdDb = unsafe { Db_::from_page(r) };
            let key = k24(k);
            match btree::get(txn, &db, &key, None).unwrap() {
                Some((kk, v)) if *kk == key => {
                    out.clear();
                    out.extend_from_slice(&v.0);
                    true
                }
                _ => false,
            }
        }
    }
}

fn rscan<T: sanakirja::LoadPage>(txn: &T, t: Tab, lo: &[u8], hi: &[u8], f: &mut dyn FnMut(&[u8], &[u8]) -> bool, root: impl Fn(usize) -> Option<u64>)
where
    T::Error: std::fmt::Debug,
{
    let Some(r) = root(t.idx()) else { return };
    match t {
        Tab::Stmt => {
            let db: StmtDb = unsafe { Db_::from_page(r) };
            for e in btree::iter(txn, &db, Some((&k8(lo), None))).unwrap() {
                let (k, v) = e.unwrap();
                if &k.0[..] > hi || !f(&k.0, &v.0) {
                    break;
                }
            }
        }
        Tab::Props => {
            let db: PropsDb = unsafe { Db_::from_page(r) };
            for e in btree::iter(txn, &db, Some((&k8(lo), None))).unwrap() {
                let (k, v) = e.unwrap();
                if &k.0[..] > hi || !f(&k.0, v) {
                    break;
                }
            }
        }
        _ => {
            let db: OrdDb = unsafe { Db_::from_page(r) };
            for e in btree::iter(txn, &db, Some((&k24(lo), None))).unwrap() {
                let (k, v) = e.unwrap();
                if &k.0[..] > hi || !f(&k.0, &v.0) {
                    break;
                }
            }
        }
    }
}

impl<'a> ReadOps for R<'a> {
    fn get(&self, t: Tab, k: &[u8], out: &mut Vec<u8>) -> bool {
        rget(&self.txn, t, k, out, |i| self.txn.root_db::<K8, K8, btree::page::Page<K8, K8>>(i).map(|d| d.db.into()))
    }
    fn scan(&self, t: Tab, lo: &[u8], hi: &[u8], f: &mut dyn FnMut(&[u8], &[u8]) -> bool) {
        rscan(&self.txn, t, lo, hi, f, |i| self.txn.root_db::<K8, K8, btree::page::Page<K8, K8>>(i).map(|d| d.db.into()))
    }
}

pub struct W<'a> {
    txn: MutTxn<&'a Env, ()>,
    roots: [Option<u64>; 8],
    dirty: [bool; 8],
    e: &'a SanaEngine,
}

impl<'a> W<'a> {
    fn touch_all(&mut self) {
        for t in Tab::ALL {
            self.ensure(t);
        }
    }
    fn ensure(&mut self, t: Tab) -> u64 {
        let i = t.idx();
        if let Some(r) = self.roots[i] {
            return r;
        }
        let r: u64 = unsafe {
            match t {
                Tab::Stmt => btree::create_db_::<_, K8, V26, btree::page::Page<K8, V26>>(&mut self.txn).unwrap().db.into(),
                Tab::Props => btree::create_db_::<_, K8, [u8], page_unsized::Page<K8, [u8]>>(&mut self.txn).unwrap().db.into(),
                _ => btree::create_db_::<_, K24, K8, btree::page::Page<K24, K8>>(&mut self.txn).unwrap().db.into(),
            }
        };
        self.roots[i] = Some(r);
        self.dirty[i] = true;
        r
    }
}

impl<'a> ReadOps for W<'a> {
    fn get(&self, t: Tab, k: &[u8], out: &mut Vec<u8>) -> bool {
        rget(&self.txn, t, k, out, |i| self.roots[i])
    }
    fn scan(&self, t: Tab, lo: &[u8], hi: &[u8], f: &mut dyn FnMut(&[u8], &[u8]) -> bool) {
        rscan(&self.txn, t, lo, hi, f, |i| self.roots[i])
    }
}

impl<'a> WriteOps for W<'a> {
    fn insert(&mut self, t: Tab, k: &[u8], v: &[u8]) {
        let r = self.ensure(t);
        let new: u64 = match t {
            Tab::Stmt => {
                let mut db: StmtDb = unsafe { Db_::from_page(r) };
                let key = k8(k);
                btree::del(&mut self.txn, &mut db, &key, None).unwrap();
                btree::put(&mut self.txn, &mut db, &key, &V26(v.try_into().unwrap())).unwrap();
                db.db.into()
            }
            Tab::Props => {
                let mut db: PropsDb = unsafe { Db_::from_page(r) };
                let key = k8(k);
                btree::del(&mut self.txn, &mut db, &key, None).unwrap();
                btree::put(&mut self.txn, &mut db, &key, v).unwrap();
                db.db.into()
            }
            _ => {
                let mut db: OrdDb = unsafe { Db_::from_page(r) };
                let key = k24(k);
                btree::del(&mut self.txn, &mut db, &key, None).unwrap();
                btree::put(&mut self.txn, &mut db, &key, &K8(v.try_into().unwrap())).unwrap();
                db.db.into()
            }
        };
        self.roots[t.idx()] = Some(new);
        self.dirty[t.idx()] = true;
    }
    fn remove(&mut self, t: Tab, k: &[u8]) -> bool {
        let r = self.ensure(t);
        let (found, new): (bool, u64) = match t {
            Tab::Stmt => {
                let mut db: StmtDb = unsafe { Db_::from_page(r) };
                let f = btree::del(&mut self.txn, &mut db, &k8(k), None).unwrap();
                (f, db.db.into())
            }
            Tab::Props => {
                let mut db: PropsDb = unsafe { Db_::from_page(r) };
                let f = btree::del(&mut self.txn, &mut db, &k8(k), None).unwrap();
                (f, db.db.into())
            }
            _ => {
                let mut db: OrdDb = unsafe { Db_::from_page(r) };
                let f = btree::del(&mut self.txn, &mut db, &k24(k), None).unwrap();
                (f, db.db.into())
            }
        };
        self.roots[t.idx()] = Some(new);
        self.dirty[t.idx()] = true;
        found
    }
    fn commit(mut self, d: Dur) {
        for i in 0..8 {
            if self.dirty[i] {
                self.txn.set_root(i, self.roots[i].unwrap());
            }
        }
        sanakirja::SKIP_MSYNC.store(d == Dur::NoFlush, Ordering::Relaxed);
        let e = self.e;
        self.txn.commit().unwrap();
        if d == Dur::Flush {
            let rc = unsafe { libc::fcntl(e.fd_file.as_raw_fd(), libc::F_FULLFSYNC) };
            assert!(rc != -1, "F_FULLFSYNC failed");
        }
    }
}

impl Engine for SanaEngine {
    type R<'a> = R<'a>;
    type W<'a> = W<'a>;
    fn name(&self) -> String {
        "sanakirja-1.4.3".into()
    }
    fn begin_read(&self) -> R<'_> {
        R { txn: Env::txn_begin(&self.env).unwrap() }
    }
    fn begin_write(&self) -> W<'_> {
        let txn = Env::mut_txn_begin(&self.env).unwrap();
        let mut roots = [None; 8];
        for (i, r) in roots.iter_mut().enumerate() {
            *r = txn.root_db::<K8, K8, btree::page::Page<K8, K8>>(i).map(|d| d.db.into());
        }
        W { txn, roots, dirty: [false; 8], e: self }
    }
    fn files(&self) -> Vec<PathBuf> {
        let mut v = vec![self.path.clone()];
        for n in 0..4 {
            v.push(self.path.with_extension(format!("lock{}", n)));
        }
        v
    }
    fn info(&self) -> serde_json::Value {
        serde_json::json!({"page_size": 4096, "n_roots": 4, "insert": "del(k)+put(k,v)",
            "flush": "msync(MS_SYNC) + F_FULLFSYNC", "noflush": "patched: msync skipped"})
    }
}

#[allow(dead_code)]
fn _assert_storable() {
    fn s<T: Storable + ?Sized>() {}
    fn u<T: UnsizedStorable + ?Sized>() {}
    s::<K24>();
    u::<[u8]>();
}
