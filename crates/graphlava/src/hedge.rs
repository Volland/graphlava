//! Typed n-ary hyperedges ([[08-hyperedges]]): one `hedge` record per edge,
//! the `incidence` index, virtual role and type triples, per-(graph, type)
//! chunks, role promotion, literal attributes as inline properties
//! ([[adr:0028]]) and the role guard on delete ([[adr:0006]]).
//!
//! Type declarations live in `meta` under `htype:<id>` until the system graph
//! holds meta-model statements, like predicate declarations
//! ([[22-implementation-decisions#Predicate declarations in meta]]).

// @tg: implements:: [[openspec:hyperedges#Typed hyperedges with roles]]
// @tg: implements:: [[openspec:hyperedges#Literal attributes of hyperedges]]
// @tg: implements:: [[openspec:hyperedges#Role triples are queryable]]
// @tg: implements:: [[openspec:hyperedges#Hyperedges in graphs and type scans]]
// @tg: implements:: [[openspec:hyperedges#Annotating a role]]
// @tg: implements:: [[openspec:hyperedges#Hyperedge roles block delete]]

use std::collections::{BTreeSet, HashSet};
use std::ops::Bound;

use crate::error::{Error, Result, corrupted, invalid};
use crate::graph::{self, gflag};
use crate::id::{DEFAULT_GRAPH, Id, Kind};
use crate::storage::{KvRead, RangeIter, prefix_iter};
use crate::store::{Match, Store, flag, stmt_get};
use crate::tables::{self as tb, feature, key, mk};
use crate::term::{TermRef, Triple};

const RECORD_VERSION: u8 = 1;
const REC_FIXED: u8 = 1 << 0;
const REC_PROMOTED: u8 = 1 << 1;
const DECL_VERSION: u8 = 1;

/// One role of a hyperedge type as a caller declares it.
#[derive(Debug, Clone)]
pub struct RoleSpec {
    pub role: TermRef,
    /// Cardinality `many`; otherwise `one`.
    pub many: bool,
    pub required: bool,
    /// The node kind fillers must have, if any.
    pub kind: Option<Kind>,
}

impl RoleSpec {
    /// A required role of cardinality one.
    pub fn one(role: impl Into<TermRef>) -> RoleSpec {
        RoleSpec {
            role: role.into(),
            many: false,
            required: true,
            kind: None,
        }
    }
    /// An optional role of cardinality one.
    pub fn optional(role: impl Into<TermRef>) -> RoleSpec {
        RoleSpec {
            required: false,
            ..RoleSpec::one(role)
        }
    }
}

/// A hyperedge type declaration (rule `he-type-declared`): ordered roles and
/// literal attributes (rule `he-literal-attribute`).
#[derive(Debug, Clone, Default)]
pub struct HedgeTypeDecl {
    pub roles: Vec<RoleSpec>,
    pub attrs: Vec<TermRef>,
}

/// A stored role declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleDecl {
    pub role: Id,
    pub many: bool,
    pub required: bool,
    pub kind: Option<Kind>,
}

/// A stored hyperedge type.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HedgeType {
    pub roles: Vec<RoleDecl>,
    pub attrs: Vec<Id>,
}

impl HedgeType {
    /// Rule `he-type-fixed`: every role has cardinality one.
    pub fn fixed(&self) -> bool {
        self.roles.iter().all(|r| !r.many)
    }

    fn encode(&self) -> Vec<u8> {
        let mut v = vec![DECL_VERSION];
        v.extend_from_slice(&(self.roles.len() as u16).to_le_bytes());
        v.extend_from_slice(&(self.attrs.len() as u16).to_le_bytes());
        for r in &self.roles {
            v.extend_from_slice(&r.role.le_bytes());
            v.push(u8::from(r.many) | (u8::from(r.required) << 1));
            v.push(r.kind.map_or(0xFF, |k| k.tag() as u8));
        }
        for a in &self.attrs {
            v.extend_from_slice(&a.le_bytes());
        }
        v
    }

    fn decode(v: &[u8]) -> Result<HedgeType> {
        if v.len() < 5 || v[0] != DECL_VERSION {
            return Err(corrupted("bad hyperedge type declaration"));
        }
        let nr = u16::from_le_bytes([v[1], v[2]]) as usize;
        let na = u16::from_le_bytes([v[3], v[4]]) as usize;
        if v.len() != 5 + 10 * nr + 8 * na {
            return Err(corrupted("hyperedge type declaration length"));
        }
        let mut roles = Vec::with_capacity(nr);
        for i in 0..nr {
            let b = &v[5 + 10 * i..];
            roles.push(RoleDecl {
                role: Id::from_le(b),
                many: b[8] & 1 != 0,
                required: b[8] & 2 != 0,
                kind: kind_of_tag(b[9]),
            });
        }
        let attrs = (0..na)
            .map(|i| Id::from_le(&v[5 + 10 * nr + 8 * i..]))
            .collect();
        Ok(HedgeType { roles, attrs })
    }

    fn role(&self, r: Id) -> Option<&RoleDecl> {
        self.roles.iter().find(|d| d.role == r)
    }
}

fn kind_of_tag(t: u8) -> Option<Kind> {
    [
        Kind::Iri,
        Kind::Blank,
        Kind::Statement,
        Kind::Graph,
        Kind::Hyperedge,
        Kind::System,
    ]
    .into_iter()
    .find(|k| k.tag() as u8 == t)
}

fn htype_key(ty: Id) -> String {
    format!("htype:{:016x}", ty.raw())
}

/// The declaration of hyperedge type `ty`.
pub fn hedge_type(kv: &dyn KvRead, ty: Id) -> Result<Option<HedgeType>> {
    tb::meta_get(kv, &htype_key(ty))?
        .map(|v| HedgeType::decode(&v))
        .transpose()
}

/// Every declared hyperedge type.
pub fn hedge_types(kv: &dyn KvRead) -> Result<Vec<(Id, HedgeType)>> {
    let mut out = Vec::new();
    for e in prefix_iter(kv, &tb::META, b"htype:") {
        let (k, v) = e?;
        let n = u64::from_str_radix(std::str::from_utf8(&k[6..]).unwrap_or(""), 16)
            .map_err(|_| corrupted("bad htype key"))?;
        out.push((Id(n), HedgeType::decode(&v)?));
    }
    Ok(out)
}

/// A hyperedge as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyperedge {
    pub id: Id,
    pub ty: Id,
    /// The home graph (rule `gr-home-graph`).
    pub graph: Id,
    /// `(role, filler)` pairs: declared role order for a fixed-arity type,
    /// sorted by role then filler otherwise. Unfilled optional roles are absent.
    pub roles: Vec<(Id, Id)>,
    /// Whether any role edge has been promoted (record flag bit 1).
    pub promoted: bool,
}

fn encode_record(ty: Id, decl: &HedgeType, roles: &[(Id, Id)], promoted: bool) -> Vec<u8> {
    let fixed = decl.fixed();
    let mut flags = 0;
    if fixed {
        flags |= REC_FIXED;
    }
    if promoted {
        flags |= REC_PROMOTED;
    }
    let mut v = vec![RECORD_VERSION, flags, 0, 0];
    if fixed {
        v.extend_from_slice(&(decl.roles.len() as u32).to_le_bytes());
        v.extend_from_slice(&ty.le_bytes());
        for d in &decl.roles {
            let n = roles
                .iter()
                .find(|(r, _)| *r == d.role)
                .map_or(Id(0), |(_, n)| *n);
            v.extend_from_slice(&n.le_bytes());
        }
    } else {
        let mut pairs = roles.to_vec();
        pairs.sort();
        v.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
        v.extend_from_slice(&ty.le_bytes());
        for (r, n) in pairs {
            v.extend_from_slice(&r.le_bytes());
            v.extend_from_slice(&n.le_bytes());
        }
    }
    v
}

fn decode_record(kv: &dyn KvRead, e: Id, v: &[u8]) -> Result<Hyperedge> {
    if v.len() < 16 || v[0] != RECORD_VERSION {
        return Err(corrupted("bad hyperedge record"));
    }
    let k = u32::from_le_bytes(v[4..8].try_into().expect("4")) as usize;
    let ty = Id::from_le(&v[8..]);
    let mut roles = Vec::with_capacity(k);
    if v[1] & REC_FIXED != 0 {
        if v.len() != 16 + 8 * k {
            return Err(corrupted("hyperedge record length"));
        }
        let decl = hedge_type(kv, ty)?.ok_or_else(|| corrupted("hyperedge of undeclared type"))?;
        for (i, d) in decl.roles.iter().enumerate().take(k) {
            let n = Id::from_le(&v[16 + 8 * i..]);
            if !n.is_null() {
                roles.push((d.role, n));
            }
        }
    } else {
        if v.len() != 16 + 16 * k {
            return Err(corrupted("hyperedge record length"));
        }
        for i in 0..k {
            roles.push((
                Id::from_le(&v[16 + 16 * i..]),
                Id::from_le(&v[24 + 16 * i..]),
            ));
        }
    }
    Ok(Hyperedge {
        id: e,
        ty,
        graph: graph::home(kv, e)?,
        roles,
        promoted: v[1] & REC_PROMOTED != 0,
    })
}

/// One `hedge` point read.
pub fn hedge(kv: &dyn KvRead, e: Id) -> Result<Option<Hyperedge>> {
    if e.kind() != Kind::Hyperedge {
        return Ok(None);
    }
    match kv.get(&tb::HEDGE, &e.be())? {
        Some(v) => decode_record(kv, e, &v).map(Some),
        None => Ok(None),
    }
}

/// The type of a hyperedge, from its chunk alone (rule `he-record-type`).
pub fn type_of(kv: &dyn KvRead, e: Id) -> Result<Option<Id>> {
    Ok(graph::chunk_owner(kv, e.chunk())?.map(|(_, kv)| Id(kv)))
}

/// Every hyperedge of type `ty`: the `kind_chunk` ranges over `hedge`.
pub fn hedges_of_type(kv: &dyn KvRead, ty: Id) -> Result<Vec<Id>> {
    let mut out = Vec::new();
    for e in prefix_iter(kv, &tb::KIND_CHUNK, &ty.be()) {
        let (k, _) = e?;
        let chunk = u64::from_be_bytes(k[8..16].try_into().expect("8"));
        out.extend(
            graph::chunk_ids(kv, &tb::HEDGE, Kind::Hyperedge, chunk)?
                .into_iter()
                .map(|(id, _)| id),
        );
    }
    Ok(out)
}

/// Hyperedges whose direct graph is `g`: its hyperedge chunks minus
/// tombstones, plus extra members. With `ty`, only that type's chunks.
pub fn direct_hedges_of(kv: &dyn KvRead, g: Id, ty: Option<Id>) -> Result<Vec<Id>> {
    let mut out = BTreeSet::new();
    let prefix = match ty {
        Some(t) => key(&[g, t]),
        None => g.be().to_vec(),
    };
    for e in prefix_iter(kv, &tb::GRAPH_CHUNKS, &prefix) {
        let (k, _) = e?;
        let kind_value = u64::from_be_bytes(k[8..16].try_into().expect("8"));
        if kind_value == graph::KV_STATEMENT || kind_value == graph::KV_GRAPH {
            continue;
        }
        let chunk = u64::from_be_bytes(k[16..24].try_into().expect("8"));
        for (x, _) in graph::chunk_ids(kv, &tb::HEDGE, Kind::Hyperedge, chunk)? {
            if graph::gmem_get(kv, g, x)?.is_some_and(|(f, _)| f & gflag::MOVED != 0) {
                continue;
            }
            out.insert(x);
        }
    }
    for e in prefix_iter(kv, &tb::GMEM, &g.be()) {
        let (k, v) = e?;
        let x = Id::from_be(&k[8..]);
        if v[0] & gflag::HOME == 0
            && x.kind() == Kind::Hyperedge
            && ty.is_none_or(|t| type_of(kv, x).ok().flatten() == Some(t))
        {
            out.insert(x);
        }
    }
    Ok(out.into_iter().collect())
}

pub(crate) fn direct_hedges(kv: &dyn KvRead, g: Id) -> Result<Vec<Id>> {
    direct_hedges_of(kv, g, None)
}

/// Hyperedges of type `ty` in the scope of `g` (transitive unless `direct`),
/// each once, without reading other items' records.
pub fn hedges_of_type_in(kv: &dyn KvRead, ty: Id, g: Id, direct: bool) -> Result<Vec<Id>> {
    let mut out = BTreeSet::new();
    for d in graph::scope_graphs(kv, g, direct)? {
        out.extend(direct_hedges_of(kv, d, Some(ty))?);
    }
    Ok(out.into_iter().collect())
}

/// `incidence` prefix scan: the hyperedges node `n` fills, as
/// `(role, type, hyperedge)`, optionally narrowed by role and type.
pub fn incidence(
    kv: &dyn KvRead,
    n: Id,
    role: Option<Id>,
    ty: Option<Id>,
) -> Result<Vec<(Id, Id, Id)>> {
    let prefix = match (role, ty) {
        (Some(r), Some(t)) => key(&[n, r, t]),
        (Some(r), None) => key(&[n, r]),
        _ => n.be().to_vec(),
    };
    let mut out = Vec::new();
    for e in prefix_iter(kv, &tb::INCIDENCE, &prefix) {
        let (k, _) = e?;
        let (r, t, e) = (
            Id::from_be(&k[8..]),
            Id::from_be(&k[16..]),
            Id::from_be(&k[24..]),
        );
        if ty.is_none_or(|x| x == t) {
            out.push((r, t, e));
        }
    }
    Ok(out)
}

/// The hyperedges in which `n` fills a role (for the delete guard).
pub fn role_holders(kv: &dyn KvRead, n: Id) -> Result<Vec<Id>> {
    let mut v: Vec<Id> = incidence(kv, n, None, None)?
        .into_iter()
        .map(|(_, _, e)| e)
        .collect();
    v.sort();
    v.dedup();
    Ok(v)
}

/// The promoted statement of role edge `(e, r, n)`, if any.
pub fn role_id(kv: &dyn KvRead, e: Id, r: Id, n: Id) -> Result<Option<Id>> {
    Ok(kv
        .get(&tb::HEDGE_PROMO, &key(&[e, r, n]))?
        .map(|v| Id::from_le(&v)))
}

/// Statements that exist only because of item `y` and go with it: its
/// promoted memberships and, for a hyperedge, its promoted role edges.
pub fn dependents(kv: &dyn KvRead, y: Id) -> Result<Vec<Id>> {
    let mut out = Vec::new();
    for e in prefix_iter(kv, &tb::GMEM_REV, &y.be()) {
        let (k, _) = e?;
        if let Some((_, ptr)) = graph::gmem_get(kv, Id::from_be(&k[8..]), y)? {
            if !ptr.is_null() {
                out.push(ptr);
            }
        }
    }
    if y.kind() == Kind::Hyperedge {
        for e in prefix_iter(kv, &tb::HEDGE_PROMO, &y.be()) {
            out.push(Id::from_le(&e?.1));
        }
    }
    Ok(out)
}

/// Virtual role triples `(e, r, n)` and type triples `(e, rdf:type, T)`
/// matching a pattern (rule `he-virtual-role`), appended to `out`. With `e`
/// unbound and the filler bound, they come from `incidence`; with `e` bound,
/// from `hedge`.
pub(crate) fn virtual_matches(
    kv: &dyn KvRead,
    s: Option<Id>,
    p: Option<Id>,
    o: Option<Id>,
    out: &mut Vec<Match>,
) -> Result<()> {
    if o.is_some_and(|o| o.is_literal()) {
        return Ok(());
    }
    let type_id = graph::sys_id(kv, mk::SYS_TYPE)?;
    let emit = |h: &Hyperedge, out: &mut Vec<Match>| -> Result<()> {
        if let Some(t) = type_id {
            if p.is_none_or(|p| p == t) && o.is_none_or(|o| o == h.ty) {
                out.push(Match {
                    s: h.id,
                    p: t,
                    o: h.ty,
                    id: None,
                });
            }
        }
        for (r, n) in &h.roles {
            if p.is_none_or(|p| p == *r) && o.is_none_or(|o| o == *n) {
                out.push(Match {
                    s: h.id,
                    p: *r,
                    o: *n,
                    id: role_id(kv, h.id, *r, *n)?,
                });
            }
        }
        Ok(())
    };
    match (s, o) {
        (Some(e), _) => {
            if let Some(h) = hedge(kv, e)? {
                emit(&h, out)?;
            }
        }
        (None, Some(n)) => {
            if p.is_none_or(|p| Some(p) != type_id) {
                for (r, _, e) in incidence(kv, n, p, None)? {
                    out.push(Match {
                        s: e,
                        p: r,
                        o: n,
                        id: role_id(kv, e, r, n)?,
                    });
                }
            }
            if let Some(t) = type_id {
                if p.is_none_or(|p| p == t) && hedge_type(kv, n)?.is_some() {
                    for e in hedges_of_type(kv, n)? {
                        out.push(Match {
                            s: e,
                            p: t,
                            o: n,
                            id: None,
                        });
                    }
                }
            }
        }
        (None, None) => {
            for (ty, decl) in hedge_types(kv)? {
                let relevant = p.is_none_or(|p| Some(p) == type_id || decl.role(p).is_some());
                if !relevant {
                    continue;
                }
                for e in hedges_of_type(kv, ty)? {
                    if let Some(h) = hedge(kv, e)? {
                        emit(&h, out)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Checks rule `he-incidence-complete` and the promotion pointers: the
/// `incidence` keys are exactly the non-null fillers of the records, every
/// record's type equals its chunk's kind, and every `hedge_promo` entry names a
/// promoted role statement of a current filler.
pub fn check_invariants(kv: &dyn KvRead) -> Result<Option<String>> {
    let mut expected = HashSet::new();
    let mut fillers = HashSet::new();
    for e in RangeIter::new(kv, &tb::HEDGE, Bound::Unbounded, Bound::Unbounded, false) {
        let (k, v) = e?;
        let h = decode_record(kv, Id::from_be(&k), &v)?;
        if type_of(kv, h.id)? != Some(h.ty) {
            return Ok(Some(format!(
                "hyperedge {} type differs from its chunk",
                h.id
            )));
        }
        for (r, n) in &h.roles {
            expected.insert(key(&[*n, *r, h.ty, h.id]));
            fillers.insert((h.id, *r, *n));
        }
    }
    let mut found = HashSet::new();
    for e in RangeIter::new(
        kv,
        &tb::INCIDENCE,
        Bound::Unbounded,
        Bound::Unbounded,
        false,
    ) {
        found.insert(e?.0);
    }
    if found != expected {
        return Ok(Some(format!(
            "incidence has {} keys, the records have {} fillers",
            found.len(),
            expected.len()
        )));
    }
    for e in RangeIter::new(
        kv,
        &tb::HEDGE_PROMO,
        Bound::Unbounded,
        Bound::Unbounded,
        false,
    ) {
        let (k, v) = e?;
        let (he, r, n) = (Id::from_be(&k), Id::from_be(&k[8..]), Id::from_be(&k[16..]));
        let id = Id::from_le(&v);
        if !fillers.contains(&(he, r, n)) {
            return Ok(Some(format!("hedge_promo ({he},{r},{n}) is not a filler")));
        }
        match stmt_get(kv, id)? {
            Some((t, f)) if f & flag::ROLE != 0 && t == (Triple { s: he, p: r, o: n }) => {}
            other => {
                return Ok(Some(format!("role pointer {id} has stmt {other:?}")));
            }
        }
    }
    Ok(None)
}

// ---- writes -------------------------------------------------------------------

impl Store<'_> {
    /// Declares or extends a hyperedge type (rules `he-type-declared`,
    /// `he-type-change`, `dm-metamodel-validate`). While records of the type
    /// exist, only optional roles appended at the end and new attributes are
    /// accepted; anything else needs a migration.
    pub fn declare_hedge_type(&mut self, ty: &TermRef, decl: &HedgeTypeDecl) -> Result<Id> {
        let ty = self.intern(ty)?;
        if ty.kind() != Kind::Iri {
            return invalid("a hyperedge type is named by an IRI");
        }
        let mut roles = Vec::with_capacity(decl.roles.len());
        for r in &decl.roles {
            let role = self.intern(&r.role)?;
            if role.kind() != Kind::Iri {
                return invalid("a role is named by an IRI");
            }
            if roles.iter().any(|d: &RoleDecl| d.role == role) {
                return invalid("a role is declared twice");
            }
            roles.push(RoleDecl {
                role,
                many: r.many,
                required: r.required,
                kind: r.kind,
            });
        }
        let mut attrs = Vec::with_capacity(decl.attrs.len());
        for a in &decl.attrs {
            let a = self.intern(a)?;
            if a.kind() != Kind::Iri || roles.iter().any(|d| d.role == a) || attrs.contains(&a) {
                return invalid("an attribute is an IRI distinct from the roles");
            }
            attrs.push(a);
        }
        let new = HedgeType { roles, attrs };
        if let Some(old) = hedge_type(self.ro(), ty)? {
            let has_records = !hedges_of_type(self.ro(), ty)?.is_empty();
            let compatible = new.roles.len() >= old.roles.len()
                && new.roles[..old.roles.len()] == old.roles[..]
                && new.roles[old.roles.len()..].iter().all(|r| !r.required)
                && (old.fixed() == new.fixed())
                && old.attrs.iter().all(|a| new.attrs.contains(a));
            if has_records && !compatible {
                return invalid(
                    "changing or removing roles of a type with records needs a migration (rule he-type-change)",
                );
            }
        }
        tb::set_feature(self.kv, feature::HYPEREDGES)?;
        tb::meta_put(self.kv, &htype_key(ty), &new.encode())?;
        Ok(ty)
    }

    /// Validates `(role, filler)` pairs against a type (rule `he-type-declared`).
    fn validate_roles(
        &mut self,
        decl: &HedgeType,
        roles: &[(TermRef, TermRef)],
    ) -> Result<Vec<(Id, Id)>> {
        let mut filled: Vec<(Id, Id)> = Vec::with_capacity(roles.len());
        for (r, n) in roles {
            let r = self.intern(r)?;
            let Some(rd) = decl.role(r).copied() else {
                return Err(Error::HyperedgeShapeMismatch(format!(
                    "role {r} is not declared by the type"
                )));
            };
            let n = self.intern(n)?;
            self.check_filler(&rd, n)?;
            if filled.contains(&(r, n)) {
                return Err(Error::HyperedgeShapeMismatch(format!(
                    "role {r} is filled twice by {n}"
                )));
            }
            filled.push((r, n));
        }
        for d in &decl.roles {
            let c = filled.iter().filter(|(r, _)| *r == d.role).count();
            if !d.many && c > 1 {
                return Err(Error::HyperedgeShapeMismatch(format!(
                    "role {} has cardinality one",
                    d.role
                )));
            }
            if d.required && c == 0 {
                return Err(Error::HyperedgeShapeMismatch(format!(
                    "required role {} is missing",
                    d.role
                )));
            }
        }
        Ok(filled)
    }

    fn check_filler(&self, rd: &RoleDecl, n: Id) -> Result<()> {
        if n.is_literal() || n.is_null() {
            return Err(Error::HyperedgeShapeMismatch(format!(
                "role {} needs a node; a literal value is an attribute (rule he-literal-attribute)",
                rd.role
            )));
        }
        if rd.kind.is_some_and(|k| k != n.kind()) {
            return Err(Error::HyperedgeShapeMismatch(format!(
                "role {} needs a {:?}",
                rd.role,
                rd.kind.expect("kind")
            )));
        }
        if n.kind() == Kind::Statement
            && stmt_get(self.ro(), n)?.is_some_and(|(_, f)| f & flag::UNASSERTED != 0)
        {
            return invalid("an unasserted statement cannot fill a role");
        }
        Ok(())
    }

    /// Creates a hyperedge of type `ty` in `graph` (default: the default
    /// graph): one `hedge` record, one `incidence` key per filler, and each
    /// literal attribute as an inline property of the hyperedge.
    pub fn create_hedge(
        &mut self,
        ty: &TermRef,
        graph: Option<&TermRef>,
        roles: &[(TermRef, TermRef)],
        attrs: &[(TermRef, TermRef)],
    ) -> Result<Id> {
        let ty = self.intern(ty)?;
        let decl = hedge_type(self.ro(), ty)?.ok_or_else(|| {
            Error::HyperedgeShapeMismatch(format!("{ty} is not a declared hyperedge type"))
        })?;
        let filled = self.validate_roles(&decl, roles)?;
        let mut values = Vec::with_capacity(attrs.len());
        for (p, o) in attrs {
            let p = self.intern(p)?;
            if !decl.attrs.contains(&p) {
                return Err(Error::HyperedgeShapeMismatch(format!(
                    "{p} is not an attribute of the type"
                )));
            }
            let o = self.intern(o)?;
            if !o.is_literal() {
                return Err(Error::HyperedgeShapeMismatch(format!(
                    "attribute {p} takes a literal; a node is a role"
                )));
            }
            values.push((p, o));
        }
        let g = match graph {
            Some(g) => self.graph(g, true)?,
            None => DEFAULT_GRAPH,
        };
        let e = self.alloc_chunked(g, Kind::Hyperedge, ty.raw())?;
        debug_assert!(
            self.ro().get(&tb::HEDGE, &e.be())?.is_none(),
            "assumption id-never-reused: fresh hyperedge {e} already exists"
        );
        tb::set_feature(self.kv, feature::HYPEREDGES)?;
        self.kv.put(
            &tb::HEDGE,
            &e.be(),
            &encode_record(ty, &decl, &filled, false),
        )?;
        for (r, n) in &filled {
            self.kv.put(&tb::INCIDENCE, &key(&[*n, *r, ty, e]), &[])?;
        }
        for (p, o) in values {
            self.insert_ids_in(e, p, o, g)?;
        }
        Ok(e)
    }

    fn put_record(&mut self, h: &Hyperedge) -> Result<()> {
        let decl = hedge_type(self.ro(), h.ty)?.ok_or_else(|| corrupted("undeclared type"))?;
        self.kv.put(
            &tb::HEDGE,
            &h.id.be(),
            &encode_record(h.ty, &decl, &h.roles, h.promoted),
        )
    }

    /// Changes the filler of a role of cardinality one in place: the record
    /// slot, one `incidence` key out and one in. The hyperedge keeps its ID and
    /// annotations; a promoted role edge keeps its statement ID (rule
    /// `he-promote`).
    pub fn update_role(&mut self, e: Id, role: &TermRef, filler: &TermRef) -> Result<()> {
        let mut h = hedge(self.ro(), e)?.ok_or(Error::NotFound(e.raw()))?;
        let decl = hedge_type(self.ro(), h.ty)?.ok_or_else(|| corrupted("undeclared type"))?;
        let r = self.intern(role)?;
        let rd = *decl
            .role(r)
            .ok_or_else(|| Error::HyperedgeShapeMismatch(format!("role {r} is not declared")))?;
        if rd.many {
            return invalid("update_role changes a role of cardinality one");
        }
        let n = self.intern(filler)?;
        self.check_filler(&rd, n)?;
        let old = h.roles.iter().position(|(x, _)| *x == r);
        if old.is_some_and(|i| h.roles[i].1 == n) {
            return Ok(());
        }
        match old {
            Some(i) => {
                let was = h.roles[i].1;
                self.kv.del(&tb::INCIDENCE, &key(&[was, r, h.ty, e]))?;
                if let Some(sid) = role_id(self.ro(), e, r, was)? {
                    self.kv.del(&tb::HEDGE_PROMO, &key(&[e, r, was]))?;
                    self.kv
                        .put(&tb::HEDGE_PROMO, &key(&[e, r, n]), &sid.le_bytes())?;
                    self.write_stmt(sid, Triple { s: e, p: r, o: n }, flag::ROLE)?;
                }
                h.roles[i].1 = n;
            }
            None => h.roles.push((r, n)),
        }
        self.kv.put(&tb::INCIDENCE, &key(&[n, r, h.ty, e]), &[])?;
        self.put_record(&h)
    }

    /// The statement ID of role edge `(e, r, n)`, promoting it on first use in
    /// a statement chunk of the hyperedge's home graph (rules `he-promote`,
    /// `dm-annotation-targets`). It stays served from `hedge` and `incidence`.
    pub fn role_statement(&mut self, e: Id, r: Id, n: Id) -> Result<Id> {
        let mut h = hedge(self.ro(), e)?.ok_or(Error::NotFound(e.raw()))?;
        if !h.roles.contains(&(r, n)) {
            return Err(Error::NotFound(n.raw()));
        }
        if let Some(id) = role_id(self.ro(), e, r, n)? {
            return Ok(id);
        }
        let id = self.alloc_stmt(h.graph)?;
        self.write_stmt(id, Triple { s: e, p: r, o: n }, flag::ROLE)?;
        self.kv
            .put(&tb::HEDGE_PROMO, &key(&[e, r, n]), &id.le_bytes())?;
        if !h.promoted {
            h.promoted = true;
            self.put_record(&h)?;
        }
        Ok(id)
    }

    /// Called by `delete_one`: removes the record, its incidence keys and its
    /// promotion entries (the promoted statements are deleted by the cascade).
    pub(crate) fn delete_hedge_record(&mut self, e: Id) -> Result<()> {
        let Some(h) = hedge(self.ro(), e)? else {
            return Ok(());
        };
        for (r, n) in &h.roles {
            self.kv.del(&tb::INCIDENCE, &key(&[*n, *r, h.ty, e]))?;
            self.kv.del(&tb::HEDGE_PROMO, &key(&[e, *r, *n]))?;
        }
        self.kv.del(&tb::HEDGE, &e.be())?;
        Ok(())
    }

    /// Called by `delete_one` for a promoted role statement deleted with its
    /// hyperedge.
    pub(crate) fn unlink_role(&mut self, id: Id, t: Triple) -> Result<()> {
        if role_id(self.ro(), t.s, t.p, t.o)? == Some(id) {
            self.kv.del(&tb::HEDGE_PROMO, &key(&[t.s, t.p, t.o]))?;
        }
        Ok(())
    }
}
