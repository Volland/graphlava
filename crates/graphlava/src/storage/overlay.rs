//! Per-request overlay ([[adr:0025]], rule `tx-group-request-isolation`).
//!
//! A request runs against an overlay on top of the open batch transaction. All
//! its writes land in a [`WriteSet`]; reads see it first. If the request fails,
//! the write set is dropped and the batch is unchanged; if it succeeds, the
//! write set is applied to the batch. Overlays nest: an interactive
//! transaction runs each call in an inner overlay so a failing call undoes
//! only itself.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;

use super::{Entry, KvRead, KvWrite, RangeIter, TableDef};
use crate::error::Result;

type Changes = BTreeMap<Vec<u8>, Option<Vec<u8>>>;

/// Uncommitted writes, detached from any view.
#[derive(Debug, Default, Clone)]
pub struct WriteSet {
    tables: HashMap<&'static str, (&'static TableDef, Changes)>,
    /// Bytes of keys and values written, for the batch size window.
    pub bytes: usize,
}

impl WriteSet {
    pub fn is_empty(&self) -> bool {
        self.tables.values().all(|(_, c)| c.is_empty())
    }

    /// Applies every write to `kv`, table by table in key order.
    pub fn apply(self, kv: &mut dyn KvWrite) -> Result<()> {
        let mut tables: Vec<_> = self.tables.into_values().collect();
        tables.sort_by_key(|(t, _)| t.name);
        for (t, changes) in tables {
            for (k, v) in changes {
                match v {
                    Some(v) => kv.put(t, &k, &v)?,
                    None => {
                        kv.del(t, &k)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// A view: a base plus uncommitted writes.
pub struct Overlay<'b> {
    base: &'b dyn KvRead,
    set: WriteSet,
}

impl std::fmt::Debug for Overlay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overlay")
            .field("bytes", &self.set.bytes)
            .finish()
    }
}

impl<'b> Overlay<'b> {
    pub fn new(base: &'b dyn KvRead) -> Self {
        Overlay {
            base,
            set: WriteSet::default(),
        }
    }

    /// Resumes an overlay with earlier writes.
    pub fn with(base: &'b dyn KvRead, set: WriteSet) -> Self {
        Overlay { base, set }
    }

    pub fn into_writes(self) -> WriteSet {
        self.set
    }

    fn changes(&mut self, t: &'static TableDef) -> &mut Changes {
        &mut self
            .set
            .tables
            .entry(t.name)
            .or_insert_with(|| (t, BTreeMap::new()))
            .1
    }
}

fn to_owned(b: Bound<&[u8]>) -> Bound<Vec<u8>> {
    match b {
        Bound::Included(x) => Bound::Included(x.to_vec()),
        Bound::Excluded(x) => Bound::Excluded(x.to_vec()),
        Bound::Unbounded => Bound::Unbounded,
    }
}

impl KvRead for Overlay<'_> {
    fn get(&self, t: &'static TableDef, key: &[u8]) -> Result<Option<Vec<u8>>> {
        if let Some(v) = self.set.tables.get(t.name).and_then(|(_, c)| c.get(key)) {
            return Ok(v.clone());
        }
        self.base.get(t, key)
    }

    fn scan(
        &self,
        t: &'static TableDef,
        lo: Bound<&[u8]>,
        hi: Bound<&[u8]>,
        rev: bool,
        limit: usize,
    ) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        if limit == 0 {
            return Ok(out);
        }
        let Some((_, changes)) = self.set.tables.get(t.name).filter(|(_, c)| !c.is_empty()) else {
            return self.base.scan(t, lo, hi, rev, limit);
        };
        if let (Bound::Included(a) | Bound::Excluded(a), Bound::Included(b) | Bound::Excluded(b)) =
            (lo, hi)
        {
            if a > b
                || (a == b
                    && !(matches!(lo, Bound::Included(_)) && matches!(hi, Bound::Included(_))))
            {
                return Ok(out);
            }
        }
        let mut ov: Vec<(&Vec<u8>, &Option<Vec<u8>>)> =
            changes.range::<[u8], _>((lo, hi)).collect();
        if rev {
            ov.reverse();
        }
        let mut ov = ov.into_iter().peekable();
        let mut base = RangeIter::new(self.base, t, to_owned(lo), to_owned(hi), rev).peekable();
        let before = |a: &[u8], b: &[u8]| if rev { a > b } else { a < b };
        while out.len() < limit {
            let take_base = match (base.peek(), ov.peek()) {
                (None, None) => break,
                (Some(Err(_)), _) => {
                    return Err(base.next().expect("peeked").expect_err("peeked an error"));
                }
                (Some(Ok((bk, _))), Some((ok, _))) => {
                    if bk.as_slice() == ok.as_slice() {
                        base.next(); // shadowed by the overlay
                        false
                    } else {
                        before(bk, ok)
                    }
                }
                (Some(Ok(_)), None) => true,
                (None, Some(_)) => false,
            };
            if take_base {
                out.push(base.next().expect("peeked")?);
            } else {
                let (k, v) = ov.next().expect("peeked");
                if let Some(v) = v {
                    out.push((k.clone(), v.clone()));
                }
            }
        }
        Ok(out)
    }

    fn snapshot_id(&self) -> u64 {
        self.base.snapshot_id()
    }
}

impl KvWrite for Overlay<'_> {
    fn as_read(&self) -> &dyn KvRead {
        self
    }
    fn put(&mut self, t: &'static TableDef, key: &[u8], value: &[u8]) -> Result<()> {
        t.check(key, Some(value));
        self.set.bytes += key.len() + value.len();
        self.changes(t).insert(key.to_vec(), Some(value.to_vec()));
        Ok(())
    }

    fn del(&mut self, t: &'static TableDef, key: &[u8]) -> Result<bool> {
        let existed = self.get(t, key)?.is_some();
        if existed {
            self.set.bytes += key.len();
            self.changes(t).insert(key.to_vec(), None);
        } else if let Some((_, c)) = self.set.tables.get_mut(t.name) {
            // Only drop a pending delete when the base lacks the key too.
            if self.base.get(t, key)?.is_none() {
                c.remove(key);
            }
        }
        Ok(existed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Width;

    static T: TableDef = TableDef {
        name: "t",
        key: Width::Var,
        value: Width::Var,
    };

    #[derive(Default)]
    struct Mem(BTreeMap<Vec<u8>, Vec<u8>>);
    impl KvRead for Mem {
        fn get(&self, _: &'static TableDef, key: &[u8]) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(key).cloned())
        }
        fn scan(
            &self,
            _: &'static TableDef,
            lo: Bound<&[u8]>,
            hi: Bound<&[u8]>,
            rev: bool,
            limit: usize,
        ) -> Result<Vec<Entry>> {
            let it = self
                .0
                .range::<[u8], _>((lo, hi))
                .map(|(k, v)| (k.clone(), v.clone()));
            Ok(if rev {
                it.rev().take(limit).collect()
            } else {
                it.take(limit).collect()
            })
        }
    }

    #[test]
    fn overlay_merges_scans() {
        let mut base = Mem::default();
        for k in [1u8, 3, 5, 7] {
            base.0.insert(vec![k], vec![k]);
        }
        let mut o = Overlay::new(&base);
        o.put(&T, &[4], &[4]).unwrap();
        o.del(&T, &[5]).unwrap();
        o.put(&T, &[7], &[70]).unwrap();
        let all = o
            .scan(&T, Bound::Unbounded, Bound::Unbounded, false, 100)
            .unwrap();
        assert_eq!(
            all,
            vec![
                (vec![1], vec![1]),
                (vec![3], vec![3]),
                (vec![4], vec![4]),
                (vec![7], vec![70])
            ]
        );
        let rev = o
            .scan(&T, Bound::Included(&[2][..]), Bound::Unbounded, true, 2)
            .unwrap();
        assert_eq!(rev, vec![(vec![7], vec![70]), (vec![4], vec![4])]);
        let mut inner = Overlay::new(&o);
        inner.del(&T, &[1]).unwrap();
        let ws = inner.into_writes();
        ws.apply(&mut o).unwrap();
        assert_eq!(o.get(&T, &[1]).unwrap(), None);
    }
}
