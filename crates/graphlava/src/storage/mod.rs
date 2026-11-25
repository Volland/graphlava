//! The storage trait: the only boundary between graphlava and the KV engine
//! ([[05-storage-trait]]).
//!
//! Everything above this module talks to [`KvRead`] and [`KvWrite`] with byte
//! keys and values; no engine type crosses it (rule `st-no-engine-types`). Keys
//! compare as unsigned bytes (rule `st-order`), so composite keys are written
//! big-endian ([[04-physical-layout#Key byte order]]).
//!
//! Deviation from the sketch in the spec: reads return owned byte vectors and
//! scans are paged, instead of borrowed slices behind GAT cursors. This keeps
//! the trait object-safe so that the per-request overlay of [[adr:0025]] can sit
//! between the store logic and the engine. Costs per method are unchanged.

pub mod fault;
pub mod overlay;
pub mod redb_engine;

use std::ops::Bound;

use crate::error::Result;

/// Width of a key or value in a table: fixed widths let the engine store
/// entries without a length prefix (rule `pl-v1-plain`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Fixed(usize),
    Var,
}

/// A table: its name and the widths of its keys and values.
#[derive(Debug, PartialEq, Eq)]
pub struct TableDef {
    pub name: &'static str,
    pub key: Width,
    pub value: Width,
}

impl TableDef {
    pub(crate) fn check(&self, key: &[u8], value: Option<&[u8]>) {
        if let Width::Fixed(n) = self.key {
            debug_assert_eq!(key.len(), n, "key width of table {}", self.name);
        }
        if let (Width::Fixed(n), Some(v)) = (self.value, value) {
            debug_assert_eq!(v.len(), n, "value width of table {}", self.name);
        }
    }
}

/// One key-value pair, owned.
pub type Entry = (Vec<u8>, Vec<u8>);

/// Read access to one consistent view: a snapshot, a write transaction, or an
/// overlay on top of one.
pub trait KvRead {
    /// Point lookup. O(log n).
    fn get(&self, t: &'static TableDef, key: &[u8]) -> Result<Option<Vec<u8>>>;

    /// Up to `limit` entries in `[lo, hi]` in key order (descending when `rev`).
    /// O(log n + limit). A table that does not exist reads as empty.
    fn scan(
        &self,
        t: &'static TableDef,
        lo: Bound<&[u8]>,
        hi: Bound<&[u8]>,
        rev: bool,
        limit: usize,
    ) -> Result<Vec<Entry>>;

    /// The commit counter this view sees (rule `st-snapshot`).
    fn snapshot_id(&self) -> u64 {
        0
    }
}

/// Write access inside one write transaction (or one request overlay).
pub trait KvWrite: KvRead {
    /// This view as a reader. Explicit because trait upcasting needs Rust
    /// 1.86 and the MSRV is 1.85.
    fn as_read(&self) -> &dyn KvRead;
    /// Insert or replace. O(log n).
    fn put(&mut self, t: &'static TableDef, key: &[u8], value: &[u8]) -> Result<()>;
    /// Remove; returns whether the key existed. O(log n).
    fn del(&mut self, t: &'static TableDef, key: &[u8]) -> Result<bool>;
}

/// The two engine durability modes the user-facing levels are built on
/// (rule `st-durability-map`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineDurability {
    /// Returns only after the commit is durable on the device.
    Flush,
    /// Atomic and visible; durable no later than the next `Flush` commit.
    NoFlush,
}

/// The engine: one file, one writer, many snapshot readers
/// (rules `st-single-writer`, `st-snapshot`).
pub trait Storage: Send + Sync + 'static {
    type Read: KvRead + Send + 'static;
    type Write: KvWrite + Send + 'static;

    fn begin_read(&self) -> Result<Self::Read>;
    /// Blocks while another engine write transaction is open.
    fn begin_write(&self) -> Result<Self::Write>;
    fn commit(&self, w: Self::Write, durability: EngineDurability) -> Result<()>;
    fn abort(&self, w: Self::Write) -> Result<()>;
    fn file_size(&self) -> Result<u64>;
}

const PAGE: usize = 512;

/// Forward or backward iteration over a range, fetched in pages so that no
/// borrow of the engine outlives one call.
pub struct RangeIter<'a> {
    kv: &'a dyn KvRead,
    t: &'static TableDef,
    lo: Bound<Vec<u8>>,
    hi: Bound<Vec<u8>>,
    rev: bool,
    buf: std::vec::IntoIter<Entry>,
    done: bool,
}

impl std::fmt::Debug for RangeIter<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RangeIter")
            .field("table", &self.t.name)
            .finish()
    }
}

fn as_ref(b: &Bound<Vec<u8>>) -> Bound<&[u8]> {
    match b {
        Bound::Included(v) => Bound::Included(v.as_slice()),
        Bound::Excluded(v) => Bound::Excluded(v.as_slice()),
        Bound::Unbounded => Bound::Unbounded,
    }
}

impl<'a> RangeIter<'a> {
    pub fn new(
        kv: &'a dyn KvRead,
        t: &'static TableDef,
        lo: Bound<Vec<u8>>,
        hi: Bound<Vec<u8>>,
        rev: bool,
    ) -> Self {
        RangeIter {
            kv,
            t,
            lo,
            hi,
            rev,
            buf: Vec::new().into_iter(),
            done: false,
        }
    }

    fn refill(&mut self) -> Result<()> {
        let page = self
            .kv
            .scan(self.t, as_ref(&self.lo), as_ref(&self.hi), self.rev, PAGE)?;
        if page.len() < PAGE {
            self.done = true;
        }
        if let Some((k, _)) = page.last() {
            if self.rev {
                self.hi = Bound::Excluded(k.clone());
            } else {
                self.lo = Bound::Excluded(k.clone());
            }
        }
        self.buf = page.into_iter();
        Ok(())
    }

    /// Repositions a forward iterator at the first key `>= key` (`Cursor::seek`).
    pub fn seek(&mut self, key: &[u8]) {
        debug_assert!(!self.rev);
        self.lo = Bound::Included(key.to_vec());
        self.buf = Vec::new().into_iter();
        self.done = false;
    }
}

impl Iterator for RangeIter<'_> {
    type Item = Result<Entry>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(e) = self.buf.next() {
                return Some(Ok(e));
            }
            if self.done {
                return None;
            }
            if let Err(e) = self.refill() {
                self.done = true;
                return Some(Err(e));
            }
        }
    }
}

/// Convenience scans over any [`KvRead`].
pub trait KvReadExt: KvRead {
    fn iter_range(
        &self,
        t: &'static TableDef,
        lo: Bound<Vec<u8>>,
        hi: Bound<Vec<u8>>,
    ) -> RangeIter<'_>
    where
        Self: Sized,
    {
        RangeIter::new(self, t, lo, hi, false)
    }
}

impl<T: KvRead> KvReadExt for T {}

/// Every key starting with `prefix`, in order.
pub fn prefix_iter<'a>(kv: &'a dyn KvRead, t: &'static TableDef, prefix: &[u8]) -> RangeIter<'a> {
    let hi = match prefix_successor(prefix) {
        Some(s) => Bound::Excluded(s),
        None => Bound::Unbounded,
    };
    RangeIter::new(kv, t, Bound::Included(prefix.to_vec()), hi, false)
}

/// The smallest byte string greater than every string with this prefix.
pub fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut s = prefix.to_vec();
    while let Some(last) = s.pop() {
        if last < 0xFF {
            s.push(last + 1);
            return Some(s);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successor() {
        assert_eq!(prefix_successor(&[1, 2]), Some(vec![1, 3]));
        assert_eq!(prefix_successor(&[1, 0xFF]), Some(vec![2]));
        assert_eq!(prefix_successor(&[0xFF, 0xFF]), None);
    }
}
