//! The statement store ([[01-data-model]], [[04-physical-layout]]): statements
//! with stable IDs, inline literal properties, promotion, annotations, the
//! orderings and cascade delete.
//!
//! Every write goes through [`Store`], which wraps one request's view (an
//! overlay on the open batch), so a failing request leaves nothing behind.
//! Invariants mirror `MetaCore.Store` (`WF`, `RefClosed`); the debug
//! assertions name the `<assumption>` of [[20-proofs]] they check.

// @tg: implements:: [[openspec:statement-store#Statement identity under update]]
// @tg: implements:: [[openspec:statement-store#Unique statements by default]]
// @tg: implements:: [[openspec:statement-store#Functional predicates update in place]]
// @tg: implements:: [[openspec:statement-store#Inline literal properties]]
// @tg: implements:: [[openspec:statement-store#Promotion on annotation]]
// @tg: implements:: [[openspec:statement-store#Cascade delete of annotations]]
// @tg: implements:: [[openspec:statement-store#Index coverage of base orderings]]

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Bound;

use crate::dict::Dict;
use crate::error::{Error, Result, corrupted, invalid};
use crate::graph;
use crate::hedge;
use crate::id::{self, CHUNK, Id, Kind};
use crate::literal;
use crate::storage::{KvRead, KvWrite, RangeIter, prefix_iter};
use crate::tables::{self as tb, Perm, feature, key, mk, unkey3};
use crate::term::{Literal, Term, TermRef, Triple};

// ---- records -----------------------------------------------------------------

/// `stmt.flags` bits ([[04-physical-layout#Tables]]).
pub mod flag {
    pub const PROMOTED: u16 = 1 << 0;
    /// A promoted hyperedge role edge ([[08-hyperedges#Promotion of role edges]]).
    pub const ROLE: u16 = 1 << 1;
    /// A promoted graph membership ([[09-graphs#Extra membership]]).
    pub const MEMBERSHIP: u16 = 1 << 2;
    pub const ANNOTATED: u16 = 1 << 4;
    pub const MULTI: u16 = 1 << 5;
    /// [[adr:0032]]: a reifier of a triple that is not asserted.
    pub const UNASSERTED: u16 = 1 << 6;
}

/// Predicate declaration flags in `meta` (`pred:<id>`).
pub mod pflag {
    pub const MULTI: u64 = 1 << 0;
    pub const FUNCTIONAL: u64 = 1 << 1;
    pub const VIDX_BUILDING: u64 = 1 << 2;
    pub const VIDX_READY: u64 = 1 << 3;
}

fn stmt_encode(t: Triple, flags: u16) -> [u8; 26] {
    let mut v = [0u8; 26];
    v[0..8].copy_from_slice(&t.s.le_bytes());
    v[8..16].copy_from_slice(&t.p.le_bytes());
    v[16..24].copy_from_slice(&t.o.le_bytes());
    v[24..26].copy_from_slice(&flags.to_le_bytes());
    v
}

fn stmt_decode(v: &[u8]) -> (Triple, u16) {
    (
        Triple {
            s: Id::from_le(&v[0..]),
            p: Id::from_le(&v[8..]),
            o: Id::from_le(&v[16..]),
        },
        u16::from_le_bytes([v[24], v[25]]),
    )
}

pub fn stmt_get(kv: &dyn KvRead, id: Id) -> Result<Option<(Triple, u16)>> {
    Ok(kv.get(&tb::STMT, &id.be())?.map(|v| stmt_decode(&v)))
}

pub fn pred_flags(kv: &dyn KvRead, p: Id) -> Result<u64> {
    Ok(tb::meta_u64(kv, &format!("pred:{:016x}", p.raw()))?.unwrap_or(0))
}

fn set_pred_flags(kv: &mut dyn KvWrite, p: Id, f: u64) -> Result<()> {
    tb::meta_put_u64(kv, &format!("pred:{:016x}", p.raw()), f)
}

/// State of an on-demand ordering: 0 absent, 1 building, 2 usable.
pub fn ordering_state(kv: &dyn KvRead, perm: Perm) -> Result<u64> {
    Ok(tb::meta_u64(kv, &format!("ord:{}", perm.name()))?.unwrap_or(0))
}

// ---- property records ---------------------------------------------------------

const PROPS_VERSION: u8 = 1;
const PROPS_SPILLED: u8 = 1;
/// [[adr:0034]]: spill above 256 entries or 2 KiB.
pub const SPILL_ENTRIES: usize = 256;
pub const SPILL_BYTES: usize = 2048;

fn must_spill(n: usize) -> bool {
    n > SPILL_ENTRIES || 4 + 16 * n > SPILL_BYTES
}

/// All inline properties of one node, sorted by `(pred, value)`. A value that is
/// a statement ID is a promoted property (rule `dm-promote-pointer`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PropsRec {
    pub spilled: bool,
    pub entries: Vec<(Id, Id)>,
}

pub fn load_props(kv: &dyn KvRead, node: Id) -> Result<PropsRec> {
    let Some(v) = kv.get(&tb::PROPS, &node.be())? else {
        return Ok(PropsRec::default());
    };
    if v.len() < 4 || v[0] != PROPS_VERSION {
        return Err(corrupted("bad property record"));
    }
    if v[1] & PROPS_SPILLED != 0 {
        let mut entries = Vec::new();
        for e in prefix_iter(kv, &tb::PROPS_EXT, &node.be()) {
            let (k, _) = e?;
            entries.push((Id::from_be(&k[8..]), Id::from_be(&k[16..])));
        }
        return Ok(PropsRec {
            spilled: true,
            entries,
        });
    }
    let n = u16::from_le_bytes([v[2], v[3]]) as usize;
    if v.len() != 4 + 16 * n {
        return Err(corrupted("property record length"));
    }
    let entries = (0..n)
        .map(|i| {
            (
                Id::from_le(&v[4 + 16 * i..]),
                Id::from_le(&v[12 + 16 * i..]),
            )
        })
        .collect();
    Ok(PropsRec {
        spilled: false,
        entries,
    })
}

fn store_props(
    kv: &mut dyn KvWrite,
    node: Id,
    old: &PropsRec,
    mut new: Vec<(Id, Id)>,
) -> Result<()> {
    new.sort();
    new.dedup();
    let spill = must_spill(new.len());
    if old.spilled {
        let olds: HashSet<_> = old.entries.iter().copied().collect();
        let news: HashSet<_> = if spill {
            new.iter().copied().collect()
        } else {
            HashSet::new()
        };
        for (p, v) in olds.difference(&news) {
            kv.del(&tb::PROPS_EXT, &key(&[node, *p, *v]))?;
        }
    }
    if new.is_empty() {
        kv.del(&tb::PROPS, &node.be())?;
        return Ok(());
    }
    if spill {
        tb::set_feature(kv, feature::PROPS_SPILL)?;
        let olds: HashSet<_> = if old.spilled {
            old.entries.iter().copied().collect()
        } else {
            HashSet::new()
        };
        for e in &new {
            if !olds.contains(e) {
                kv.put(&tb::PROPS_EXT, &key(&[node, e.0, e.1]), &[])?;
            }
        }
        let mut rec = vec![PROPS_VERSION, PROPS_SPILLED];
        rec.extend_from_slice(&(new.len().min(u16::MAX as usize) as u16).to_le_bytes());
        kv.put(&tb::PROPS, &node.be(), &rec)?;
    } else {
        let mut rec = Vec::with_capacity(4 + 16 * new.len());
        rec.extend_from_slice(&[PROPS_VERSION, 0]);
        rec.extend_from_slice(&(new.len() as u16).to_le_bytes());
        for (p, v) in &new {
            rec.extend_from_slice(&p.le_bytes());
            rec.extend_from_slice(&v.le_bytes());
        }
        kv.put(&tb::PROPS, &node.be(), &rec)?;
    }
    Ok(())
}

// ---- reads ----------------------------------------------------------------------

/// One statement as a pattern match returns it. `id` is `None` for an
/// unpromoted inline property (rule `dm-props-no-id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Match {
    pub s: Id,
    pub p: Id,
    pub o: Id,
    pub id: Option<Id>,
}

fn ordering_scan(kv: &dyn KvRead, perm: Perm, prefix: &[Id], out: &mut Vec<Match>) -> Result<()> {
    let pk = key(prefix);
    for e in prefix_iter(kv, perm.table(false), &pk) {
        let (k, v) = e?;
        let (s, p, o) = perm.spo(unkey3(&k));
        out.push(Match {
            s,
            p,
            o,
            id: Some(Id::from_le(&v)),
        });
    }
    for e in prefix_iter(kv, perm.table(true), &pk) {
        let (k, _) = e?;
        let (s, p, o) = perm.spo(unkey3(&k));
        out.push(Match {
            s,
            p,
            o,
            id: Some(Id::from_be(&k[24..])),
        });
    }
    Ok(())
}

fn props_matches(
    kv: &dyn KvRead,
    s: Id,
    p: Option<Id>,
    o: Option<Id>,
    out: &mut Vec<Match>,
) -> Result<()> {
    for (pp, v) in load_props(kv, s)?.entries {
        if v.kind() == Kind::Statement {
            continue; // promoted: found through the orderings
        }
        if p.is_none_or(|p| p == pp) && o.is_none_or(|o| o == v) {
            out.push(Match {
                s,
                p: pp,
                o: v,
                id: None,
            });
        }
    }
    Ok(())
}

/// Every statement matching a pattern; bound positions are answered from an
/// index range (rule `dm-props-virtual`, requirement "Index coverage of base
/// orderings"). Unasserted statements are not matched ([[adr:0032]]).
pub fn find(kv: &dyn KvRead, s: Option<Id>, p: Option<Id>, o: Option<Id>) -> Result<Vec<Match>> {
    let mut out = find_stored(kv, s, p, o)?;
    graph::virtual_matches(kv, s, p, o, &mut out)?;
    hedge::virtual_matches(kv, s, p, o, &mut out)?;
    Ok(out)
}

/// Stored statements and inline properties matching a pattern, without the
/// virtual membership, role and type triples.
pub fn find_stored(
    kv: &dyn KvRead,
    s: Option<Id>,
    p: Option<Id>,
    o: Option<Id>,
) -> Result<Vec<Match>> {
    let mut out = Vec::new();
    match (s, p, o) {
        (Some(s), Some(p), Some(o)) => ordering_scan(kv, Perm::Spo, &[s, p, o], &mut out)?,
        (Some(s), Some(p), None) => ordering_scan(kv, Perm::Spo, &[s, p], &mut out)?,
        (Some(s), None, Some(o)) => ordering_scan(kv, Perm::Osp, &[o, s], &mut out)?,
        (Some(s), None, None) => ordering_scan(kv, Perm::Spo, &[s], &mut out)?,
        (None, Some(p), Some(o)) => ordering_scan(kv, Perm::Pos, &[p, o], &mut out)?,
        (None, Some(p), None) => ordering_scan(kv, Perm::Pos, &[p], &mut out)?,
        (None, None, Some(o)) => ordering_scan(kv, Perm::Osp, &[o], &mut out)?,
        (None, None, None) => ordering_scan(kv, Perm::Spo, &[], &mut out)?,
    }
    if o.is_some_and(|o| !o.is_literal()) {
        return Ok(out);
    }
    match (s, p) {
        (Some(s), _) => props_matches(kv, s, p, o, &mut out)?,
        (None, Some(p)) => {
            // Value index when usable, else the predicate's node list (`plist`):
            // only nodes that have the predicate are read.
            if let Some(o) = o.filter(|o| !o.is_owned_literal()) {
                if pred_flags(kv, p)? & pflag::VIDX_READY != 0 {
                    // value index: one key range per value, rechecked by ID
                    let sk = if o.is_inline() {
                        o.raw()
                    } else {
                        sort_key_by_decode(kv, o)?
                    };
                    for e in prefix_iter(kv, &tb::PROPIDX, &key(&[p, Id(sk)])) {
                        let (k, _) = e?;
                        if Id::from_be(&k[24..]) == o {
                            out.push(Match {
                                s: Id::from_be(&k[16..]),
                                p,
                                o,
                                id: None,
                            });
                        }
                    }
                    return Ok(out);
                }
            }
            for e in prefix_iter(kv, &tb::PLIST, &p.be()) {
                let (k, _) = e?;
                props_matches(kv, Id::from_be(&k[8..]), Some(p), o, &mut out)?;
            }
        }
        (None, None) => {
            for e in RangeIter::new(kv, &tb::PROPS, Bound::Unbounded, Bound::Unbounded, false) {
                let (k, _) = e?;
                props_matches(kv, Id::from_be(&k), None, o, &mut out)?;
            }
        }
    }
    Ok(out)
}

fn sort_key_by_decode(kv: &dyn KvRead, v: Id) -> Result<u64> {
    let dict = Dict::new(Default::default());
    match dict.decode(kv, v)? {
        Term::Literal(l) => {
            let lang = match &l.lang {
                Some(t) => dict.lang_id(kv, t)?,
                None => None,
            };
            Ok(literal::sort_key(v, &l, lang))
        }
        _ => Ok(v.raw()),
    }
}

/// Literal-valued statements of `p` whose value equals `lit`, including owned
/// literals compared by content (rule `dict-owned-equality`). Uses the value
/// index when it is usable, else the `plist` range of `p`.
pub fn find_value(kv: &dyn KvRead, dict: &Dict, p: Id, lit: &Literal) -> Result<Vec<Match>> {
    let mut out = Vec::new();
    let target = dict.lookup_literal(kv, lit)?;
    let canon = Term::Literal(lit.clone());
    let equal = |v: Id| -> Result<bool> {
        if Some(v) == target {
            return Ok(true);
        }
        if v.is_owned_literal() {
            return Ok(dict.decode(kv, v)? == canon_term(kv, dict, &canon)?);
        }
        Ok(false)
    };
    if let Some(t) = target {
        ordering_scan(kv, Perm::Pos, &[p, t], &mut out)?;
    }
    if pred_flags(kv, p)? & pflag::VIDX_READY != 0 {
        let sk = match target {
            Some(t) => literal::sort_key(t, lit, None),
            None => literal::sort_key(Id(0), lit, None),
        };
        for e in prefix_iter(kv, &tb::PROPIDX, &key(&[p, Id(sk)])) {
            let (k, _) = e?;
            let (s, v) = (Id::from_be(&k[16..]), Id::from_be(&k[24..]));
            if equal(v)? {
                out.push(Match {
                    s,
                    p,
                    o: v,
                    id: None,
                });
            }
        }
        // owned literals with long text: their sort key is the 7-byte head
        return Ok(out);
    }
    for e in prefix_iter(kv, &tb::PLIST, &p.be()) {
        let (k, _) = e?;
        let s = Id::from_be(&k[8..]);
        for (pp, v) in load_props(kv, s)?.entries {
            if pp == p && v.kind() != Kind::Statement && equal(v)? {
                out.push(Match {
                    s,
                    p,
                    o: v,
                    id: None,
                });
            }
        }
    }
    Ok(out)
}

/// Inline-literal values of `p` in `[lo, hi]` by ID order, which is value
/// order within one inline subtype ([[02-id-encoding#Inline literals]]). With
/// a usable value index this is one `propidx` key range and no decoding;
/// otherwise the `plist` range of `p` and one record per node. Promoted values
/// come from the POS range. Returns the matches and whether the index was used.
pub fn find_range(
    kv: &dyn KvRead,
    p: Id,
    lo: Bound<Id>,
    hi: Bound<Id>,
) -> Result<(Vec<Match>, bool)> {
    let inside = |v: Id| {
        (match lo {
            Bound::Included(l) => v >= l,
            Bound::Excluded(l) => v > l,
            Bound::Unbounded => true,
        }) && (match hi {
            Bound::Included(h) => v <= h,
            Bound::Excluded(h) => v < h,
            Bound::Unbounded => true,
        })
    };
    let mut out = Vec::new();
    let mut promoted = Vec::new();
    ordering_scan(kv, Perm::Pos, &[p], &mut promoted)?;
    out.extend(
        promoted
            .into_iter()
            .filter(|m| m.o.is_inline() && inside(m.o)),
    );
    if pred_flags(kv, p)? & pflag::VIDX_READY != 0 {
        let k = |b: Bound<Id>| match b {
            Bound::Included(v) => Bound::Included(key(&[p, v])),
            Bound::Excluded(v) => Bound::Excluded(key(&[p, Id(v.raw().saturating_add(1))])),
            Bound::Unbounded => Bound::Unbounded,
        };
        let lo_k = match k(lo) {
            Bound::Excluded(x) => Bound::Included(x),
            b => b,
        };
        let lo_k = if matches!(lo_k, Bound::Unbounded) {
            Bound::Included(p.be().to_vec())
        } else {
            lo_k
        };
        let hi_k = match hi {
            Bound::Included(v) => match v.raw().checked_add(1) {
                Some(n) => Bound::Excluded(key(&[p, Id(n)])),
                None => Bound::Excluded(key(&[Id(p.raw() + 1)])),
            },
            Bound::Excluded(v) => Bound::Excluded(key(&[p, v])),
            Bound::Unbounded => match crate::storage::prefix_successor(&p.be()) {
                Some(s) => Bound::Excluded(s),
                None => Bound::Unbounded,
            },
        };
        for e in RangeIter::new(kv, &tb::PROPIDX, lo_k, hi_k, false) {
            let (k, _) = e?;
            let v = Id::from_be(&k[24..]);
            if v.is_inline() && inside(v) {
                out.push(Match {
                    s: Id::from_be(&k[16..]),
                    p,
                    o: v,
                    id: None,
                });
            }
        }
        return Ok((out, true));
    }
    for e in prefix_iter(kv, &tb::PLIST, &p.be()) {
        let (k, _) = e?;
        let s = Id::from_be(&k[8..]);
        for (pp, v) in load_props(kv, s)?.entries {
            if pp == p && v.is_inline() && inside(v) {
                out.push(Match {
                    s,
                    p,
                    o: v,
                    id: None,
                });
            }
        }
    }
    Ok((out, false))
}

/// The canonical term a literal reads back as.
fn canon_term(kv: &dyn KvRead, dict: &Dict, t: &Term) -> Result<Term> {
    let Term::Literal(l) = t else {
        return Ok(t.clone());
    };
    match literal::canonicalize(l) {
        literal::Canon::Inline(id) => dict.decode(kv, id),
        literal::Canon::LangCandidate { text, tag } => Ok(Term::Literal(Literal::lang(text, tag))),
        literal::Canon::Dict {
            lexical,
            datatype,
            lang,
        } => Ok(Term::Literal(match lang {
            Some(tag) => Literal::lang(lexical, tag),
            None => Literal::typed(lexical, datatype),
        })),
    }
}

/// All inline properties of a node with their values; promoted ones carry
/// their statement ID. One record read plus one `stmt` read per promoted value.
pub fn props(kv: &dyn KvRead, node: Id) -> Result<Vec<Match>> {
    let mut out = Vec::new();
    for (p, v) in load_props(kv, node)?.entries {
        if v.kind() == Kind::Statement {
            let (t, _) = stmt_get(kv, v)?
                .ok_or_else(|| corrupted("property pointer to a missing statement"))?;
            out.push(Match {
                s: node,
                p,
                o: t.o,
                id: Some(v),
            });
        } else {
            out.push(Match {
                s: node,
                p,
                o: v,
                id: None,
            });
        }
    }
    Ok(out)
}

/// Statements with an ID whose object is `o` (OSP or POS prefix).
pub fn referrers(kv: &dyn KvRead, o: Id) -> Result<Vec<Id>> {
    let mut m = Vec::new();
    ordering_scan(kv, Perm::Osp, &[o], &mut m)?;
    let mut ids: Vec<Id> = m.into_iter().filter_map(|x| x.id).collect();
    for e in RangeIter::new(
        kv,
        &tb::UNASSERTED,
        Bound::Unbounded,
        Bound::Unbounded,
        false,
    ) {
        let (k, v) = e?;
        if Id::from_be(&k[16..]) == o {
            ids.push(Id::from_le(&v));
        }
    }
    Ok(ids)
}

/// The statement a reifier alias IRI names ([[adr:0032]]).
pub fn alias_target(kv: &dyn KvRead, iri: Id) -> Result<Option<Id>> {
    Ok(kv.get(&tb::ALIAS, &iri.be())?.map(|v| Id::from_le(&v)))
}

/// The statement ID of `(s, p, o)` without writing: the asserted statement
/// (the first one of a `multi` predicate) or the unasserted one
/// ([[adr:0032]]). An inline property has no ID and yields `None`.
pub fn stmt_lookup(kv: &dyn KvRead, s: Id, p: Id, o: Id) -> Result<Option<Id>> {
    let k = key(&[s, p, o]);
    if let Some(v) = kv.get(&tb::SPO, &k)? {
        return Ok(Some(Id::from_le(&v)));
    }
    if let Some(e) = prefix_iter(kv, &tb::SPO_M, &k).next() {
        return Ok(Some(Id::from_be(&e?.0[24..])));
    }
    Ok(kv.get(&tb::UNASSERTED, &k)?.map(|v| Id::from_le(&v)))
}

/// Unasserted statements matching a pattern, with their IDs: the triples a
/// triple term or reifier names without asserting them ([[adr:0032]]).
pub fn find_unasserted(
    kv: &dyn KvRead,
    s: Option<Id>,
    p: Option<Id>,
    o: Option<Id>,
) -> Result<Vec<Match>> {
    let prefix: Vec<Id> = match (s, p) {
        (Some(s), Some(p)) => vec![s, p],
        (Some(s), None) => vec![s],
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for e in prefix_iter(kv, &tb::UNASSERTED, &key(&prefix)) {
        let (k, v) = e?;
        let [ks, kp, ko] = unkey3(&k);
        if p.is_some_and(|p| p != kp) || o.is_some_and(|o| o != ko) {
            continue;
        }
        out.push(Match {
            s: ks,
            p: kp,
            o: ko,
            id: Some(Id::from_le(&v)),
        });
    }
    Ok(out)
}

/// Statements with an ID whose subject is `s`: the annotations of `s`.
pub fn annotations(kv: &dyn KvRead, s: Id) -> Result<Vec<Id>> {
    let mut m = Vec::new();
    ordering_scan(kv, Perm::Spo, &[s], &mut m)?;
    let mut ids: Vec<Id> = m.into_iter().filter_map(|x| x.id).collect();
    for e in prefix_iter(kv, &tb::UNASSERTED, &s.be()) {
        let (_, v) = e?;
        ids.push(Id::from_le(&v));
    }
    Ok(ids)
}

// ---- writes -------------------------------------------------------------------

/// How a delete treats what refers to the deleted item
/// ([[06-transactions-durability#Cascade delete]]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeleteMode {
    /// Delete annotations transitively; object-position references block.
    #[default]
    Cascade,
    /// Delete only the item; its annotations remain.
    KeepAnnotations,
    /// Cascade, and also delete statements that refer to a deleted item in
    /// object position ([[adr:0036]]).
    Detach,
}

/// A meta-model predicate declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PredicateDecl {
    pub multi: bool,
    pub functional: bool,
}

/// A literal object resolved for a write: owned literals are created only
/// when a slot takes them (rule `dict-owned-lifetime`).
#[derive(Debug, Clone)]
enum Obj {
    Id(Id),
    Owned(Literal),
}

/// Write-side settings.
#[derive(Debug, Clone, Copy)]
pub struct StoreConfig {
    pub cascade_limit: u64,
}

impl Default for StoreConfig {
    fn default() -> Self {
        StoreConfig {
            cascade_limit: 100_000,
        }
    }
}

/// One request's write access to the store.
pub struct Store<'a> {
    pub kv: &'a mut dyn KvWrite,
    pub dict: Dict,
    pub cfg: StoreConfig,
    /// Sequential IDs handed out by the intern path, for the sweep's kept set.
    pub interned: Vec<Id>,
    /// Request-scoped blank-node labels (rule `dm-blank-scope`).
    pub blanks: HashMap<String, Id>,
}

impl std::fmt::Debug for Store<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl<'a> Store<'a> {
    pub fn new(kv: &'a mut dyn KvWrite, dict: Dict, cfg: StoreConfig) -> Self {
        Store {
            kv,
            dict,
            cfg,
            interned: Vec::new(),
            blanks: HashMap::new(),
        }
    }

    pub(crate) fn ro(&self) -> &dyn KvRead {
        self.kv.as_read()
    }

    // ---- term resolution ------------------------------------------------------

    fn check_exists(&self, id: Id) -> Result<()> {
        match id.kind() {
            Kind::Null => invalid("the null ID is not a term"),
            Kind::Statement => match stmt_get(self.ro(), id)? {
                Some(_) => Ok(()),
                None => Err(Error::NotFound(id.raw())),
            },
            Kind::Hyperedge => match self.ro().get(&tb::HEDGE, &id.be())? {
                Some(_) => Ok(()),
                None => Err(Error::NotFound(id.raw())),
            },
            Kind::Graph => {
                if graph::graph_exists(self.ro(), id)? {
                    Ok(())
                } else {
                    Err(Error::NotFound(id.raw()))
                }
            }
            _ => Ok(()),
        }
    }

    /// Resolves a term to an ID, interning it.
    pub fn intern(&mut self, t: &TermRef) -> Result<Id> {
        match self.resolve(t)? {
            Obj::Id(id) => Ok(id),
            Obj::Owned(l) => self.materialize(&Obj::Owned(l)),
        }
    }

    fn resolve(&mut self, t: &TermRef) -> Result<Obj> {
        let id = match t {
            TermRef::Id(id) => {
                self.check_exists(*id)?;
                *id
            }
            TermRef::Term(Term::Iri(iri)) => {
                if let Some(n) = iri
                    .strip_prefix(crate::term::STMT_IRI_PREFIX)
                    .and_then(|n| n.parse::<u64>().ok())
                {
                    let id = Id(n);
                    if matches!(id.kind(), Kind::Statement | Kind::Hyperedge) {
                        self.check_exists(id)?;
                        return Ok(Obj::Id(id));
                    }
                }
                let id = self.dict.intern_iri(self.kv, iri)?;
                self.interned.push(id);
                id
            }
            TermRef::Term(Term::Blank(label)) => match self.blanks.get(label) {
                Some(id) => *id,
                None => {
                    let id = self.dict.new_blank(self.kv)?;
                    self.blanks.insert(label.clone(), id);
                    id
                }
            },
            // A triple term is its statement, unasserted when the triple is
            // not asserted ([[adr:0032]]).
            TermRef::Term(Term::Triple(t)) => {
                let s = TermRef::Term(t.subject.clone());
                let p = TermRef::Term(t.predicate.clone());
                let o = TermRef::Term(t.object.clone());
                match self.statement_id(&s, &p, &o)? {
                    Some(id) => id,
                    None => self.insert_unasserted(&s, &p, &o)?,
                }
            }
            TermRef::Term(Term::Literal(l)) => {
                if self.dict.is_owned_size(l) {
                    return Ok(Obj::Owned(l.clone()));
                }
                let id = self.dict.intern_literal(self.kv, l)?;
                if !id.is_inline() {
                    self.interned.push(id);
                }
                id
            }
        };
        Ok(Obj::Id(id))
    }

    fn materialize(&mut self, o: &Obj) -> Result<Id> {
        match o {
            Obj::Id(id) => Ok(*id),
            Obj::Owned(l) => self.dict.intern_literal(self.kv, l),
        }
    }

    fn obj_is_literal(o: &Obj) -> bool {
        match o {
            Obj::Id(id) => id.is_literal(),
            Obj::Owned(_) => true,
        }
    }

    /// Whether stored value `v` equals the resolved object (owned-aware).
    fn obj_eq(&self, v: Id, o: &Obj) -> Result<bool> {
        match o {
            Obj::Id(id) => {
                if v == *id {
                    return Ok(true);
                }
                self.dict.literal_eq(self.ro(), v, *id)
            }
            Obj::Owned(l) => {
                if !v.is_owned_literal() {
                    return Ok(false);
                }
                Ok(self.dict.decode(self.ro(), v)?
                    == canon_term(self.ro(), &self.dict, &Term::Literal(l.clone()))?)
            }
        }
    }

    // ---- allocation (rules `id-alloc-chunk-lease`, `id-never-reused`) ---------

    pub(crate) fn alloc_chunked(&mut self, graph: Id, kind: Kind, kind_value: u64) -> Result<Id> {
        let lease_key = format!("lease:{:016x}:{kind_value:016x}", graph.raw());
        let (chunk, off) = match tb::meta_get(self.ro(), &lease_key)? {
            Some(v) if v.len() == 16 => (
                u64::from_le_bytes(v[0..8].try_into().expect("8")),
                u64::from_le_bytes(v[8..16].try_into().expect("8")),
            ),
            _ => (0, CHUNK),
        };
        let (chunk, off) = if off >= CHUNK {
            let next = tb::meta_u64(self.ro(), mk::NEXT_CHUNK)?.unwrap_or(1);
            tb::meta_put_u64(self.kv, mk::NEXT_CHUNK, next + 1)?;
            let mut g = [0u8; 16];
            g[0..8].copy_from_slice(&graph.le_bytes());
            g[8..16].copy_from_slice(&kind_value.to_le_bytes());
            self.kv.put(&tb::GCHUNK, &next.to_be_bytes(), &g)?;
            let mut kc = [0u8; 16];
            kc[0..8].copy_from_slice(&kind_value.to_be_bytes());
            kc[8..16].copy_from_slice(&next.to_be_bytes());
            self.kv.put(&tb::KIND_CHUNK, &kc, &[])?;
            self.kv.put(
                &tb::GRAPH_CHUNKS,
                &key(&[graph, Id(kind_value), Id(next)]),
                &[],
            )?;
            (next, 0)
        } else {
            (chunk, off)
        };
        let mut v = [0u8; 16];
        v[0..8].copy_from_slice(&chunk.to_le_bytes());
        v[8..16].copy_from_slice(&(off + 1).to_le_bytes());
        tb::meta_put(self.kv, &lease_key, &v)?;
        Ok(Id::chunked(kind, chunk, off))
    }

    /// A statement ID in a statement chunk of graph `g`.
    pub(crate) fn alloc_stmt(&mut self, g: Id) -> Result<Id> {
        self.alloc_chunked(g, Kind::Statement, graph::KV_STATEMENT)
    }

    // ---- orderings ------------------------------------------------------------

    fn orderings(&mut self, t: Triple, id: Id, multi: bool, add: bool) -> Result<()> {
        crate::query::plan::bump(self.kv, t.p, if add { 1 } else { -1 })?;
        let mut perms: Vec<Perm> = Perm::BASE.to_vec();
        for p in Perm::ON_DEMAND {
            if ordering_state(self.ro(), p)? > 0 {
                perms.push(p);
            }
        }
        for perm in perms {
            let k3 = perm.arrange(t.s, t.p, t.o);
            if multi {
                let k = key(&[k3[0], k3[1], k3[2], id]);
                if add {
                    self.kv.put(perm.table(true), &k, &[])?;
                } else {
                    self.kv.del(perm.table(true), &k)?;
                }
            } else {
                let k = key(&k3);
                if add {
                    self.kv.put(perm.table(false), &k, &id.le_bytes())?;
                } else {
                    self.kv.del(perm.table(false), &k)?;
                }
            }
        }
        Ok(())
    }

    // ---- value index ------------------------------------------------------------

    fn vidx(&mut self, p: Id, s: Id, v: Id, add: bool) -> Result<()> {
        if pred_flags(self.ro(), p)? & (pflag::VIDX_BUILDING | pflag::VIDX_READY) == 0 {
            return Ok(());
        }
        let sk = self.sort_key_of(v)?;
        let k = key(&[p, Id(sk), s, v]);
        if add {
            self.kv.put(&tb::PROPIDX, &k, &[])?;
        } else {
            self.kv.del(&tb::PROPIDX, &k)?;
        }
        Ok(())
    }

    fn sort_key_of(&self, v: Id) -> Result<u64> {
        if v.is_inline() {
            return Ok(v.raw());
        }
        match self.dict.decode(self.ro(), v)? {
            Term::Literal(l) => {
                let lang = match &l.lang {
                    Some(t) => self.dict.lang_id(self.ro(), t)?,
                    None => None,
                };
                Ok(literal::sort_key(v, &l, lang))
            }
            _ => Ok(v.raw()),
        }
    }

    // ---- property slots -----------------------------------------------------------

    fn add_slot(&mut self, s: Id, p: Id, v: Id) -> Result<()> {
        let rec = load_props(self.ro(), s)?;
        let first = !rec
            .entries
            .iter()
            .any(|(pp, vv)| *pp == p && vv.kind() != Kind::Statement);
        let mut entries = rec.entries.clone();
        entries.push((p, v));
        store_props(self.kv, s, &rec, entries)?;
        if v.kind() != Kind::Statement {
            crate::query::plan::bump(self.kv, p, 1)?;
            if first {
                self.kv.put(&tb::PLIST, &key(&[p, s]), &[])?;
            }
            self.vidx(p, s, v, true)?;
        }
        Ok(())
    }

    /// Removes one slot; drops its owned literal unless `keep_owned`.
    pub(crate) fn remove_slot(&mut self, s: Id, p: Id, v: Id, keep_owned: bool) -> Result<()> {
        let rec = load_props(self.ro(), s)?;
        let mut entries = rec.entries.clone();
        let Some(pos) = entries.iter().position(|e| *e == (p, v)) else {
            return Ok(());
        };
        entries.remove(pos);
        let still = entries
            .iter()
            .any(|(pp, vv)| *pp == p && vv.kind() != Kind::Statement);
        store_props(self.kv, s, &rec, entries)?;
        if v.kind() != Kind::Statement {
            crate::query::plan::bump(self.kv, p, -1)?;
            if !still {
                self.kv.del(&tb::PLIST, &key(&[p, s]))?;
            }
            self.vidx(p, s, v, false)?;
            if !keep_owned {
                self.dict.drop_owned(self.kv, v)?;
            }
        }
        Ok(())
    }

    // ---- statements -------------------------------------------------------------

    pub(crate) fn write_stmt(&mut self, id: Id, t: Triple, flags: u16) -> Result<()> {
        self.kv.put(&tb::STMT, &id.be(), &stmt_encode(t, flags))
    }

    /// A new stored statement (node-valued, multi, or promoted) with home graph `g`.
    fn new_statement(&mut self, t: Triple, flags: u16, g: Id) -> Result<Id> {
        let id = self.alloc_stmt(g)?;
        debug_assert!(
            stmt_get(self.ro(), id)?.is_none(),
            "assumption id-never-reused: fresh ID {id} already in stmt"
        );
        let multi = flags & flag::MULTI != 0;
        if multi {
            tb::set_feature(self.kv, feature::MULTI)?;
        }
        self.write_stmt(id, t, flags)?;
        self.orderings(t, id, multi, true)?;
        if t.o.is_literal() {
            self.add_slot(t.s, t.p, id)?;
        }
        Ok(id)
    }

    fn validate_triple(&self, s: Id, p: Id) -> Result<()> {
        if p.kind() != Kind::Iri {
            return invalid("a predicate must be an IRI (rule dm-term-positions)");
        }
        if !s.can_be_subject() {
            return invalid("a literal cannot be a subject (rule dm-term-positions)");
        }
        Ok(())
    }

    /// Unpromoted values of `(s, p)` in the property record.
    fn inline_values(&self, s: Id, p: Id) -> Result<Vec<Id>> {
        Ok(load_props(self.ro(), s)?
            .entries
            .into_iter()
            .filter(|(pp, v)| *pp == p && v.kind() != Kind::Statement)
            .map(|(_, v)| v)
            .collect())
    }

    /// Stored statements `(s, p, *)` with their objects (non-multi tables).
    fn stored_values(&self, s: Id, p: Id) -> Result<Vec<(Id, Id)>> {
        let mut m = Vec::new();
        ordering_scan(self.ro(), Perm::Spo, &[s, p], &mut m)?;
        Ok(m.into_iter()
            .map(|x| (x.id.expect("stored"), x.o))
            .collect())
    }

    fn unasserted_get(&self, t: Triple) -> Result<Option<Id>> {
        Ok(self
            .ro()
            .get(&tb::UNASSERTED, &key(&[t.s, t.p, t.o]))?
            .map(|v| Id::from_le(&v)))
    }

    /// Asserting a triple that so far existed only as a reifier target keeps
    /// its ID ([[adr:0032]]).
    fn assert_unasserted(&mut self, id: Id, t: Triple) -> Result<Id> {
        let (_, flags) =
            stmt_get(self.ro(), id)?.ok_or_else(|| corrupted("unasserted index without stmt"))?;
        self.kv.del(&tb::UNASSERTED, &key(&[t.s, t.p, t.o]))?;
        let flags = flags & !flag::UNASSERTED;
        self.write_stmt(id, t, flags)?;
        self.orderings(t, id, false, true)?;
        if t.o.is_literal() {
            self.add_slot(t.s, t.p, id)?;
        }
        Ok(id)
    }

    /// Asserts a reifier's triple into graph `g`: it becomes a member of `g`
    /// only, whatever chunk its ID came from.
    fn assert_into(&mut self, id: Id, t: Triple, g: Id) -> Result<Id> {
        self.assert_unasserted(id, t)?;
        let h = graph::home(self.ro(), id)?;
        if h != g {
            self.add_member(id, g)?;
            self.remove_member(id, h)?;
        }
        Ok(id)
    }

    /// `insert(s, p, o)` without a graph clause: into the subject's home graph,
    /// which is the default graph for IRIs and blank nodes (rule
    /// `gr-default-graph`) and the annotated item's graph for an annotation.
    pub fn insert(&mut self, s: &TermRef, p: &TermRef, o: &TermRef) -> Result<Option<Id>> {
        self.insert_in(None, s, p, o)
    }

    /// `insert(s, p, o)` into graph `g`; returns the statement ID, or `None`
    /// for a new or existing unpromoted inline property (rules
    /// `dm-unique-spo`, `dm-route-by-kind`, `dm-functional`,
    /// `gr-insert-existing`). A graph named for the first time is created.
    pub fn insert_in(
        &mut self,
        g: Option<&TermRef>,
        s: &TermRef,
        p: &TermRef,
        o: &TermRef,
    ) -> Result<Option<Id>> {
        let s = self.intern(s)?;
        let p = self.intern(p)?;
        let o = self.resolve(o)?;
        let g = match g {
            Some(g) => self.graph(g, true)?,
            None => graph::home(self.ro(), s)?,
        };
        self.insert_obj_in(s, p, o, g)
    }

    fn insert_ids(&mut self, s: Id, p: Id, o: Obj) -> Result<Option<Id>> {
        let g = graph::home(self.ro(), s)?;
        self.insert_obj_in(s, p, o, g)
    }

    pub(crate) fn insert_ids_in(&mut self, s: Id, p: Id, o: Id, g: Id) -> Result<Option<Id>> {
        self.insert_obj_in(s, p, Obj::Id(o), g)
    }

    /// The insert path. An inline property lives in its subject's home graph
    /// `hs`; a literal triple written into another graph is a promoted
    /// statement of that graph
    /// ([[22-implementation-decisions#Inline properties belong to their subject's home graph]]).
    fn insert_obj_in(&mut self, s: Id, p: Id, o: Obj, g: Id) -> Result<Option<Id>> {
        self.validate_triple(s, p)?;
        let pf = pred_flags(self.ro(), p)?;
        let hs = graph::home(self.ro(), s)?;
        if pf & pflag::MULTI != 0 {
            // A multi predicate gives every statement its own ID, so literal
            // objects are stored statements too.
            let o = self.materialize(&o)?;
            return self
                .new_statement(Triple { s, p, o }, flag::MULTI, g)
                .map(Some);
        }
        let functional = pf & pflag::FUNCTIONAL != 0;
        if Self::obj_is_literal(&o) {
            let inline = self.inline_values(s, p)?;
            let stored = self.stored_values(s, p)?;
            for v in &inline {
                if self.obj_eq(*v, &o)? {
                    if g == hs {
                        return Ok(None);
                    }
                    let id = self.promote(s, p, *v)?;
                    self.add_member(id, g)?;
                    return Ok(Some(id));
                }
            }
            for (id, v) in &stored {
                if self.obj_eq(*v, &o)? {
                    self.add_member(*id, g)?;
                    return Ok(Some(*id));
                }
            }
            if functional {
                if let Some(v) = inline.first() {
                    if g == hs {
                        let new = self.materialize(&o)?;
                        self.remove_slot(s, p, *v, false)?;
                        self.add_slot(s, p, new)?;
                        return Ok(None);
                    }
                    self.remove_slot(s, p, *v, false)?;
                } else if let Some((id, _)) = stored.first() {
                    self.update_obj(*id, o, None)?;
                    self.add_member(*id, g)?;
                    return Ok(Some(*id));
                }
            }
            if let Obj::Id(oid) = o {
                let t = Triple { s, p, o: oid };
                if let Some(id) = self.unasserted_get(t)? {
                    return self.assert_into(id, t, g).map(Some);
                }
            }
            let v = self.materialize(&o)?;
            if g == hs {
                self.add_slot(s, p, v)?;
                return Ok(None);
            }
            return self
                .new_statement(Triple { s, p, o: v }, flag::PROMOTED, g)
                .map(Some);
        }
        let Obj::Id(o) = o else {
            unreachable!("owned objects are literals")
        };
        let t = Triple { s, p, o };
        if let Some(v) = self.ro().get(&tb::SPO, &key(&[s, p, o]))? {
            let id = Id::from_le(&v);
            self.add_member(id, g)?;
            return Ok(Some(id));
        }
        if let Some(id) = self.unasserted_get(t)? {
            return self.assert_into(id, t, g).map(Some);
        }
        if functional {
            let mut existing = self.stored_values(s, p)?;
            for v in self.inline_values(s, p)? {
                // a literal value of a now node-valued functional predicate
                self.remove_slot(s, p, v, false)?;
            }
            if let Some((id, _)) = existing.pop() {
                self.update_obj(id, Obj::Id(o), None)?;
                self.add_member(id, g)?;
                return Ok(Some(id));
            }
        }
        self.new_statement(t, 0, g).map(Some)
    }

    /// Records a reifier for a triple that is not asserted: a statement with
    /// the `unasserted` flag, invisible to pattern matching ([[adr:0032]]).
    pub fn insert_unasserted(&mut self, s: &TermRef, p: &TermRef, o: &TermRef) -> Result<Id> {
        let s = self.intern(s)?;
        let p = self.intern(p)?;
        let o = self.intern(o)?;
        self.validate_triple(s, p)?;
        let t = Triple { s, p, o };
        if let Some(id) = self.statement_id_of(t)? {
            return Ok(id);
        }
        if let Some(id) = self.unasserted_get(t)? {
            return Ok(id);
        }
        tb::set_feature(self.kv, feature::UNASSERTED)?;
        let hs = graph::home(self.ro(), s)?;
        let id = self.alloc_stmt(hs)?;
        self.write_stmt(id, t, flag::UNASSERTED)?;
        self.kv
            .put(&tb::UNASSERTED, &key(&[s, p, o]), &id.le_bytes())?;
        Ok(id)
    }

    fn statement_id_of(&mut self, t: Triple) -> Result<Option<Id>> {
        if let Some(v) = self.ro().get(&tb::SPO, &key(&[t.s, t.p, t.o]))? {
            return Ok(Some(Id::from_le(&v)));
        }
        if let Some(e) = prefix_iter(self.ro(), &tb::SPO_M, &key(&[t.s, t.p, t.o])).next() {
            return Ok(Some(Id::from_be(&e?.0[24..])));
        }
        if t.o.is_literal() && self.inline_values(t.s, t.p)?.contains(&t.o) {
            return self.promote(t.s, t.p, t.o).map(Some);
        }
        if t.s.kind() == Kind::Hyperedge
            && hedge::hedge(self.ro(), t.s)?.is_some_and(|h| h.roles.contains(&(t.p, t.o)))
        {
            return self.role_statement(t.s, t.p, t.o).map(Some);
        }
        Ok(None)
    }

    /// The statement ID of an existing triple, promoting an inline property if
    /// needed (rule `dm-promote-on-annotate`).
    pub fn statement_id(&mut self, s: &TermRef, p: &TermRef, o: &TermRef) -> Result<Option<Id>> {
        let s = self.intern(s)?;
        let p = self.intern(p)?;
        if Some(p) == graph::sys_id(self.ro(), mk::SYS_IN)?
            && matches!(s.kind(), Kind::Statement | Kind::Hyperedge)
        {
            // `<< x :in g >>`: the membership, promoted (rule `gr-membership-promote`).
            let g = self.graph(o, false)?;
            return self.membership_id(s, g).map(Some);
        }
        let o = self.resolve(o)?;
        if let Obj::Owned(_) = &o {
            for v in self.inline_values(s, p)? {
                if self.obj_eq(v, &o)? {
                    return self.promote(s, p, v).map(Some);
                }
            }
            for (id, v) in self.stored_values(s, p)? {
                if self.obj_eq(v, &o)? {
                    return Ok(Some(id));
                }
            }
            return Ok(None);
        }
        let Obj::Id(o) = o else { unreachable!() };
        self.statement_id_of(Triple { s, p, o })
    }

    /// Promotes the inline property `(s, p, v)`: allocates a statement ID,
    /// writes `stmt` and the orderings, and replaces the slot by a pointer.
    pub fn promote(&mut self, s: Id, p: Id, v: Id) -> Result<Id> {
        let hs = graph::home(self.ro(), s)?;
        let id = self.alloc_stmt(hs)?;
        self.remove_slot(s, p, v, true)?;
        let t = Triple { s, p, o: v };
        self.write_stmt(id, t, flag::PROMOTED)?;
        self.orderings(t, id, false, true)?;
        self.add_slot(s, p, id)?;
        Ok(id)
    }

    /// `annotate(id, p, o)`: an ordinary insert with the statement as subject
    /// (rule `dm-annotation-is-statement`).
    pub fn annotate(&mut self, id: Id, p: &TermRef, o: &TermRef) -> Result<Option<Id>> {
        let (t, flags) = stmt_get(self.ro(), id)?.ok_or(Error::NotFound(id.raw()))?;
        if flags & flag::ANNOTATED == 0 {
            self.write_stmt(id, t, flags | flag::ANNOTATED)?;
        }
        let p = self.intern(p)?;
        let o = self.resolve(o)?;
        self.insert_ids(id, p, o)
    }

    /// `update(id, o[, expected])`: changes the object in place and keeps the
    /// ID (rules `dm-update-in-place`, `tx-cascade-overwrite`).
    pub fn update(&mut self, id: Id, o: &TermRef, expected: Option<&TermRef>) -> Result<()> {
        let o = self.resolve(o)?;
        let expected = match expected {
            None => None,
            Some(TermRef::Id(e)) => Some(Obj::Id(*e)),
            Some(TermRef::Term(t)) => Some(match self.dict.lookup(self.ro(), t)? {
                Some(e) => Obj::Id(e),
                None if t.as_literal().is_some_and(|l| self.dict.is_owned_size(l)) => {
                    Obj::Owned(t.as_literal().expect("literal").clone())
                }
                // A term that was never interned cannot be the current object.
                None => {
                    return Err(Error::Conflict(
                        "the current object differs from the expected one".into(),
                    ));
                }
            }),
        };
        self.update_obj(id, o, expected)
    }

    fn update_obj(&mut self, id: Id, o: Obj, expected: Option<Obj>) -> Result<()> {
        let (t, flags) = stmt_get(self.ro(), id)?.ok_or(Error::NotFound(id.raw()))?;
        if flags & flag::UNASSERTED != 0 {
            return invalid("an unasserted statement cannot be updated; assert it first");
        }
        if let Some(e) = &expected {
            if !self.obj_eq(t.o, e)? {
                return Err(Error::Conflict(format!(
                    "statement {id} no longer has the expected object"
                )));
            }
        }
        if self.obj_eq(t.o, &o)? {
            return Ok(());
        }
        if flags & flag::ROLE != 0 {
            // A promoted role edge changes with its hyperedge's role (rule `he-promote`).
            let Obj::Id(n) = o else {
                return invalid("a role filler is a node");
            };
            return self.update_role(t.s, &TermRef::Id(t.p), &TermRef::Id(n));
        }
        if flags & flag::MEMBERSHIP != 0 {
            return invalid("a membership changes by moving its item between graphs");
        }
        let multi = flags & flag::MULTI != 0;
        // Assumption "the multi flag does not change during a transaction" in [[20-proofs]].
        debug_assert_eq!(
            multi,
            pred_flags(self.ro(), t.p)? & pflag::MULTI != 0,
            "multi flag of {} changed under statement {id}",
            t.p
        );
        if !multi {
            if let Obj::Id(n) = &o {
                if self.ro().get(&tb::SPO, &key(&[t.s, t.p, *n]))?.is_some()
                    || self.unasserted_get(Triple { o: *n, ..t })?.is_some()
                {
                    return Err(Error::Conflict(format!(
                        "({}, {}, {n}) already exists",
                        t.s, t.p
                    )));
                }
            }
            if Self::obj_is_literal(&o) {
                for v in self.inline_values(t.s, t.p)? {
                    if self.obj_eq(v, &o)? {
                        return Err(Error::Conflict(
                            "the new triple already exists as an inline property".into(),
                        ));
                    }
                }
            }
        }
        let new = self.materialize(&o)?;
        self.orderings(t, id, multi, false)?;
        let nt = Triple { o: new, ..t };
        self.write_stmt(id, nt, flags)?;
        self.orderings(nt, id, multi, true)?;
        match (t.o.is_literal(), new.is_literal()) {
            (true, false) => self.remove_slot(t.s, t.p, id, true)?,
            (false, true) => self.add_slot(t.s, t.p, id)?,
            _ => {}
        }
        self.dict.drop_owned(self.kv, t.o)?;
        Ok(())
    }

    /// `set(s, p, o)`: replaces all values of `(s, p)` with `o`.
    pub fn set(&mut self, s: &TermRef, p: &TermRef, o: &TermRef) -> Result<Option<Id>> {
        let s = self.intern(s)?;
        let p = self.intern(p)?;
        let o = self.resolve(o)?;
        self.validate_triple(s, p)?;
        let pf = pred_flags(self.ro(), p)?;
        if pf & pflag::FUNCTIONAL != 0 {
            return self.insert_ids(s, p, o);
        }
        let inline = self.inline_values(s, p)?;
        let mut stored = self.stored_values(s, p)?;
        let mut multi_ids = Vec::new();
        for e in prefix_iter(self.ro(), &tb::SPO_M, &key(&[s, p])) {
            multi_ids.push(Id::from_be(&e?.0[24..]));
        }
        // One promoted value and nothing else: update it in place, keeping its ID.
        if inline.is_empty()
            && multi_ids.is_empty()
            && stored.len() == 1
            && stored[0].1.is_literal()
            && Self::obj_is_literal(&o)
        {
            let (id, _) = stored.pop().expect("one");
            self.update_obj(id, o, None)?;
            return Ok(Some(id));
        }
        let mut keep: Option<Option<Id>> = None;
        for v in inline {
            if keep.is_none() && self.obj_eq(v, &o)? {
                keep = Some(None);
            } else {
                self.remove_slot(s, p, v, false)?;
            }
        }
        for (id, v) in stored {
            if keep.is_none() && self.obj_eq(v, &o)? {
                keep = Some(Some(id));
            } else {
                self.delete(id, DeleteMode::Cascade)?;
            }
        }
        for id in multi_ids {
            self.delete(id, DeleteMode::Cascade)?;
        }
        match keep {
            Some(k) => Ok(k),
            None => self.insert_ids(s, p, o),
        }
    }

    /// Removes the statement `(s, p, o)` whatever its storage; returns whether
    /// one existed.
    pub fn remove(
        &mut self,
        s: &TermRef,
        p: &TermRef,
        o: &TermRef,
        mode: DeleteMode,
    ) -> Result<bool> {
        self.remove_in(None, s, p, o, mode)
    }

    /// Removes `(s, p, o)`. Without a graph the statement is deleted; inside a
    /// graph clause only its membership in that graph goes, and the statement
    /// is deleted with its last membership (rule `gr-delete-in-graph`).
    pub fn remove_in(
        &mut self,
        g: Option<&TermRef>,
        s: &TermRef,
        p: &TermRef,
        o: &TermRef,
        mode: DeleteMode,
    ) -> Result<bool> {
        let g = match g {
            None => None,
            Some(g) => match self.graph(g, false) {
                Ok(g) => Some(g),
                Err(Error::NotFound(_)) | Err(Error::Invalid(_)) => return Ok(false),
                Err(e) => return Err(e),
            },
        };
        let s = self.intern(s)?;
        let p = self.intern(p)?;
        let o = self.resolve(o)?;
        for v in self.inline_values(s, p)? {
            if self.obj_eq(v, &o)? {
                if g.is_some_and(|g| g != graph::home(self.ro(), s).unwrap_or(g)) {
                    return Ok(false);
                }
                self.remove_slot(s, p, v, false)?;
                return Ok(true);
            }
        }
        let mut found = None;
        for (id, v) in self.stored_values(s, p)? {
            if self.obj_eq(v, &o)? {
                found = Some(id);
                break;
            }
        }
        if found.is_none() {
            if let Obj::Id(oid) = o {
                if let Some(e) = prefix_iter(self.ro(), &tb::SPO_M, &key(&[s, p, oid])).next() {
                    found = Some(Id::from_be(&e?.0[24..]));
                }
            }
        }
        let Some(id) = found else {
            return Ok(false);
        };
        match g {
            None => {
                self.delete(id, mode)?;
            }
            Some(g) => {
                if !graph::direct_graphs(self.ro(), id)?.contains(&g) {
                    return Ok(false);
                }
                self.remove_member(id, g)?;
            }
        }
        Ok(true)
    }

    /// Deletes a statement or hyperedge and cascades (rules
    /// `tx-cascade-annotations`, `tx-cascade-keep`, `tx-cascade-limit`,
    /// `he-delete-filler`, [[adr:0036]]). Returns every deleted ID in deletion
    /// order. Deleting a promoted membership removes that membership.
    pub fn delete(&mut self, id: Id, mode: DeleteMode) -> Result<Vec<Id>> {
        match id.kind() {
            Kind::Statement => {
                let (t, f) = stmt_get(self.ro(), id)?.ok_or(Error::NotFound(id.raw()))?;
                if f & flag::MEMBERSHIP != 0 {
                    return self.remove_member(t.s, t.o);
                }
                if f & flag::ROLE != 0 {
                    return invalid(
                        "a role edge is deleted with its hyperedge or changed with update_role",
                    );
                }
            }
            Kind::Hyperedge => {
                if self.ro().get(&tb::HEDGE, &id.be())?.is_none() {
                    return Err(Error::NotFound(id.raw()));
                }
            }
            _ => return Err(Error::NotFound(id.raw())),
        }
        self.cascade(&[id], mode, 1)
    }

    /// The cascade of [[06-transactions-durability#Cascade delete]]. Mirrors
    /// `MetaCore.Store.cascadeDelete`: the doomed set is closed under
    /// "annotates" and "depends on" (and, with `Detach`, under object
    /// references and role fillings) before the guards are checked. The first
    /// `exempt` roots do not count towards the cascade limit (rule
    /// `gr-drop-cascade`).
    pub(crate) fn cascade(
        &mut self,
        roots: &[Id],
        mode: DeleteMode,
        exempt: usize,
    ) -> Result<Vec<Id>> {
        let limit = self.cfg.cascade_limit;
        let mut doomed: Vec<Id> = Vec::new();
        let mut seen: HashSet<Id> = HashSet::new();
        let mut queue: VecDeque<Id> = VecDeque::new();
        for r in roots {
            if seen.insert(*r) {
                doomed.push(*r);
                queue.push_back(*r);
            }
        }
        while let Some(y) = queue.pop_front() {
            let mut next = hedge::dependents(self.ro(), y)?;
            if mode != DeleteMode::KeepAnnotations {
                next.extend(annotations(self.ro(), y)?);
            }
            if mode == DeleteMode::Detach {
                next.extend(referrers(self.ro(), y)?);
                next.extend(hedge::role_holders(self.ro(), y)?);
            }
            for x in next {
                if seen.insert(x) {
                    doomed.push(x);
                    queue.push_back(x);
                    if doomed.len().saturating_sub(exempt) as u64 > limit {
                        return Err(Error::CascadeLimitExceeded { limit });
                    }
                }
            }
        }
        if mode != DeleteMode::Detach {
            for y in &doomed {
                for r in referrers(self.ro(), *y)? {
                    if !seen.contains(&r) {
                        return Err(Error::ReferencedByStatement {
                            item: y.raw(),
                            by: r.raw(),
                        });
                    }
                }
                for e in hedge::role_holders(self.ro(), *y)? {
                    if !seen.contains(&e) {
                        return Err(Error::FillsRole {
                            item: y.raw(),
                            hedge: e.raw(),
                        });
                    }
                }
            }
        }
        for y in &doomed {
            self.delete_one(*y, mode != DeleteMode::KeepAnnotations)?;
        }
        if cfg!(debug_assertions) {
            // Assumption of "Cascade delete" in [[20-proofs]]: no surviving
            // statement or hyperedge names a deleted item (RefClosed).
            for y in &doomed {
                debug_assert!(
                    annotations(self.ro(), *y)?.is_empty() || mode == DeleteMode::KeepAnnotations,
                    "RefClosed: annotation of {y} survived"
                );
                debug_assert!(
                    referrers(self.ro(), *y)?.is_empty(),
                    "RefClosed: object reference to {y} survived"
                );
                debug_assert!(
                    hedge::role_holders(self.ro(), *y)?.is_empty(),
                    "RefClosed: a hyperedge role still holds {y}"
                );
            }
        }
        Ok(doomed)
    }

    fn delete_one(&mut self, y: Id, drop_props: bool) -> Result<()> {
        if y.kind() == Kind::Hyperedge {
            self.delete_hedge_record(y)?;
        } else {
            let Some((t, flags)) = stmt_get(self.ro(), y)? else {
                return Ok(());
            };
            if flags & flag::UNASSERTED != 0 {
                self.kv.del(&tb::UNASSERTED, &key(&[t.s, t.p, t.o]))?;
            } else if flags & flag::ROLE != 0 {
                self.unlink_role(y, t)?;
            } else if flags & flag::MEMBERSHIP != 0 {
                self.unlink_membership(y, t)?;
            } else {
                self.orderings(t, y, flags & flag::MULTI != 0, false)?;
                if t.o.is_literal() {
                    self.remove_slot(t.s, t.p, y, true)?;
                }
            }
            self.kv.del(&tb::STMT, &y.be())?;
            self.dict.drop_owned(self.kv, t.o)?;
            let aliases: Vec<Id> = prefix_iter(self.ro(), &tb::ALIAS_REV, &y.be())
                .map(|e| e.map(|(k, _)| Id::from_be(&k[8..])))
                .collect::<Result<_>>()?;
            for a in aliases {
                self.kv.del(&tb::ALIAS, &a.be())?;
                self.kv.del(&tb::ALIAS_REV, &key(&[y, a]))?;
            }
        }
        self.forget_memberships(y)?;
        if drop_props {
            let rec = load_props(self.ro(), y)?;
            for (p, v) in rec.entries.clone() {
                if v.kind() != Kind::Statement {
                    self.kv.del(&tb::PLIST, &key(&[p, y]))?;
                    self.vidx(p, y, v, false)?;
                    self.dict.drop_owned(self.kv, v)?;
                }
            }
            store_props(self.kv, y, &rec, Vec::new())?;
        }
        Ok(())
    }

    /// Makes the IRI `alias` a second reifier of statement `id` (rule
    /// `sparql-reifier-iri`, [[adr:0032]]). Fails when the IRI already names
    /// another statement or is used as a term in a statement.
    pub fn add_alias(&mut self, alias: Id, id: Id) -> Result<()> {
        if alias.kind() != Kind::Iri {
            return invalid("a reifier alias is an IRI");
        }
        stmt_get(self.ro(), id)?.ok_or(Error::NotFound(id.raw()))?;
        match alias_target(self.ro(), alias)? {
            Some(x) if x == id => return Ok(()),
            Some(x) => {
                return Err(Error::Conflict(format!(
                    "reifier mismatch: {alias} already names statement {x}"
                )));
            }
            None => {}
        }
        tb::set_feature(self.kv, feature::ALIASES)?;
        self.kv.put(&tb::ALIAS, &alias.be(), &id.le_bytes())?;
        self.kv.put(&tb::ALIAS_REV, &key(&[id, alias]), &[])
    }

    // ---- meta-model ---------------------------------------------------------------

    /// Declares a predicate `multi` or `functional` (rules `dm-multi-predicate`,
    /// `dm-functional`, `dm-metamodel-validate`).
    pub fn declare_predicate(&mut self, p: &TermRef, decl: PredicateDecl) -> Result<()> {
        let p = self.intern(p)?;
        if decl.multi && decl.functional {
            return invalid("a predicate cannot be both multi and functional");
        }
        let old = pred_flags(self.ro(), p)?;
        let has_data = prefix_iter(self.ro(), &tb::POS, &p.be()).next().is_some()
            || prefix_iter(self.ro(), &tb::POS_M, &p.be()).next().is_some()
            || prefix_iter(self.ro(), &tb::PLIST, &p.be()).next().is_some();
        if (old & pflag::MULTI != 0) != decl.multi && has_data {
            return invalid(
                "changing a predicate between multi and non-multi needs a migration of its statements",
            );
        }
        if decl.functional && old & pflag::FUNCTIONAL == 0 {
            let mut per_subject: HashMap<Id, usize> = HashMap::new();
            for m in find(self.ro(), None, Some(p), None)? {
                let n = per_subject.entry(m.s).or_default();
                *n += 1;
                if *n > 1 {
                    return Err(Error::Conflict(format!(
                        "{} has two objects for {p}; it cannot become functional",
                        m.s
                    )));
                }
            }
        }
        let mut f = old & !(pflag::MULTI | pflag::FUNCTIONAL);
        if decl.multi {
            f |= pflag::MULTI;
        }
        if decl.functional {
            f |= pflag::FUNCTIONAL;
        }
        set_pred_flags(self.kv, p, f)
    }

    // ---- on-demand structures (rule `pl-ondemand-online`) -------------------------

    /// Starts building an on-demand ordering; writes keep it in step from now on.
    pub fn begin_ordering(&mut self, perm: Perm) -> Result<()> {
        if Perm::BASE.contains(&perm) {
            return Ok(());
        }
        if ordering_state(self.ro(), perm)? == 0 {
            tb::set_feature(self.kv, feature::ON_DEMAND_ORDERINGS)?;
            tb::meta_put_u64(self.kv, &format!("ord:{}", perm.name()), 1)?;
        }
        Ok(())
    }

    /// Copies up to `n` statements from SPO (then SPO_M) into `perm`, starting
    /// after `cursor`; returns the next cursor, or `None` when done.
    pub fn build_ordering_batch(
        &mut self,
        perm: Perm,
        cursor: Option<(bool, Vec<u8>)>,
        n: usize,
    ) -> Result<Option<(bool, Vec<u8>)>> {
        let (multi, from) = cursor.unwrap_or((false, Vec::new()));
        let lo = if from.is_empty() {
            Bound::Unbounded
        } else {
            Bound::Excluded(from.as_slice())
        };
        let page = self
            .ro()
            .scan(Perm::Spo.table(multi), lo, Bound::Unbounded, false, n)?;
        for (k, v) in &page {
            let [s, p, o] = unkey3(k);
            let k3 = perm.arrange(s, p, o);
            if multi {
                self.kv.put(
                    perm.table(true),
                    &key(&[k3[0], k3[1], k3[2], Id::from_be(&k[24..])]),
                    &[],
                )?;
            } else {
                self.kv.put(perm.table(false), &key(&k3), v)?;
            }
        }
        match (page.len() < n, multi) {
            (false, _) => Ok(Some((multi, page.last().expect("full page").0.clone()))),
            (true, false) => Ok(Some((true, Vec::new()))),
            (true, true) => {
                tb::meta_put_u64(self.kv, &format!("ord:{}", perm.name()), 2)?;
                Ok(None)
            }
        }
    }

    /// Starts a value index on `p`; writes keep it in step from now on.
    pub fn begin_value_index(&mut self, p: Id) -> Result<()> {
        let f = pred_flags(self.ro(), p)?;
        if f & (pflag::VIDX_BUILDING | pflag::VIDX_READY) == 0 {
            tb::set_feature(self.kv, feature::VALUE_INDEX)?;
            set_pred_flags(self.kv, p, f | pflag::VIDX_BUILDING)?;
        }
        Ok(())
    }

    /// Indexes up to `n` nodes of `p` after `cursor`; `None` when done.
    pub fn build_value_index_batch(
        &mut self,
        p: Id,
        cursor: Option<Id>,
        n: usize,
    ) -> Result<Option<Id>> {
        let lo = cursor.map(|c| key(&[p, c]));
        let pbe = p.be();
        let lo_b = match &lo {
            Some(k) => Bound::Excluded(k.as_slice()),
            None => Bound::Included(&pbe[..]),
        };
        let hi = crate::storage::prefix_successor(&p.be());
        let hi_b = match &hi {
            Some(h) => Bound::Excluded(h.as_slice()),
            None => Bound::Unbounded,
        };
        let page = self.ro().scan(&tb::PLIST, lo_b, hi_b, false, n)?;
        let mut last = None;
        for (k, _) in &page {
            let s = Id::from_be(&k[8..]);
            for v in self.inline_values(s, p)? {
                let sk = self.sort_key_of(v)?;
                self.kv.put(&tb::PROPIDX, &key(&[p, Id(sk), s, v]), &[])?;
            }
            last = Some(s);
        }
        if page.len() < n {
            let f = pred_flags(self.ro(), p)?;
            set_pred_flags(self.kv, p, (f & !pflag::VIDX_BUILDING) | pflag::VIDX_READY)?;
            return Ok(None);
        }
        Ok(last)
    }
}

/// Checks the cross-table invariants of [[04-physical-layout#Test plan]] over
/// a whole view: every `stmt` record has exactly its ordering entries, every
/// ordering entry has a `stmt` record, and `plist` equals the set of
/// `(p, n)` with an unpromoted property. Returns the first violation.
pub fn check_invariants(kv: &dyn KvRead) -> Result<Option<String>> {
    let mut stmts: HashMap<Id, (Triple, u16)> = HashMap::new();
    for e in RangeIter::new(kv, &tb::STMT, Bound::Unbounded, Bound::Unbounded, false) {
        let (k, v) = e?;
        stmts.insert(Id::from_be(&k), stmt_decode(&v));
    }
    let mut perms: Vec<Perm> = Perm::BASE.to_vec();
    for p in Perm::ON_DEMAND {
        if ordering_state(kv, p)? > 0 {
            perms.push(p);
        }
    }
    for perm in perms {
        let mut seen = 0usize;
        for multi in [false, true] {
            for e in RangeIter::new(
                kv,
                perm.table(multi),
                Bound::Unbounded,
                Bound::Unbounded,
                false,
            ) {
                let (k, v) = e?;
                let (s, p, o) = perm.spo(unkey3(&k));
                let id = if multi {
                    Id::from_be(&k[24..])
                } else {
                    Id::from_le(&v)
                };
                match stmts.get(&id) {
                    Some((t, f))
                        if *t == (Triple { s, p, o })
                            && (f & flag::MULTI != 0) == multi
                            && f & (flag::UNASSERTED | flag::ROLE | flag::MEMBERSHIP) == 0 =>
                    {
                        seen += 1
                    }
                    other => {
                        return Ok(Some(format!(
                            "{} entry ({s},{p},{o})->{id} has stmt {other:?}",
                            perm.name()
                        )));
                    }
                }
            }
        }
        let expected = stmts
            .values()
            .filter(|(_, f)| f & (flag::UNASSERTED | flag::ROLE | flag::MEMBERSHIP) == 0)
            .count();
        if seen != expected {
            return Ok(Some(format!(
                "{} has {seen} entries, stmt has {expected} asserted statements",
                perm.name()
            )));
        }
    }
    let mut plist_expected: HashSet<(Id, Id)> = HashSet::new();
    for e in RangeIter::new(kv, &tb::PROPS, Bound::Unbounded, Bound::Unbounded, false) {
        let (k, _) = e?;
        let node = Id::from_be(&k);
        for (p, v) in load_props(kv, node)?.entries {
            if v.kind() == Kind::Statement {
                match stmts.get(&v) {
                    Some((t, _)) if t.s == node && t.p == p && t.o.is_literal() => {}
                    other => {
                        return Ok(Some(format!(
                            "pointer ({node},{p})->{v} has stmt {other:?}"
                        )));
                    }
                }
            } else {
                plist_expected.insert((p, node));
            }
        }
    }
    let mut plist = HashSet::new();
    for e in RangeIter::new(kv, &tb::PLIST, Bound::Unbounded, Bound::Unbounded, false) {
        let (k, _) = e?;
        plist.insert((Id::from_be(&k), Id::from_be(&k[8..])));
    }
    if plist != plist_expected {
        return Ok(Some(format!(
            "plist differs: {} entries vs {} expected",
            plist.len(),
            plist_expected.len()
        )));
    }
    for (id, (t, f)) in &stmts {
        if f & flag::UNASSERTED == 0
            && t.o.is_literal()
            && !load_props(kv, t.s)?.entries.contains(&(t.p, *id))
        {
            return Ok(Some(format!(
                "statement {id} with literal object has no pointer slot"
            )));
        }
    }
    let _ = id::K;
    if let Some(v) = graph::check_invariants(kv)? {
        return Ok(Some(v));
    }
    hedge::check_invariants(kv)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;

    use super::*;
    use crate::storage::overlay::Overlay;
    use crate::storage::{Entry, TableDef};

    /// An empty in-memory base: the store runs entirely in an overlay on it.
    struct Empty;
    impl KvRead for Empty {
        fn get(&self, _: &'static TableDef, _: &[u8]) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn scan(
            &self,
            _: &'static TableDef,
            _: Bound<&[u8]>,
            _: Bound<&[u8]>,
            _: bool,
            _: usize,
        ) -> Result<Vec<Entry>> {
            Ok(Vec::new())
        }
    }

    /// Counts point reads of property records.
    struct Counting<'a> {
        inner: &'a dyn KvRead,
        props_gets: Cell<usize>,
    }
    impl KvRead for Counting<'_> {
        fn get(&self, t: &'static TableDef, k: &[u8]) -> Result<Option<Vec<u8>>> {
            if t.name == tb::PROPS.name {
                self.props_gets.set(self.props_gets.get() + 1);
            }
            self.inner.get(t, k)
        }
        fn scan(
            &self,
            t: &'static TableDef,
            lo: Bound<&[u8]>,
            hi: Bound<&[u8]>,
            rev: bool,
            limit: usize,
        ) -> Result<Vec<Entry>> {
            self.inner.scan(t, lo, hi, rev, limit)
        }
    }

    fn iri(s: &str) -> TermRef {
        TermRef::Term(Term::iri(format!("http://ex.org/{s}")))
    }

    // @tg: verifies:: [[openspec:statement-store#Inline literal properties#Literal predicate scan reads only nodes that have the predicate]] {level: unit}
    #[test]
    fn plist_scan_reads_only_matching_records() {
        let mut ov = Overlay::new(&Empty);
        let mut st = Store::new(
            &mut ov,
            Dict::new(Default::default()),
            StoreConfig::default(),
        );
        for i in 0..10_000 {
            st.insert(
                &iri(&format!("n{i}")),
                &iri("name"),
                &TermRef::Term(Term::integer(i)),
            )
            .unwrap();
        }
        for i in 0..100 {
            st.insert(
                &iri(&format!("n{}", i * 97)),
                &iri("age"),
                &TermRef::Term(Term::integer(20 + i)),
            )
            .unwrap();
        }
        let age = st.intern(&iri("age")).unwrap();
        let counting = Counting {
            inner: &ov,
            props_gets: Cell::new(0),
        };
        let rows = find(&counting, None, Some(age), None).unwrap();
        assert_eq!(rows.len(), 100);
        assert_eq!(
            counting.props_gets.get(),
            100,
            "one record read per node that has :age"
        );
    }

    #[test]
    fn spill_threshold_matches_adr_0034() {
        assert!(!must_spill(127));
        assert!(
            must_spill(128),
            "2 KiB is reached at 128 entries of 16 bytes"
        );
        let _ = BTreeMap::<u8, u8>::new();
    }
}
