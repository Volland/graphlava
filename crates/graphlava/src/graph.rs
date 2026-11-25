//! Named and nested graphs ([[09-graphs]]): chunk membership, extra
//! memberships with `moved` tombstones, graph names, transitive and direct
//! scope, the nesting DAG with its materialized closure `in_star`, lazy
//! membership IDs and graph drop.
//!
//! Membership rules: every chunked item (statement, hyperedge) has the home
//! graph of its ID chunk; an unpromoted inline property belongs to its
//! subject's home graph, which is the default graph for IRIs and blank nodes
//! ([[22-implementation-decisions#Inline properties belong to their subject's home graph]]).

// @tg: implements:: [[openspec:named-graphs#Home graph membership]]
// @tg: implements:: [[openspec:named-graphs#One statement in several graphs]]
// @tg: implements:: [[openspec:named-graphs#Transitive graph scope by default]]
// @tg: implements:: [[openspec:named-graphs#Acyclic nesting]]
// @tg: implements:: [[openspec:named-graphs#Annotatable membership]]
// @tg: implements:: [[openspec:named-graphs#Graph drop]]

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::ops::Bound;

use crate::error::{Error, Result, corrupted, invalid};
use crate::id::{CHUNK, DEFAULT_GRAPH, Id, Kind, SYSTEM_GRAPH};
use crate::storage::{KvRead, KvWrite, RangeIter, prefix_iter};
use crate::store::{self, DeleteMode, Match, Store, flag, stmt_get};
use crate::tables::{self as tb, feature, key, mk};
use crate::term::{DEFAULT_GRAPH_IRI, SYSTEM_GRAPH_IRI, Term, TermRef, Triple};

/// `gmem` value flags ([[09-graphs#Extra membership]]).
pub mod gflag {
    /// The entry is the item's home membership (it holds a pointer or a tombstone).
    pub const HOME: u8 = 1 << 0;
    /// The home membership was removed (`moved` tombstone).
    pub const MOVED: u8 = 1 << 1;
}

/// Lease kind values ([[02-id-encoding#Allocation]]): statements and graphs;
/// a hyperedge chunk's kind value is its type ID.
pub const KV_STATEMENT: u64 = 0;
pub const KV_GRAPH: u64 = 1;

/// Default nesting depth limit (rule `gr-dag-depth`).
pub const DEFAULT_DEPTH_LIMIT: u32 = 64;

// ---- reads --------------------------------------------------------------------

/// `gchunk[chunk]`: the owning graph and kind value of a chunk.
pub fn chunk_owner(kv: &dyn KvRead, chunk: u64) -> Result<Option<(Id, u64)>> {
    Ok(kv.get(&tb::GCHUNK, &chunk.to_be_bytes())?.map(|v| {
        (
            Id::from_le(&v[0..]),
            u64::from_le_bytes(v[8..16].try_into().expect("8")),
        )
    }))
}

/// The home graph of `x` (rule `gr-home-graph`). For a node that is not a
/// chunked item, the graph its inline properties belong to: the default graph.
pub fn home(kv: &dyn KvRead, x: Id) -> Result<Id> {
    match x.kind() {
        Kind::Statement | Kind::Graph | Kind::Hyperedge => chunk_owner(kv, x.chunk())?
            .map(|(g, _)| g)
            .ok_or_else(|| corrupted(format!("item {x} is in an unleased chunk"))),
        _ => Ok(DEFAULT_GRAPH),
    }
}

/// Whether `g` names a live graph.
pub fn graph_exists(kv: &dyn KvRead, g: Id) -> Result<bool> {
    if g == SYSTEM_GRAPH || g == DEFAULT_GRAPH {
        return Ok(true);
    }
    Ok(g.kind() == Kind::Graph && kv.get(&tb::GNAME_REV, &g.be())?.is_some())
}

/// The graph named by an IRI ID, if one exists.
pub fn graph_by_name_id(kv: &dyn KvRead, iri: Id) -> Result<Option<Id>> {
    Ok(kv.get(&tb::GNAME, &iri.be())?.map(|v| Id::from_le(&v)))
}

/// The IRI ID naming a graph (none for the two fixed graphs).
pub fn graph_name_id(kv: &dyn KvRead, g: Id) -> Result<Option<Id>> {
    Ok(kv.get(&tb::GNAME_REV, &g.be())?.map(|v| Id::from_le(&v)))
}

/// The graph named by a term, without creating anything.
pub fn graph_named(kv: &dyn KvRead, dict: &crate::dict::Dict, name: &Term) -> Result<Option<Id>> {
    match name {
        Term::Iri(i) if i == DEFAULT_GRAPH_IRI => Ok(Some(DEFAULT_GRAPH)),
        Term::Iri(i) if i == SYSTEM_GRAPH_IRI => Ok(Some(SYSTEM_GRAPH)),
        Term::Iri(_) => match dict.lookup(kv, name)? {
            Some(iri) => graph_by_name_id(kv, iri),
            None => Ok(None),
        },
        _ => Ok(None),
    }
}

/// A system IRI ID recorded in `meta` at file creation.
pub fn sys_id(kv: &dyn KvRead, k: &str) -> Result<Option<Id>> {
    Ok(tb::meta_u64(kv, k)?.map(Id))
}

pub fn gmem_get(kv: &dyn KvRead, g: Id, x: Id) -> Result<Option<(u8, Id)>> {
    Ok(kv
        .get(&tb::GMEM, &key(&[g, x]))?
        .map(|v| (v[0], Id::from_le(&v[1..]))))
}

fn gmem_put(kv: &mut dyn KvWrite, g: Id, x: Id, flags: u8, ptr: Id) -> Result<()> {
    let mut v = [0u8; 9];
    v[0] = flags;
    v[1..9].copy_from_slice(&ptr.le_bytes());
    kv.put(&tb::GMEM, &key(&[g, x]), &v)?;
    kv.put(&tb::GMEM_REV, &key(&[x, g]), &[])
}

fn gmem_del(kv: &mut dyn KvWrite, g: Id, x: Id) -> Result<()> {
    kv.del(&tb::GMEM, &key(&[g, x]))?;
    kv.del(&tb::GMEM_REV, &key(&[x, g]))?;
    Ok(())
}

/// The graphs that have an entry for `x` in `gmem` (home pointers, tombstones
/// and extra memberships).
fn gmem_graphs(kv: &dyn KvRead, x: Id) -> Result<Vec<Id>> {
    prefix_iter(kv, &tb::GMEM_REV, &x.be())
        .map(|e| e.map(|(k, _)| Id::from_be(&k[8..])))
        .collect()
}

/// The direct graphs of a chunked item: its home graph unless tombstoned,
/// plus its extra memberships (rule `gr-membership-virtual`).
pub fn direct_graphs(kv: &dyn KvRead, x: Id) -> Result<Vec<Id>> {
    let h = home(kv, x)?;
    let mut out = Vec::new();
    let mut moved = false;
    for g in gmem_graphs(kv, x)? {
        let (f, _) = gmem_get(kv, g, x)?.ok_or_else(|| corrupted("gmem_rev without gmem"))?;
        if g == h {
            moved = f & gflag::MOVED != 0;
        } else {
            out.push(g);
        }
    }
    if !moved {
        out.insert(0, h);
    }
    Ok(out)
}

/// The graphs a pattern match is a direct member of: a statement's (or
/// hyperedge's) memberships; for an unpromoted inline property, its
/// subject's home graph; for a virtual role, type or membership triple, the
/// memberships of its subject.
pub fn match_graphs(kv: &dyn KvRead, m: &Match) -> Result<Vec<Id>> {
    match m.id {
        Some(x) if !is_virtual_promoted(kv, x)? => direct_graphs(kv, x),
        _ if m.o.is_literal() => Ok(vec![home(kv, m.s)?]),
        _ => match m.s.kind() {
            Kind::Statement | Kind::Hyperedge => direct_graphs(kv, m.s),
            _ => Ok(vec![home(kv, m.s)?]),
        },
    }
}

/// A promoted role or membership statement: it has an ID but is served as a
/// virtual triple.
fn is_virtual_promoted(kv: &dyn KvRead, x: Id) -> Result<bool> {
    Ok(x.kind() == Kind::Statement
        && stmt_get(kv, x)?.is_some_and(|(_, f)| f & (flag::ROLE | flag::MEMBERSHIP) != 0))
}

pub fn parents(kv: &dyn KvRead, g: Id) -> Result<Vec<Id>> {
    second_ids(kv, &tb::GPARENT, g)
}

pub fn children(kv: &dyn KvRead, g: Id) -> Result<Vec<Id>> {
    second_ids(kv, &tb::GCHILD, g)
}

/// Every graph `g` is nested under, transitively (`in_star` prefix).
pub fn ancestors(kv: &dyn KvRead, g: Id) -> Result<Vec<Id>> {
    second_ids(kv, &tb::IN_STAR, g)
}

/// Every graph nested under `g`, transitively (`in_star_rev` prefix).
pub fn descendants(kv: &dyn KvRead, g: Id) -> Result<Vec<Id>> {
    second_ids(kv, &tb::IN_STAR_REV, g)
}

fn second_ids(kv: &dyn KvRead, t: &'static crate::storage::TableDef, first: Id) -> Result<Vec<Id>> {
    prefix_iter(kv, t, &first.be())
        .map(|e| e.map(|(k, _)| Id::from_be(&k[8..])))
        .collect()
}

/// The graphs a scope covers: `g` and, unless `direct`, every graph nested
/// under it (rules `gr-scope-transitive`, `gr-scope-direct-mode`).
pub fn scope_graphs(kv: &dyn KvRead, g: Id, direct: bool) -> Result<Vec<Id>> {
    let mut v = vec![g];
    if !direct {
        v.extend(descendants(kv, g)?);
    }
    Ok(v)
}

/// The chunks leased to `(g, kind_value)` (`graph_chunks` prefix).
pub fn graph_chunks(kv: &dyn KvRead, g: Id, kind_value: u64) -> Result<Vec<u64>> {
    prefix_iter(kv, &tb::GRAPH_CHUNKS, &key(&[g, Id(kind_value)]))
        .map(|e| e.map(|(k, _)| u64::from_be_bytes(k[16..24].try_into().expect("8"))))
        .collect()
}

/// IDs in `t` whose key lies in one chunk's ID range.
pub(crate) fn chunk_ids(
    kv: &dyn KvRead,
    t: &'static crate::storage::TableDef,
    kind: Kind,
    chunk: u64,
) -> Result<Vec<(Id, Vec<u8>)>> {
    let lo = Id::chunked(kind, chunk, 0);
    let hi = Id(lo.raw() + CHUNK);
    RangeIter::new(
        kv,
        t,
        Bound::Included(lo.be().to_vec()),
        Bound::Excluded(hi.be().to_vec()),
        false,
    )
    .map(|e| e.map(|(k, v)| (Id::from_be(&k), v)))
    .collect()
}

/// The asserted statements that are direct members of `g`: the statements of
/// its chunks minus tombstones, plus its extra members (rule `gr-graph-ranges`).
/// Promoted role and membership statements are served as virtual triples and
/// are not listed.
pub fn direct_statements(kv: &dyn KvRead, g: Id) -> Result<Vec<Id>> {
    let mut out = BTreeSet::new();
    for chunk in graph_chunks(kv, g, KV_STATEMENT)? {
        for (id, v) in chunk_ids(kv, &tb::STMT, Kind::Statement, chunk)? {
            let f = u16::from_le_bytes([v[24], v[25]]);
            if f & (flag::UNASSERTED | flag::ROLE | flag::MEMBERSHIP) != 0 {
                continue;
            }
            if gmem_get(kv, g, id)?.is_some_and(|(f, _)| f & gflag::MOVED != 0) {
                continue;
            }
            out.insert(id);
        }
    }
    for e in prefix_iter(kv, &tb::GMEM, &g.be()) {
        let (k, v) = e?;
        let x = Id::from_be(&k[8..]);
        if v[0] & gflag::HOME == 0 && x.kind() == Kind::Statement {
            out.insert(x);
        }
    }
    Ok(out.into_iter().collect())
}

/// The asserted statements in the scope of `g`, each once, in ID order.
pub fn scope_statements(kv: &dyn KvRead, g: Id, direct: bool) -> Result<Vec<Id>> {
    let mut out = BTreeSet::new();
    for d in scope_graphs(kv, g, direct)? {
        out.extend(direct_statements(kv, d)?);
    }
    Ok(out.into_iter().collect())
}

/// Every statement and virtual triple matching a pattern inside the scope of
/// `g` (the filter-driven plan of [[09-graphs#Plan choice]]). An item reachable
/// through several nested graphs is returned once.
pub fn find_in(
    kv: &dyn KvRead,
    g: Id,
    direct: bool,
    s: Option<Id>,
    p: Option<Id>,
    o: Option<Id>,
) -> Result<Vec<Match>> {
    let scope: HashSet<Id> = scope_graphs(kv, g, direct)?.into_iter().collect();
    let mut out = Vec::new();
    for m in store::find(kv, s, p, o)? {
        if match_graphs(kv, &m)?.iter().any(|x| scope.contains(x)) {
            out.push(m);
        }
    }
    Ok(out)
}

/// Virtual `:in` triples ([[09-graphs#Extra membership]], nesting edges)
/// matching a pattern, appended to `out`.
pub(crate) fn virtual_matches(
    kv: &dyn KvRead,
    s: Option<Id>,
    p: Option<Id>,
    o: Option<Id>,
    out: &mut Vec<Match>,
) -> Result<()> {
    let Some(in_id) = sys_id(kv, mk::SYS_IN)? else {
        return Ok(());
    };
    if p.is_some_and(|p| p != in_id) || o.is_some_and(|o| o.is_literal()) {
        return Ok(());
    }
    let member = |x: Id, g: Id, out: &mut Vec<Match>| -> Result<()> {
        if o.is_none_or(|o| o == g) {
            let ptr = gmem_get(kv, g, x)?.map(|(_, p)| p).filter(|p| !p.is_null());
            out.push(Match {
                s: x,
                p: in_id,
                o: g,
                id: ptr,
            });
        }
        Ok(())
    };
    match s {
        Some(x) => match x.kind() {
            Kind::Statement | Kind::Hyperedge => {
                let has_memberships = if x.kind() == Kind::Statement {
                    stmt_get(kv, x)?.is_some_and(|(_, f)| {
                        f & (flag::UNASSERTED | flag::ROLE | flag::MEMBERSHIP) == 0
                    })
                } else {
                    kv.get(&tb::HEDGE, &x.be())?.is_some()
                };
                if has_memberships {
                    for g in direct_graphs(kv, x)? {
                        member(x, g, out)?;
                    }
                }
            }
            Kind::Graph => {
                for par in parents(kv, x)? {
                    if o.is_none_or(|o| o == par) {
                        out.push(Match {
                            s: x,
                            p: in_id,
                            o: par,
                            id: None,
                        });
                    }
                }
            }
            _ => {}
        },
        None => {
            let graphs: Vec<Id> = match o {
                Some(g) if g.kind() == Kind::Graph => vec![g],
                Some(_) => return Ok(()),
                None => all_graphs(kv)?,
            };
            for g in graphs {
                for x in direct_statements(kv, g)? {
                    member(x, g, out)?;
                }
                for x in crate::hedge::direct_hedges(kv, g)? {
                    member(x, g, out)?;
                }
                for c in children(kv, g)? {
                    out.push(Match {
                        s: c,
                        p: in_id,
                        o: g,
                        id: None,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Every live graph: the two fixed graphs and every named graph.
pub fn all_graphs(kv: &dyn KvRead) -> Result<Vec<Id>> {
    let mut v = vec![SYSTEM_GRAPH, DEFAULT_GRAPH];
    for e in RangeIter::new(
        kv,
        &tb::GNAME_REV,
        Bound::Unbounded,
        Bound::Unbounded,
        false,
    ) {
        v.push(Id::from_be(&e?.0));
    }
    Ok(v)
}

/// Longest path, in edges, from `g` up to a root (`up`) or down to a leaf.
fn depth(kv: &dyn KvRead, g: Id, up: bool, memo: &mut HashMap<Id, u32>) -> Result<u32> {
    if let Some(d) = memo.get(&g) {
        return Ok(*d);
    }
    let next = if up {
        parents(kv, g)?
    } else {
        children(kv, g)?
    };
    let mut d = 0;
    for n in next {
        d = d.max(1 + depth(kv, n, up, memo)?);
    }
    memo.insert(g, d);
    Ok(d)
}

/// `set` in topological order, parents first (the subgraph is acyclic).
fn topo_order(kv: &dyn KvRead, set: &HashSet<Id>) -> Result<Vec<Id>> {
    let mut indeg: HashMap<Id, usize> = HashMap::new();
    for d in set {
        let n = parents(kv, *d)?.iter().filter(|p| set.contains(p)).count();
        indeg.insert(*d, n);
    }
    let mut ready: Vec<Id> = indeg
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(d, _)| *d)
        .collect();
    ready.sort();
    let mut queue: VecDeque<Id> = ready.into();
    let mut out = Vec::with_capacity(set.len());
    while let Some(d) = queue.pop_front() {
        out.push(d);
        for c in children(kv, d)? {
            if let Some(n) = indeg.get_mut(&c) {
                *n -= 1;
                if *n == 0 {
                    queue.push_back(c);
                }
            }
        }
    }
    if out.len() != set.len() {
        return Err(corrupted("graph nesting has a cycle"));
    }
    Ok(out)
}

/// Checks the graph tables: `gmem`/`gmem_rev` and `gparent`/`gchild` hold the
/// same pairs (rule `gr-extra-pair`), `in_star` is exactly the transitive
/// closure of `gparent` and mirrors `in_star_rev` (rule `gr-in-star-closure`),
/// and every membership pointer names a promoted membership statement.
pub fn check_invariants(kv: &dyn KvRead) -> Result<Option<String>> {
    let pairs = |t: &'static crate::storage::TableDef, flip: bool| -> Result<HashSet<(Id, Id)>> {
        RangeIter::new(kv, t, Bound::Unbounded, Bound::Unbounded, false)
            .map(|e| {
                e.map(|(k, _)| {
                    let (a, b) = (Id::from_be(&k), Id::from_be(&k[8..]));
                    if flip { (b, a) } else { (a, b) }
                })
            })
            .collect()
    };
    if pairs(&tb::GMEM, false)? != pairs(&tb::GMEM_REV, true)? {
        return Ok(Some("gmem and gmem_rev differ".into()));
    }
    let edges = pairs(&tb::GPARENT, false)?;
    if edges != pairs(&tb::GCHILD, true)? {
        return Ok(Some("gparent and gchild differ".into()));
    }
    let star = pairs(&tb::IN_STAR, false)?;
    if star != pairs(&tb::IN_STAR_REV, true)? {
        return Ok(Some("in_star and in_star_rev differ".into()));
    }
    // naive closure
    let mut up: HashMap<Id, Vec<Id>> = HashMap::new();
    for (c, p) in &edges {
        up.entry(*c).or_default().push(*p);
    }
    let mut closure = HashSet::new();
    for start in up.keys() {
        let mut seen = HashSet::new();
        let mut stack = up[start].clone();
        while let Some(a) = stack.pop() {
            if seen.insert(a) {
                closure.insert((*start, a));
                stack.extend(up.get(&a).cloned().unwrap_or_default());
            }
        }
        if seen.contains(start) {
            return Ok(Some(format!("nesting cycle through {start}")));
        }
    }
    if closure != star {
        return Ok(Some(format!(
            "in_star has {} pairs, the closure of gparent has {}",
            star.len(),
            closure.len()
        )));
    }
    let in_id = sys_id(kv, mk::SYS_IN)?;
    for e in RangeIter::new(kv, &tb::GMEM, Bound::Unbounded, Bound::Unbounded, false) {
        let (k, v) = e?;
        let (g, x) = (Id::from_be(&k), Id::from_be(&k[8..]));
        let (f, ptr) = (v[0], Id::from_le(&v[1..]));
        let live = match x.kind() {
            Kind::Statement => stmt_get(kv, x)?.is_some(),
            Kind::Hyperedge => kv.get(&tb::HEDGE, &x.be())?.is_some(),
            _ => false,
        };
        if !live {
            return Ok(Some(format!("gmem ({g},{x}) names a dead item")));
        }
        if (f & gflag::HOME != 0) != (home(kv, x)? == g) {
            return Ok(Some(format!("gmem ({g},{x}) home flag is wrong")));
        }
        if f & gflag::MOVED != 0 && !ptr.is_null() {
            return Ok(Some(format!("tombstone ({g},{x}) keeps a pointer")));
        }
        if f == gflag::HOME && ptr.is_null() {
            return Ok(Some(format!("empty home entry ({g},{x})")));
        }
        if !ptr.is_null() {
            match stmt_get(kv, ptr)? {
                Some((t, f))
                    if f & flag::MEMBERSHIP != 0 && Some(t.p) == in_id && t.s == x && t.o == g => {}
                other => {
                    return Ok(Some(format!(
                        "membership pointer ({g},{x})->{ptr} has stmt {other:?}"
                    )));
                }
            }
        }
    }
    Ok(None)
}

// ---- writes -------------------------------------------------------------------

impl Store<'_> {
    /// The ID of a system IRI, interning it on first use.
    pub(crate) fn sys(&mut self, k: &str, iri: &str) -> Result<Id> {
        if let Some(id) = sys_id(self.ro(), k)? {
            return Ok(id);
        }
        let id = self.dict.intern_iri(self.kv, iri)?;
        tb::meta_put_u64(self.kv, k, id.raw())?;
        Ok(id)
    }

    /// Resolves a graph reference: a graph ID, or an IRI naming a graph. With
    /// `create`, a name seen for the first time creates the graph.
    pub fn graph(&mut self, t: &TermRef, create: bool) -> Result<Id> {
        match t {
            TermRef::Id(g) if matches!(g.kind(), Kind::Graph) => {
                if graph_exists(self.ro(), *g)? {
                    Ok(*g)
                } else {
                    Err(Error::NotFound(g.raw()))
                }
            }
            TermRef::Id(iri) if iri.kind() == Kind::Iri => self.graph_by_name(*iri, create),
            TermRef::Term(Term::Iri(n)) if n == DEFAULT_GRAPH_IRI => Ok(DEFAULT_GRAPH),
            TermRef::Term(Term::Iri(n)) if n == SYSTEM_GRAPH_IRI => Ok(SYSTEM_GRAPH),
            TermRef::Term(Term::Iri(n)) => {
                if !create {
                    let found = match self.dict.lookup(self.ro(), &Term::Iri(n.clone()))? {
                        Some(iri) => graph_by_name_id(self.ro(), iri)?,
                        None => None,
                    };
                    return found.ok_or_else(|| Error::Invalid(format!("no graph named <{n}>")));
                }
                let iri = self.dict.intern_iri(self.kv, n)?;
                self.interned.push(iri);
                self.graph_by_name(iri, create)
            }
            _ => invalid("a graph is named by an IRI or a graph ID"),
        }
    }

    fn graph_by_name(&mut self, iri: Id, create: bool) -> Result<Id> {
        if let Some(g) = graph_by_name_id(self.ro(), iri)? {
            return Ok(g);
        }
        if !create {
            return Err(Error::NotFound(iri.raw()));
        }
        let g = self.alloc_chunked(SYSTEM_GRAPH, Kind::Graph, KV_GRAPH)?;
        self.kv.put(&tb::GNAME, &iri.be(), &g.le_bytes())?;
        self.kv.put(&tb::GNAME_REV, &g.be(), &iri.le_bytes())?;
        Ok(g)
    }

    /// `x` must be a live, asserted statement or hyperedge that has graph
    /// memberships of its own.
    fn check_member_item(&self, x: Id) -> Result<()> {
        let ok = match x.kind() {
            Kind::Statement => stmt_get(self.ro(), x)?
                .is_some_and(|(_, f)| f & (flag::UNASSERTED | flag::ROLE | flag::MEMBERSHIP) == 0),
            Kind::Hyperedge => self.ro().get(&tb::HEDGE, &x.be())?.is_some(),
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(Error::NotFound(x.raw()))
        }
    }

    /// Adds a membership of `g` to `x` (rule `gr-insert-existing`): an extra
    /// membership, or removal of the `moved` tombstone when `g` is its home.
    /// Returns false when `x` already is a direct member of `g`.
    pub fn add_member(&mut self, x: Id, g: Id) -> Result<bool> {
        self.check_member_item(x)?;
        if !graph_exists(self.ro(), g)? {
            return Err(Error::NotFound(g.raw()));
        }
        let h = home(self.ro(), x)?;
        match gmem_get(self.ro(), g, x)? {
            Some((f, ptr)) if g == h && f & gflag::MOVED != 0 => {
                if ptr.is_null() {
                    gmem_del(self.kv, g, x)?;
                } else {
                    gmem_put(self.kv, g, x, gflag::HOME, ptr)?;
                }
                Ok(true)
            }
            Some(_) => Ok(false),
            None if g == h => Ok(false),
            None => {
                tb::set_feature(self.kv, feature::MEMBERSHIP)?;
                gmem_put(self.kv, g, x, 0, Id(0))?;
                Ok(true)
            }
        }
    }

    /// Removes the membership of `x` in `g` (rule `gr-delete-in-graph`). When
    /// it was the last one, `x` is deleted and cascades; otherwise only the
    /// membership goes, with its promoted statement if any. Returns the deleted
    /// IDs. Fails with `NotFound` when `x` is not a direct member of `g`.
    pub fn remove_member(&mut self, x: Id, g: Id) -> Result<Vec<Id>> {
        self.check_member_item(x)?;
        let gs = direct_graphs(self.ro(), x)?;
        if !gs.contains(&g) {
            return Err(Error::NotFound(x.raw()));
        }
        if gs.len() == 1 {
            return self.cascade(&[x], DeleteMode::Cascade, 1);
        }
        self.drop_membership(x, g)
    }

    /// Removes one of several memberships; writes a tombstone for the home one.
    fn drop_membership(&mut self, x: Id, g: Id) -> Result<Vec<Id>> {
        let mut removed = Vec::new();
        if let Some((_, ptr)) = gmem_get(self.ro(), g, x)? {
            if !ptr.is_null() {
                removed = self.cascade(&[ptr], DeleteMode::Cascade, 1)?;
            }
        }
        if g == home(self.ro(), x)? {
            tb::set_feature(self.kv, feature::MEMBERSHIP)?;
            gmem_put(self.kv, g, x, gflag::HOME | gflag::MOVED, Id(0))?;
        } else {
            gmem_del(self.kv, g, x)?;
        }
        Ok(removed)
    }

    /// Moves `x` from `from` to `to` in one step (rule `gr-move`): it keeps its
    /// ID, home graph and annotations.
    pub fn move_member(&mut self, x: Id, from: Id, to: Id) -> Result<Vec<Id>> {
        if from == to {
            return Ok(Vec::new());
        }
        self.add_member(x, to)?;
        self.remove_member(x, from)
    }

    /// The statement ID of the membership `(x, :in, g)`, promoting it on first
    /// use in a statement chunk of `x`'s home graph (rule `gr-membership-promote`).
    pub fn membership_id(&mut self, x: Id, g: Id) -> Result<Id> {
        self.check_member_item(x)?;
        if !direct_graphs(self.ro(), x)?.contains(&g) {
            return Err(Error::NotFound(x.raw()));
        }
        let entry = gmem_get(self.ro(), g, x)?;
        if let Some((_, ptr)) = entry {
            if !ptr.is_null() {
                return Ok(ptr);
            }
        }
        let in_id = self.sys(mk::SYS_IN, crate::term::META_IN)?;
        let h = home(self.ro(), x)?;
        let id = self.alloc_stmt(h)?;
        self.write_stmt(
            id,
            Triple {
                s: x,
                p: in_id,
                o: g,
            },
            flag::MEMBERSHIP,
        )?;
        let f = match entry {
            Some((f, _)) => f,
            None if g == h => gflag::HOME,
            None => 0,
        };
        tb::set_feature(self.kv, feature::MEMBERSHIP)?;
        gmem_put(self.kv, g, x, f, id)?;
        Ok(id)
    }

    /// Called by `delete_one` for a promoted membership statement: clears the
    /// pointer, dropping an entry that only held it.
    pub(crate) fn unlink_membership(&mut self, id: Id, t: Triple) -> Result<()> {
        let (g, x) = (t.o, t.s);
        if let Some((f, ptr)) = gmem_get(self.ro(), g, x)? {
            if ptr == id {
                if f == gflag::HOME {
                    gmem_del(self.kv, g, x)?;
                } else {
                    gmem_put(self.kv, g, x, f, Id(0))?;
                }
            }
        }
        Ok(())
    }

    /// Called by `delete_one` for every deleted item: removes its `gmem` entries.
    pub(crate) fn forget_memberships(&mut self, x: Id) -> Result<()> {
        for g in gmem_graphs(self.ro(), x)? {
            gmem_del(self.kv, g, x)?;
        }
        Ok(())
    }

    fn check_nestable(&self, g: Id) -> Result<()> {
        if g == SYSTEM_GRAPH || g == DEFAULT_GRAPH {
            return invalid("the system and default graphs do not nest");
        }
        if !graph_exists(self.ro(), g)? {
            return Err(Error::NotFound(g.raw()));
        }
        Ok(())
    }

    /// Adds the nesting edge `(child, :in, parent)` (rules `gr-dag-cycle`,
    /// `gr-dag-depth`, `gr-in-star-closure`). Returns false when it exists.
    pub fn nest(&mut self, child: Id, parent: Id) -> Result<bool> {
        self.check_nestable(child)?;
        self.check_nestable(parent)?;
        if child == parent
            || self
                .ro()
                .get(&tb::IN_STAR, &key(&[parent, child]))?
                .is_some()
        {
            return Err(Error::GraphCycle {
                child: child.raw(),
                parent: parent.raw(),
            });
        }
        if self
            .ro()
            .get(&tb::GPARENT, &key(&[child, parent]))?
            .is_some()
        {
            return Ok(false);
        }
        let limit =
            tb::meta_u64(self.ro(), mk::DEPTH_LIMIT)?.map_or(DEFAULT_DEPTH_LIMIT, |v| v as u32);
        let below = depth(self.ro(), child, false, &mut HashMap::new())?;
        let above = depth(self.ro(), parent, true, &mut HashMap::new())?;
        if below + 1 + above > limit {
            return Err(Error::GraphDepthExceeded { limit });
        }
        tb::set_feature(self.kv, feature::NESTING)?;
        self.kv.put(&tb::GPARENT, &key(&[child, parent]), &[])?;
        self.kv.put(&tb::GCHILD, &key(&[parent, child]), &[])?;
        let mut ds = descendants(self.ro(), child)?;
        ds.push(child);
        let mut us = ancestors(self.ro(), parent)?;
        us.push(parent);
        for d in &ds {
            for u in &us {
                self.kv.put(&tb::IN_STAR, &key(&[*d, *u]), &[])?;
                self.kv.put(&tb::IN_STAR_REV, &key(&[*u, *d]), &[])?;
            }
        }
        Ok(true)
    }

    /// Removes the nesting edge `(child, :in, parent)` and recomputes the
    /// ancestors of `child` and its descendants in topological order (rule
    /// `gr-in-star-closure`, [[adr:0029]]). Returns false when it did not exist.
    pub fn unnest(&mut self, child: Id, parent: Id) -> Result<bool> {
        if self
            .ro()
            .get(&tb::GPARENT, &key(&[child, parent]))?
            .is_none()
        {
            return Ok(false);
        }
        self.kv.del(&tb::GPARENT, &key(&[child, parent]))?;
        self.kv.del(&tb::GCHILD, &key(&[parent, child]))?;
        let mut affected: HashSet<Id> = descendants(self.ro(), child)?.into_iter().collect();
        affected.insert(child);
        let order = topo_order(self.ro(), &affected)?;
        let mut anc: HashMap<Id, HashSet<Id>> = HashMap::new();
        for d in order {
            let mut new = HashSet::new();
            for p in parents(self.ro(), d)? {
                new.insert(p);
                match anc.get(&p) {
                    Some(a) => new.extend(a.iter().copied()),
                    None => new.extend(ancestors(self.ro(), p)?),
                }
            }
            let old: HashSet<Id> = ancestors(self.ro(), d)?.into_iter().collect();
            for u in old.difference(&new) {
                self.kv.del(&tb::IN_STAR, &key(&[d, *u]))?;
                self.kv.del(&tb::IN_STAR_REV, &key(&[*u, d]))?;
            }
            for u in new.difference(&old) {
                self.kv.put(&tb::IN_STAR, &key(&[d, *u]), &[])?;
                self.kv.put(&tb::IN_STAR_REV, &key(&[*u, d]), &[])?;
            }
            anc.insert(d, new);
        }
        Ok(true)
    }

    /// `DROP [RECURSIVE] GRAPH g` (rules `gr-drop-plain`, `gr-drop-cascade`,
    /// `gr-drop-recursive`). Returns every deleted item. Dropping the default
    /// graph clears it and keeps the graph.
    pub fn drop_graph(&mut self, g: Id, recursive: bool) -> Result<Vec<Id>> {
        if g == SYSTEM_GRAPH {
            return invalid("the system graph cannot be dropped");
        }
        if !graph_exists(self.ro(), g)? {
            return Err(Error::NotFound(g.raw()));
        }
        let mut graphs = vec![g];
        if recursive {
            // A descendant goes when every parent goes: then every path from it
            // to a root passes through g.
            let mut set: HashSet<Id> = descendants(self.ro(), g)?.into_iter().collect();
            set.insert(g);
            let mut doomed: HashSet<Id> = HashSet::from([g]);
            for d in topo_order(self.ro(), &set)? {
                if d != g && parents(self.ro(), d)?.iter().all(|p| doomed.contains(p)) {
                    doomed.insert(d);
                    graphs.push(d);
                }
            }
        }
        let mut removed = Vec::new();
        for d in graphs.into_iter().rev() {
            removed.extend(self.drop_one(d)?);
        }
        Ok(removed)
    }

    fn drop_one(&mut self, g: Id) -> Result<Vec<Id>> {
        let mut doomed = Vec::new();
        let mut removed = Vec::new();
        let mut items = Vec::new();
        for e in prefix_iter(self.ro(), &tb::GMEM, &g.be()) {
            let (k, v) = e?;
            if v[0] & gflag::HOME == 0 {
                items.push(Id::from_be(&k[8..]));
            }
        }
        for e in prefix_iter(self.ro(), &tb::GRAPH_CHUNKS, &g.be()) {
            let (k, _) = e?;
            let kind_value = u64::from_be_bytes(k[8..16].try_into().expect("8"));
            let chunk = u64::from_be_bytes(k[16..24].try_into().expect("8"));
            let (t, kind) = match kind_value {
                KV_STATEMENT => (&tb::STMT, Kind::Statement),
                KV_GRAPH => continue,
                _ => (&tb::HEDGE, Kind::Hyperedge),
            };
            for (x, v) in chunk_ids(self.ro(), t, kind, chunk)? {
                if kind == Kind::Statement {
                    let f = u16::from_le_bytes([v[24], v[25]]);
                    if f & (flag::UNASSERTED | flag::ROLE | flag::MEMBERSHIP) != 0 {
                        continue;
                    }
                }
                if gmem_get(self.ro(), g, x)?.is_some_and(|(f, _)| f & gflag::MOVED != 0) {
                    continue;
                }
                items.push(x);
            }
        }
        for x in items {
            if direct_graphs(self.ro(), x)?.len() > 1 {
                removed.extend(self.drop_membership(x, g)?);
            } else {
                doomed.push(x);
            }
        }
        let n = doomed.len();
        removed.extend(self.cascade(&doomed, DeleteMode::Cascade, n)?);
        if g == DEFAULT_GRAPH {
            // Inline properties of IRIs and blank nodes belong to the default graph.
            let mut nodes = Vec::new();
            for e in RangeIter::new(
                self.ro(),
                &tb::PROPS,
                Bound::Unbounded,
                Bound::Unbounded,
                false,
            ) {
                let node = Id::from_be(&e?.0);
                if matches!(node.kind(), Kind::Iri | Kind::Blank) {
                    nodes.push(node);
                }
            }
            for node in nodes {
                for (p, v) in store::load_props(self.ro(), node)?.entries {
                    if v.kind() != Kind::Statement {
                        self.remove_slot(node, p, v, false)?;
                    }
                }
            }
            return Ok(removed);
        }
        for c in children(self.ro(), g)? {
            self.unnest(c, g)?;
        }
        for p in parents(self.ro(), g)? {
            self.unnest(g, p)?;
        }
        if let Some(iri) = graph_name_id(self.ro(), g)? {
            self.kv.del(&tb::GNAME, &iri.be())?;
        }
        self.kv.del(&tb::GNAME_REV, &g.be())?;
        Ok(removed)
    }
}
