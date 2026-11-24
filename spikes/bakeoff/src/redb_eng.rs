//! redb adapter. One macro instantiated for redb 2.6.4 (the shipping pin) and, for comparison
//! only, redb 4.6.0. Fixed-width tables use `&[u8; N]` (memcmp order, no per-entry length).

macro_rules! redb_adapter {
    ($modname:ident, $redb:ident, $label:expr $(, $extra:ident)*) => {
        pub mod $modname {
            #[allow(unused_imports)]
            use $redb::{$($extra),*};
            use crate::engine::*;
            use $redb::{
                Builder, Database, Durability, ReadOnlyTable, ReadTransaction, ReadableTable,
                Table, TableDefinition, WriteTransaction,
            };
            use std::cell::RefCell;
            use std::path::{Path, PathBuf};

            type K24 = &'static [u8; 24];
            type K8 = &'static [u8; 8];
            type V26 = &'static [u8; 26];
            type VB = &'static [u8];

            const ORD: [TableDefinition<'static, K24, K8>; 6] = [
                TableDefinition::new("spo"),
                TableDefinition::new("pos"),
                TableDefinition::new("osp"),
                TableDefinition::new("pso"),
                TableDefinition::new("sop"),
                TableDefinition::new("ops"),
            ];
            const STMT: TableDefinition<'static, K8, V26> = TableDefinition::new("stmt");
            const PROPS: TableDefinition<'static, K8, VB> = TableDefinition::new("props");

            pub struct RedbEngine {
                pub db: Database,
                path: PathBuf,
                pub quick_repair: bool,
                cache: usize,
            }

            impl RedbEngine {
                pub fn open(path: &Path, cache_bytes: usize, tables: &[Tab]) -> Self {
                    let db = Builder::new()
                        .set_cache_size(cache_bytes)
                        .create(path)
                        .expect("redb open");
                    let e = RedbEngine {
                        db,
                        path: path.to_path_buf(),
                        quick_repair: false,
                        cache: cache_bytes,
                    };
                    // make sure the requested tables exist
                    let mut w = e.begin_write();
                    for t in tables {
                        w.ensure(*t);
                    }
                    w.commit(Dur::Flush);
                    e
                }
            }

            pub struct R {
                txn: ReadTransaction,
                ord: [RefCell<Option<Option<ReadOnlyTable<K24, K8>>>>; 6],
                stmt: RefCell<Option<Option<ReadOnlyTable<K8, V26>>>>,
                props: RefCell<Option<Option<ReadOnlyTable<K8, VB>>>>,
            }

            impl R {
                fn with_ord<T>(&self, i: usize, f: impl FnOnce(Option<&ReadOnlyTable<K24, K8>>) -> T) -> T {
                    let mut c = self.ord[i].borrow_mut();
                    if c.is_none() {
                        *c = Some(self.txn.open_table(ORD[i]).ok());
                    }
                    f(c.as_ref().unwrap().as_ref())
                }
                fn with_stmt<T>(&self, f: impl FnOnce(Option<&ReadOnlyTable<K8, V26>>) -> T) -> T {
                    let mut c = self.stmt.borrow_mut();
                    if c.is_none() {
                        *c = Some(self.txn.open_table(STMT).ok());
                    }
                    f(c.as_ref().unwrap().as_ref())
                }
                fn with_props<T>(&self, f: impl FnOnce(Option<&ReadOnlyTable<K8, VB>>) -> T) -> T {
                    let mut c = self.props.borrow_mut();
                    if c.is_none() {
                        *c = Some(self.txn.open_table(PROPS).ok());
                    }
                    f(c.as_ref().unwrap().as_ref())
                }
            }

            fn a24(k: &[u8]) -> &[u8; 24] {
                k.try_into().unwrap()
            }
            fn a8(k: &[u8]) -> &[u8; 8] {
                k.try_into().unwrap()
            }

            macro_rules! do_get {
                ($tbl:expr, $k:expr, $out:expr) => {{
                    match $tbl {
                        None => false,
                        Some(t) => match t.get($k).unwrap() {
                            Some(g) => {
                                $out.clear();
                                $out.extend_from_slice(&g.value()[..]);
                                true
                            }
                            None => false,
                        },
                    }
                }};
            }
            macro_rules! do_scan {
                ($tbl:expr, $lo:expr, $hi:expr, $f:expr, $kt:ty) => {{
                    if let Some(t) = $tbl {
                        for r in t.range::<$kt>($lo..=$hi).unwrap() {
                            let (k, v) = r.unwrap();
                            if !$f(&k.value()[..], &v.value()[..]) {
                                break;
                            }
                        }
                    }
                }};
            }

            impl ReadOps for R {
                fn get(&self, t: Tab, k: &[u8], out: &mut Vec<u8>) -> bool {
                    match t {
                        Tab::Stmt => self.with_stmt(|tb| do_get!(tb, a8(k), out)),
                        Tab::Props => self.with_props(|tb| do_get!(tb, a8(k), out)),
                        _ => self.with_ord(t.idx(), |tb| do_get!(tb, a24(k), out)),
                    }
                }
                fn scan(&self, t: Tab, lo: &[u8], hi: &[u8], f: &mut dyn FnMut(&[u8], &[u8]) -> bool) {
                    match t {
                        Tab::Stmt => self.with_stmt(|tb| do_scan!(tb, a8(lo), a8(hi), f, &[u8; 8])),
                        Tab::Props => self.with_props(|tb| do_scan!(tb, a8(lo), a8(hi), f, &[u8; 8])),
                        _ => self.with_ord(t.idx(), |tb| do_scan!(tb, a24(lo), a24(hi), f, &[u8; 24])),
                    }
                }
            }

            /// Write transaction. Tables borrow the boxed transaction; their lifetime is
            /// erased to 'static and they are always dropped before the transaction.
            pub struct W {
                ord: [RefCell<Option<Table<'static, K24, K8>>>; 6],
                stmt: RefCell<Option<Table<'static, K8, V26>>>,
                props: RefCell<Option<Table<'static, K8, VB>>>,
                txn: Option<Box<WriteTransaction>>,
            }

            impl W {
                fn txn_static(&self) -> &'static WriteTransaction {
                    unsafe { &*(&**self.txn.as_ref().unwrap() as *const WriteTransaction) }
                }
                fn ord_tab(&self, i: usize) -> std::cell::RefMut<'_, Option<Table<'static, K24, K8>>> {
                    let mut c = self.ord[i].borrow_mut();
                    if c.is_none() {
                        *c = Some(self.txn_static().open_table(ORD[i]).unwrap());
                    }
                    c
                }
                fn stmt_tab(&self) -> std::cell::RefMut<'_, Option<Table<'static, K8, V26>>> {
                    let mut c = self.stmt.borrow_mut();
                    if c.is_none() {
                        *c = Some(self.txn_static().open_table(STMT).unwrap());
                    }
                    c
                }
                fn props_tab(&self) -> std::cell::RefMut<'_, Option<Table<'static, K8, VB>>> {
                    let mut c = self.props.borrow_mut();
                    if c.is_none() {
                        *c = Some(self.txn_static().open_table(PROPS).unwrap());
                    }
                    c
                }
                pub fn ensure(&mut self, t: Tab) {
                    match t {
                        Tab::Stmt => drop(self.stmt_tab()),
                        Tab::Props => drop(self.props_tab()),
                        _ => drop(self.ord_tab(t.idx())),
                    }
                }
                fn close_tables(&mut self) {
                    for c in &self.ord {
                        c.borrow_mut().take();
                    }
                    self.stmt.borrow_mut().take();
                    self.props.borrow_mut().take();
                }
                /// Raw access for redb-specific experiments (savepoints).
                pub fn raw(&self) -> &WriteTransaction {
                    self.txn.as_ref().unwrap()
                }
                pub fn commit_with(mut self, d: Durability) {
                    self.close_tables();
                    let mut txn = self.txn.take().unwrap();
                    #[allow(clippy::let_unit_value)]
                    let _ = txn.set_durability(d);
                    txn.commit().unwrap();
                }
            }

            impl ReadOps for W {
                fn get(&self, t: Tab, k: &[u8], out: &mut Vec<u8>) -> bool {
                    match t {
                        Tab::Stmt => do_get!(self.stmt_tab().as_ref(), a8(k), out),
                        Tab::Props => do_get!(self.props_tab().as_ref(), a8(k), out),
                        _ => do_get!(self.ord_tab(t.idx()).as_ref(), a24(k), out),
                    }
                }
                fn scan(&self, t: Tab, lo: &[u8], hi: &[u8], f: &mut dyn FnMut(&[u8], &[u8]) -> bool) {
                    match t {
                        Tab::Stmt => do_scan!(self.stmt_tab().as_ref(), a8(lo), a8(hi), f, &[u8; 8]),
                        Tab::Props => do_scan!(self.props_tab().as_ref(), a8(lo), a8(hi), f, &[u8; 8]),
                        _ => do_scan!(self.ord_tab(t.idx()).as_ref(), a24(lo), a24(hi), f, &[u8; 24]),
                    }
                }
            }

            impl WriteOps for W {
                fn insert(&mut self, t: Tab, k: &[u8], v: &[u8]) {
                    match t {
                        Tab::Stmt => {
                            let v: &[u8; 26] = v.try_into().unwrap();
                            self.stmt_tab().as_mut().unwrap().insert(a8(k), v).unwrap();
                        }
                        Tab::Props => {
                            self.props_tab().as_mut().unwrap().insert(a8(k), v).unwrap();
                        }
                        _ => {
                            self.ord_tab(t.idx()).as_mut().unwrap().insert(a24(k), a8(v)).unwrap();
                        }
                    }
                }
                fn remove(&mut self, t: Tab, k: &[u8]) -> bool {
                    match t {
                        Tab::Stmt => self.stmt_tab().as_mut().unwrap().remove(a8(k)).unwrap().is_some(),
                        Tab::Props => self.props_tab().as_mut().unwrap().remove(a8(k)).unwrap().is_some(),
                        _ => self.ord_tab(t.idx()).as_mut().unwrap().remove(a24(k)).unwrap().is_some(),
                    }
                }
                fn commit(self, d: Dur) {
                    self.commit_with(match d {
                        Dur::Flush => Durability::Immediate,
                        Dur::NoFlush => Durability::None,
                    })
                }
            }

            impl Drop for W {
                fn drop(&mut self) {
                    // abort path: tables first, then the transaction (field order)
                    self.close_tables();
                }
            }

            impl Engine for RedbEngine {
                type R<'a> = R;
                type W<'a> = W;
                fn name(&self) -> String {
                    $label.to_string()
                }
                fn begin_read(&self) -> R {
                    R {
                        txn: self.db.begin_read().unwrap(),
                        ord: Default::default(),
                        stmt: RefCell::new(None),
                        props: RefCell::new(None),
                    }
                }
                fn begin_write(&self) -> W {
                    let mut txn = self.db.begin_write().unwrap();
                    if self.quick_repair {
                        txn.set_quick_repair(true);
                    }
                    W {
                        ord: Default::default(),
                        stmt: RefCell::new(None),
                        props: RefCell::new(None),
                        txn: Some(Box::new(txn)),
                    }
                }
                fn files(&self) -> Vec<PathBuf> {
                    vec![self.path.clone()]
                }
                fn info(&self) -> serde_json::Value {
                    serde_json::json!({"cache_bytes": self.cache, "quick_repair": self.quick_repair,
                        "page_size": 4096, "key_types": "&[u8;24]->&[u8;8], &[u8;8]->&[u8;26], &[u8;8]->&[u8]"})
                }
            }
        }
    };
}

redb_adapter!(v2, redb, "redb-2.6.4");
redb_adapter!(v4, redb4, "redb-4.6.0", ReadableDatabase);
