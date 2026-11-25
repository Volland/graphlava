//! SPARQL 1.2 Update ([[11-sparql]], [[10-ir#Operators#Write operators]]).
//! Each request is one transaction. Every operation's `WHERE` part reads the
//! state before its writes; deletes apply before inserts, and a delete plus
//! insert of the same subject and predicate through the same reifier is one
//! update in place (rule `ir-write-after-read`).

use std::collections::{HashMap, HashSet};

use spargebra::term::{
    GraphName, GraphNamePattern, GroundQuadPattern, GroundTermPattern, NamedNodePattern,
    QuadPattern, TermPattern,
};
use spargebra::{GraphUpdateOperation, SparqlParser};

use super::{QueryOptions, Translator, literal_term};
use crate::error::{Error, Result};
use crate::id::{DEFAULT_GRAPH, Id, Kind};
use crate::query::exec::Exec;
use crate::store::{DeleteMode, Store};
use crate::term::{Term, TermRef, Triple};

const RDF_REIFIES: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies";

/// What an update request did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateReport {
    pub inserted: usize,
    pub deleted: usize,
    pub updated_in_place: usize,
}

/// A term ready to write: a stored ID, or a term to intern.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum W {
    Id(Id),
    T(Term),
    /// A triple term `<<( s p o )>>`.
    Triple(Box<(W, W, W)>),
}

impl W {
    fn r(&self) -> TermRef {
        match self {
            W::Id(i) => TermRef::Id(*i),
            W::T(t) => TermRef::Term(t.clone()),
            W::Triple(_) => unreachable!("triple terms are resolved first"),
        }
    }
}

/// One instantiated quad.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Q {
    s: W,
    p: W,
    o: W,
    g: Option<W>,
}

fn bad<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(m.into()))
}

/// Parses and applies an update request through one store view.
pub fn update(st: &mut Store<'_>, text: &str, opts: &QueryOptions) -> Result<UpdateReport> {
    let mut p = SparqlParser::new();
    if let Some(b) = &opts.base_iri {
        p = p
            .with_base_iri(b)
            .map_err(|e| Error::Invalid(e.to_string()))?;
    }
    p = p
        .with_prefix("meta", "https://graphlava.org/ns#")
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let u = p
        .parse_update(&super::prepass(text))
        .map_err(|e| Error::Invalid(format!("SPARQL syntax error: {e}")))?;
    let mut rep = UpdateReport::default();
    for (k, op) in u.operations.iter().enumerate() {
        apply(st, op, opts, k, &mut rep)?;
    }
    Ok(rep)
}

fn apply(
    st: &mut Store<'_>,
    op: &GraphUpdateOperation,
    opts: &QueryOptions,
    k: usize,
    rep: &mut UpdateReport,
) -> Result<()> {
    match op {
        GraphUpdateOperation::InsertData { data } => {
            let quads: Vec<Q> = data
                .iter()
                .map(|q| Q {
                    s: subject_w(&q.subject),
                    p: W::T(Term::Iri(q.predicate.as_str().into())),
                    o: object_w(&q.object),
                    g: graph_w(&q.graph_name),
                })
                .collect();
            write(st, Vec::new(), quads, opts, rep)
        }
        GraphUpdateOperation::DeleteData { data } => {
            let quads: Vec<Q> = data
                .iter()
                .map(|q| Q {
                    s: W::T(Term::Iri(q.subject.as_str().into())),
                    p: W::T(Term::Iri(q.predicate.as_str().into())),
                    o: ground_w(&q.object),
                    g: graph_w(&q.graph_name),
                })
                .collect();
            write(st, quads, Vec::new(), opts, rep)
        }
        GraphUpdateOperation::DeleteInsert {
            delete,
            insert,
            using,
            pattern,
        } => {
            // Read part: evaluate the pattern on the state before any write.
            let (del, ins) = {
                let mut ex = Exec::new(st.ro(), st.dict);
                ex.standard = opts.standard;
                let ex = ex;
                let mut tr = Translator::new(&ex, opts.clone());
                let graph = tr.dataset(using.as_ref())?;
                let node = tr.pattern(pattern, graph)?;
                // template variables need slots too
                for q in delete {
                    for v in ground_quad_vars(q) {
                        tr.var(&v);
                    }
                }
                for q in insert {
                    for v in quad_vars(q) {
                        tr.var(&v);
                    }
                }
                let rows = ex.run(&node, tr.vars.len())?;
                let slots = super::slots(&tr.vars);
                let val = |name: &str, r: &[Id]| -> Result<Option<W>> {
                    let id = r[slots[name]];
                    if id.is_null() {
                        return Ok(None);
                    }
                    // Transient (computed) terms are written by value; a
                    // triple term becomes its statement on write.
                    if id.kind() == Kind::System {
                        return Ok(Some(term_w(ex.decode(id)?)));
                    }
                    Ok(Some(W::Id(id)))
                };
                let mut del = Vec::new();
                let mut ins = Vec::new();
                for (n, r) in rows.iter().enumerate() {
                    for q in delete {
                        if let Some(x) = inst_ground(q, r, &val)? {
                            del.push(x);
                        }
                    }
                    for q in insert {
                        if let Some(x) = inst(q, r, &val, n, k)? {
                            ins.push(x);
                        }
                    }
                }
                (del, ins)
            };
            write(st, del, ins, opts, rep)
        }
        GraphUpdateOperation::Load { silent, .. } => {
            if *silent {
                Ok(())
            } else {
                bad("LOAD is not supported in v0")
            }
        }
        GraphUpdateOperation::Clear { silent, graph }
        | GraphUpdateOperation::Drop { silent, graph } => {
            let keep = matches!(op, GraphUpdateOperation::Clear { .. });
            let targets = targets(st, graph)?;
            if targets.is_empty()
                && !*silent
                && matches!(graph, spargebra::algebra::GraphTarget::NamedNode(_))
            {
                return bad("no such graph");
            }
            for (g, name) in targets {
                rep.deleted += st.drop_graph(g, false)?.len();
                if keep {
                    if let Some(n) = name {
                        st.graph(&TermRef::Term(n), true)?;
                    }
                }
            }
            Ok(())
        }
        GraphUpdateOperation::Create { silent, graph } => {
            let name = Term::Iri(graph.as_str().into());
            let exists = crate::graph::graph_named(st.ro(), &st.dict, &name)?.is_some();
            if exists && !*silent {
                return bad(format!("graph <{}> already exists", graph.as_str()));
            }
            st.graph(&TermRef::Term(name), true)?;
            Ok(())
        }
    }
}

/// Graphs named by a `CLEAR`/`DROP` target, with their names.
fn targets(st: &Store<'_>, t: &spargebra::algebra::GraphTarget) -> Result<Vec<(Id, Option<Term>)>> {
    use spargebra::algebra::GraphTarget as T;
    let kv = st.ro();
    let named = || -> Result<Vec<(Id, Option<Term>)>> {
        let mut v = Vec::new();
        for g in crate::graph::all_graphs(kv)? {
            if g != DEFAULT_GRAPH && g != crate::id::SYSTEM_GRAPH {
                v.push((g, Some(st.dict.decode(kv, g)?)));
            }
        }
        Ok(v)
    };
    Ok(match t {
        T::NamedNode(n) => {
            let name = Term::Iri(n.as_str().into());
            match crate::graph::graph_named(kv, &st.dict, &name)? {
                Some(g) => vec![(g, Some(name))],
                None => Vec::new(),
            }
        }
        T::DefaultGraph => vec![(DEFAULT_GRAPH, None)],
        T::NamedGraphs => named()?,
        T::AllGraphs => {
            let mut v = named()?;
            v.push((DEFAULT_GRAPH, None));
            v
        }
    })
}

fn subject_w(s: &oxrdf::NamedOrBlankNode) -> W {
    match s {
        oxrdf::NamedOrBlankNode::NamedNode(n) => W::T(Term::Iri(n.as_str().into())),
        oxrdf::NamedOrBlankNode::BlankNode(b) => W::T(Term::Blank(b.as_str().into())),
    }
}

fn object_w(o: &oxrdf::Term) -> W {
    match o {
        oxrdf::Term::NamedNode(n) => W::T(Term::Iri(n.as_str().into())),
        oxrdf::Term::BlankNode(b) => W::T(Term::Blank(b.as_str().into())),
        oxrdf::Term::Literal(l) => W::T(literal_term(l)),
        oxrdf::Term::Triple(t) => W::Triple(Box::new((
            subject_w(&t.subject),
            W::T(Term::Iri(t.predicate.as_str().into())),
            object_w(&t.object),
        ))),
    }
}

fn term_w(t: Term) -> W {
    match t {
        Term::Triple(x) => {
            let x = *x;
            W::Triple(Box::new((
                term_w(x.subject),
                term_w(x.predicate),
                term_w(x.object),
            )))
        }
        t => W::T(t),
    }
}

fn ground_w(o: &spargebra::term::GroundTerm) -> W {
    use spargebra::term::GroundTerm as G;
    match o {
        G::NamedNode(n) => W::T(Term::Iri(n.as_str().into())),
        G::Literal(l) => W::T(literal_term(l)),
        G::Triple(t) => W::Triple(Box::new((
            W::T(Term::Iri(t.subject.as_str().into())),
            W::T(Term::Iri(t.predicate.as_str().into())),
            ground_w(&t.object),
        ))),
    }
}

fn graph_w(g: &GraphName) -> Option<W> {
    match g {
        GraphName::NamedNode(n) => Some(W::T(Term::Iri(n.as_str().into()))),
        GraphName::DefaultGraph => None,
    }
}

fn quad_vars(q: &QuadPattern) -> Vec<String> {
    let mut v = Vec::new();
    term_vars(&q.subject, &mut v);
    if let NamedNodePattern::Variable(x) = &q.predicate {
        v.push(x.as_str().into());
    }
    term_vars(&q.object, &mut v);
    if let GraphNamePattern::Variable(x) = &q.graph_name {
        v.push(x.as_str().into());
    }
    v
}

fn term_vars(t: &TermPattern, v: &mut Vec<String>) {
    match t {
        TermPattern::Variable(x) => v.push(x.as_str().into()),
        TermPattern::Triple(tp) => {
            term_vars(&tp.subject, v);
            if let NamedNodePattern::Variable(x) = &tp.predicate {
                v.push(x.as_str().into());
            }
            term_vars(&tp.object, v);
        }
        _ => {}
    }
}

fn ground_quad_vars(q: &GroundQuadPattern) -> Vec<String> {
    let mut v = Vec::new();
    gterm_vars(&q.subject, &mut v);
    if let NamedNodePattern::Variable(x) = &q.predicate {
        v.push(x.as_str().into());
    }
    gterm_vars(&q.object, &mut v);
    if let GraphNamePattern::Variable(x) = &q.graph_name {
        v.push(x.as_str().into());
    }
    v
}

fn gterm_vars(t: &GroundTermPattern, v: &mut Vec<String>) {
    match t {
        GroundTermPattern::Variable(x) => v.push(x.as_str().into()),
        GroundTermPattern::Triple(tp) => {
            gterm_vars(&tp.subject, v);
            if let NamedNodePattern::Variable(x) = &tp.predicate {
                v.push(x.as_str().into());
            }
            gterm_vars(&tp.object, v);
        }
        _ => {}
    }
}

type Val<'a> = dyn Fn(&str, &[Id]) -> Result<Option<W>> + 'a;

fn inst_term(t: &TermPattern, r: &[Id], val: &Val<'_>, row: usize, op: usize) -> Result<Option<W>> {
    Ok(match t {
        TermPattern::NamedNode(n) => Some(W::T(Term::Iri(n.as_str().into()))),
        // blank nodes in an INSERT template are fresh per solution
        TermPattern::BlankNode(b) => Some(W::T(Term::Blank(format!("{}_{op}_{row}", b.as_str())))),
        TermPattern::Literal(l) => Some(W::T(literal_term(l))),
        TermPattern::Variable(v) => val(v.as_str(), r)?,
        TermPattern::Triple(tp) => {
            let (Some(s), Some(p), Some(o)) = (
                inst_term(&tp.subject, r, val, row, op)?,
                inst_np(&tp.predicate, r, val)?,
                inst_term(&tp.object, r, val, row, op)?,
            ) else {
                return Ok(None);
            };
            Some(W::Triple(Box::new((s, p, o))))
        }
    })
}

fn inst_np(p: &NamedNodePattern, r: &[Id], val: &Val<'_>) -> Result<Option<W>> {
    Ok(match p {
        NamedNodePattern::NamedNode(n) => Some(W::T(Term::Iri(n.as_str().into()))),
        NamedNodePattern::Variable(v) => val(v.as_str(), r)?,
    })
}

fn inst_graph(g: &GraphNamePattern, r: &[Id], val: &Val<'_>) -> Result<Option<Option<W>>> {
    Ok(match g {
        GraphNamePattern::NamedNode(n) => Some(Some(W::T(Term::Iri(n.as_str().into())))),
        GraphNamePattern::DefaultGraph => Some(None),
        GraphNamePattern::Variable(v) => val(v.as_str(), r)?.map(Some),
    })
}

fn inst(q: &QuadPattern, r: &[Id], val: &Val<'_>, row: usize, op: usize) -> Result<Option<Q>> {
    let (Some(s), Some(p), Some(o), Some(g)) = (
        inst_term(&q.subject, r, val, row, op)?,
        inst_np(&q.predicate, r, val)?,
        inst_term(&q.object, r, val, row, op)?,
        inst_graph(&q.graph_name, r, val)?,
    ) else {
        return Ok(None);
    };
    Ok(Some(Q { s, p, o, g }))
}

fn inst_gterm(t: &GroundTermPattern, r: &[Id], val: &Val<'_>) -> Result<Option<W>> {
    Ok(match t {
        GroundTermPattern::NamedNode(n) => Some(W::T(Term::Iri(n.as_str().into()))),
        GroundTermPattern::Literal(l) => Some(W::T(literal_term(l))),
        GroundTermPattern::Variable(v) => val(v.as_str(), r)?,
        GroundTermPattern::Triple(tp) => {
            let (Some(s), Some(p), Some(o)) = (
                inst_gterm(&tp.subject, r, val)?,
                inst_np(&tp.predicate, r, val)?,
                inst_gterm(&tp.object, r, val)?,
            ) else {
                return Ok(None);
            };
            Some(W::Triple(Box::new((s, p, o))))
        }
    })
}

fn inst_ground(q: &GroundQuadPattern, r: &[Id], val: &Val<'_>) -> Result<Option<Q>> {
    let (Some(s), Some(p), Some(o), Some(g)) = (
        inst_gterm(&q.subject, r, val)?,
        inst_np(&q.predicate, r, val)?,
        inst_gterm(&q.object, r, val)?,
        inst_graph(&q.graph_name, r, val)?,
    ) else {
        return Ok(None);
    };
    Ok(Some(Q { s, p, o, g }))
}

fn is_reifies(p: &W) -> bool {
    matches!(p, W::T(Term::Iri(i)) if i == RDF_REIFIES)
}

/// The statement ID of `(s, p, o)` if it exists, without promoting.
fn lookup_stmt(st: &mut Store<'_>, s: &W, p: &W, o: &W) -> Result<Option<Id>> {
    let ex = Exec::new(st.ro(), st.dict);
    let id = |w: &W| -> Result<Option<Id>> {
        Ok(match w {
            W::Id(i) => Some(*i),
            W::T(Term::Blank(_)) => None,
            W::T(t) => {
                let i = ex.encode(t)?;
                (i.kind() != Kind::System).then_some(i)
            }
            W::Triple(_) => None,
        })
    };
    let (Some(s), Some(p), Some(o)) = (id(s)?, id(p)?, id(o)?) else {
        return Ok(None);
    };
    let t = crate::store::find(st.ro(), Some(s), Some(p), Some(o))?;
    Ok(t.into_iter().find_map(|m| m.id))
}

/// Resolves triple terms in subject or object position to statement IDs
/// (creating unasserted statements, [[adr:0032]]).
fn resolve_triple(st: &mut Store<'_>, w: &W) -> Result<W> {
    let W::Triple(b) = w else {
        return Ok(w.clone());
    };
    let (s, p, o) = &**b;
    let s = resolve_triple(st, s)?;
    let o = resolve_triple(st, o)?;
    if let Some(id) = lookup_stmt(st, &s, p, &o)? {
        return Ok(W::Id(id));
    }
    match st.statement_id(&s.r(), &p.r(), &o.r())? {
        Some(id) => Ok(W::Id(id)),
        None => Ok(W::Id(st.insert_unasserted(&s.r(), &p.r(), &o.r())?)),
    }
}

/// Applies deletes then inserts. Reifier quads (`r rdf:reifies <<( s p o )>>`)
/// bind `r`: a blank reifier is the statement ID, an IRI reifier becomes an
/// alias of it ([[adr:0032]]); a delete and an insert with the same reifier
/// and the same subject and predicate is one update in place.
fn write(
    st: &mut Store<'_>,
    del: Vec<Q>,
    ins: Vec<Q>,
    opts: &QueryOptions,
    rep: &mut UpdateReport,
) -> Result<()> {
    // Reifier bindings of the insert template.
    let mut reifiers: Vec<(W, (W, W, W))> = Vec::new();
    let mut plain_ins = Vec::new();
    for q in ins {
        if is_reifies(&q.p) {
            if let W::Triple(b) = &q.o {
                reifiers.push((q.s.clone(), (**b).clone()));
                continue;
            }
        }
        plain_ins.push(q);
    }
    let mut del_set: Vec<Q> = del.into_iter().filter(|q| !is_reifies(&q.p)).collect();
    // In-place updates: an existing statement ID reused for a new object.
    let mut in_place: HashSet<(W, W, W)> = HashSet::new();
    for (r, (s, p, o)) in &reifiers {
        let W::Id(rid) = r else { continue };
        let Some((t, _)) = crate::store::stmt_get(st.ro(), *rid)? else {
            continue;
        };
        let pos = del_set.iter().position(|q| {
            matches!(q.s, W::Id(x) if x == t.s) || q.s == *s
        } && (matches!(q.p, W::Id(x) if x == t.p) || q.p == *p)
            && same_w(st, &q.o, t.o).unwrap_or(false));
        let Some(pos) = pos else { continue };
        if !same_w(st, s, t.s)? || !same_w(st, p, t.p)? {
            continue;
        }
        del_set.remove(pos);
        let o2 = resolve_triple(st, o)?;
        st.update(*rid, &o2.r(), None)?;
        rep.updated_in_place += 1;
        in_place.insert((s.clone(), p.clone(), o.clone()));
    }
    // Deletes.
    for q in del_set {
        let (s, o) = (resolve_triple(st, &q.s)?, resolve_triple(st, &q.o)?);
        let g = match &q.g {
            Some(g) => Some(g.r()),
            None if opts.standard => Some(TermRef::Id(DEFAULT_GRAPH)),
            None => None,
        };
        if !opts.standard && is_meta_in(&q.p) {
            if membership_write(st, &s, &o, false)? {
                rep.deleted += 1;
            }
            continue;
        }
        if st.remove_in(g.as_ref(), &s.r(), &q.p.r(), &o.r(), DeleteMode::Cascade)? {
            rep.deleted += 1;
        }
    }
    // Inserts: asserted triples first, then reifiers, then the rest.
    let mut bnode_ids: HashMap<Term, Id> = HashMap::new();
    let subst = |w: &W, m: &HashMap<Term, Id>| match w {
        W::T(t @ Term::Blank(_)) | W::T(t @ Term::Iri(_)) if m.contains_key(t) => W::Id(m[t]),
        other => other.clone(),
    };
    let reifier_names: HashSet<W> = reifiers.iter().map(|(r, _)| r.clone()).collect();
    let (first, rest): (Vec<Q>, Vec<Q>) = plain_ins
        .into_iter()
        .partition(|q| !reifier_names.contains(&q.s) && !reifier_names.contains(&q.o));
    for q in first {
        if in_place.contains(&(q.s.clone(), q.p.clone(), q.o.clone())) {
            continue;
        }
        insert(st, &q, opts)?;
        rep.inserted += 1;
    }
    for (r, (s, p, o)) in &reifiers {
        let s = resolve_triple(st, &subst(s, &bnode_ids))?;
        let o = resolve_triple(st, &subst(o, &bnode_ids))?;
        let id = resolve_triple(st, &W::Triple(Box::new((s, p.clone(), o))))?;
        let W::Id(id) = id else { unreachable!() };
        match r {
            W::T(t @ Term::Blank(_)) => {
                bnode_ids.insert(t.clone(), id);
            }
            W::T(Term::Iri(iri)) => {
                let ex = Exec::new(st.ro(), st.dict);
                match ex.special_iri(iri)? {
                    Some(x) if x == id => {}
                    Some(x) => {
                        return Err(Error::Conflict(format!(
                            "reifier mismatch: <{iri}> names statement {x}, not {id}"
                        )));
                    }
                    None => {
                        drop(ex);
                        let a = st.intern(&TermRef::Term(Term::Iri(iri.clone())))?;
                        st.add_alias(a, id)?;
                    }
                }
                bnode_ids.insert(Term::Iri(iri.clone()), id);
            }
            W::Id(x) if *x == id => {}
            W::Id(x) => {
                return Err(Error::Conflict(format!(
                    "reifier mismatch: {x} is not the statement {id}"
                )));
            }
            _ => return bad("a reifier is a blank node or an IRI"),
        }
    }
    for q in rest {
        let q = Q {
            s: subst(&q.s, &bnode_ids),
            p: q.p.clone(),
            o: subst(&q.o, &bnode_ids),
            g: q.g.clone(),
        };
        insert(st, &q, opts)?;
        rep.inserted += 1;
    }
    Ok(())
}

fn same_w(st: &Store<'_>, w: &W, id: Id) -> Result<bool> {
    Ok(match w {
        W::Id(x) => *x == id,
        W::T(t) => {
            let ex = Exec::new(st.ro(), st.dict);
            ex.encode(t)? == id
        }
        W::Triple(_) => false,
    })
}

fn is_meta_in(p: &W) -> bool {
    matches!(p, W::T(Term::Iri(i)) if i == crate::term::META_IN)
}

/// `(x, meta:in, g)` written through SPARQL ([[01-data-model#Metagraph kinds]]):
/// between two graph names it is a nesting edge (rule `gr-dag-cycle` applies);
/// on a statement or hyperedge it is a membership. Returns whether anything
/// changed.
fn membership_write(st: &mut Store<'_>, s: &W, o: &W, add: bool) -> Result<bool> {
    let g = st.graph(&o.r(), add)?;
    let item = match s {
        W::Id(x) if matches!(x.kind(), Kind::Statement | Kind::Hyperedge) => Some(*x),
        _ => None,
    };
    match (item, add) {
        (Some(x), true) => st.add_member(x, g),
        (Some(x), false) => {
            st.remove_member(x, g)?;
            Ok(true)
        }
        (None, true) => {
            let child = st.graph(&s.r(), true)?;
            st.nest(child, g)
        }
        (None, false) => {
            let child = st.graph(&s.r(), false)?;
            st.unnest(child, g)
        }
    }
}

fn insert(st: &mut Store<'_>, q: &Q, opts: &QueryOptions) -> Result<()> {
    let s = resolve_triple(st, &q.s)?;
    let o = resolve_triple(st, &q.o)?;
    if !opts.standard && is_meta_in(&q.p) {
        membership_write(st, &s, &o, true)?;
        return Ok(());
    }
    if matches!(s, W::T(Term::Literal(_))) {
        return bad("a literal cannot be a subject");
    }
    // Writes without GRAPH go to the default graph created with the file
    // ([[adr:0038]]); an annotation (statement subject) goes with its statement.
    let g = match (&q.g, &s) {
        (Some(g), _) => Some(g.r()),
        (None, W::Id(x))
            if matches!(x.kind(), Kind::Statement | Kind::Hyperedge) && !opts.standard =>
        {
            None
        }
        (None, _) => Some(TermRef::Id(DEFAULT_GRAPH)),
    };
    let _ = Triple {
        s: Id(0),
        p: Id(0),
        o: Id(0),
    };
    st.insert_in(g.as_ref(), &s.r(), &q.p.r(), &o.r())?;
    Ok(())
}
