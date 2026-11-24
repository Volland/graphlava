//! Minimal storage trait for the bake-off (shape of tg.md/05-storage-trait.md "Trait API",
//! reduced to what the workload needs). Deviation from the spec: `range` takes a callback
//! instead of returning a cursor, which keeps the three adapters free of self-referential
//! iterator types. Bounds are inclusive and full-width.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Spo = 0,
    Pos = 1,
    Osp = 2,
    Pso = 3,
    Sop = 4,
    Ops = 5,
    Stmt = 6,
    Props = 7,
}

impl Tab {
    pub const ALL: [Tab; 8] = [
        Tab::Spo,
        Tab::Pos,
        Tab::Osp,
        Tab::Pso,
        Tab::Sop,
        Tab::Ops,
        Tab::Stmt,
        Tab::Props,
    ];
    pub const ORD: [Tab; 6] = [Tab::Spo, Tab::Pos, Tab::Osp, Tab::Pso, Tab::Sop, Tab::Ops];
    pub fn idx(self) -> usize {
        self as usize
    }
    pub fn name(self) -> &'static str {
        ["spo", "pos", "osp", "pso", "sop", "ops", "stmt", "props"][self as usize]
    }
    pub fn is_ordering(self) -> bool {
        (self as usize) < 6
    }
    /// Permute (s,p,o) into this ordering's component order.
    pub fn key(self, s: u64, p: u64, o: u64) -> [u8; 24] {
        let (a, b, c) = match self {
            Tab::Spo => (s, p, o),
            Tab::Pos => (p, o, s),
            Tab::Osp => (o, s, p),
            Tab::Pso => (p, s, o),
            Tab::Sop => (s, o, p),
            Tab::Ops => (o, p, s),
            _ => unreachable!(),
        };
        let mut k = [0u8; 24];
        k[0..8].copy_from_slice(&a.to_be_bytes());
        k[8..16].copy_from_slice(&b.to_be_bytes());
        k[16..24].copy_from_slice(&c.to_be_bytes());
        k
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dur {
    Flush,
    NoFlush,
}

pub trait ReadOps {
    /// Point get; copies the value into `out` (cleared first). Returns false if absent.
    fn get(&self, t: Tab, k: &[u8], out: &mut Vec<u8>) -> bool;
    /// Inclusive range scan [lo, hi] in key order; `f` returns false to stop.
    fn scan(&self, t: Tab, lo: &[u8], hi: &[u8], f: &mut dyn FnMut(&[u8], &[u8]) -> bool);
}

pub trait WriteOps: ReadOps {
    /// Upsert (map semantics: replaces an existing value).
    fn insert(&mut self, t: Tab, k: &[u8], v: &[u8]);
    fn remove(&mut self, t: Tab, k: &[u8]) -> bool;
    fn commit(self, d: Dur);
}

pub trait Engine: Send + Sync + 'static {
    type R<'a>: ReadOps
    where
        Self: 'a;
    type W<'a>: WriteOps
    where
        Self: 'a;
    fn name(&self) -> String;
    fn begin_read(&self) -> Self::R<'_>;
    fn begin_write(&self) -> Self::W<'_>;
    /// Data files belonging to this database (for size and cleanup).
    fn files(&self) -> Vec<PathBuf>;
    fn info(&self) -> serde_json::Value {
        serde_json::json!({})
    }
}

/// Prefix scan helper: fixed-width keys padded with 0x00 / 0xFF.
pub fn prefix_bounds(prefix: &[u8], width: usize) -> (Vec<u8>, Vec<u8>) {
    let mut lo = prefix.to_vec();
    lo.resize(width, 0x00);
    let mut hi = prefix.to_vec();
    hi.resize(width, 0xFF);
    (lo, hi)
}

/// (logical length, allocated bytes on disk) summed over files.
pub fn file_sizes(files: &[PathBuf]) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    let mut len = 0;
    let mut alloc = 0;
    for f in files {
        if let Ok(m) = std::fs::metadata(f) {
            len += m.len();
            alloc += m.blocks() * 512;
        }
    }
    (len, alloc)
}
