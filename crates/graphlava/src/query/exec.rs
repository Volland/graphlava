//! The executor ([[14-planner-execution]]): evaluates IR nodes over one read
//! view. Rows are full-width (one slot per query variable, `0` unbound) and
//! flow in batches of at most [`BATCH`] rows through index nested-loop joins
//! (rule `plan-inlj`): each batch's distinct probe keys are sorted and each
//! key probes the store once. Terms are decoded only by expressions, ordering
//! and the final projection (rule `plan-late-materialize`).

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::time::Instant;

use crate::dict::Dict;
use crate::error::{Error, Result};
use crate::graph;
use crate::id::{DEFAULT_GRAPH, Id, Kind, SYSTEM_GRAPH};
use crate::literal;
use crate::query::func::{self, ExprError, FnContext};
use crate::query::ir::*;
use crate::storage::KvRead;
use crate::store::{self, Match};
use crate::term::{Literal, Term, TripleTerm};

const RDF_REIFIES: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies";
/// Label prefix of a statement reifier read as a blank node in standard mode.
const STMT_BLANK: &str = "stmt";

type TermTable = RefCell<(Vec<Term>, HashMap<Term, Id>)>;
type ScopeCache = RefCell<HashMap<(Id, bool), std::rc::Rc<HashSet<Id>>>>;
/// Datalog relations: name → (full, delta).
pub type RelTable = RefCell<HashMap<String, (Vec<Row>, Vec<Row>)>>;
type ProbeKey = (Option<Id>, Option<Id>, Option<Id>, Option<Id>);

/// Rows per batch (rule `plan-batch`).
pub const BATCH: usize = 1024;

/// A row: one ID per slot.
pub type Row = Vec<Id>;

/// Transient IDs for terms computed during a query that are not in the
/// dictionary (`BIND(CONCAT(…))`, aggregates). They live in the `System` kind
/// above bit 59 and never reach the store.
const TRANSIENT: u64 = (7u64 << 60) | (1u64 << 59);

/// Limits of one query (rule `dl-on-demand-limits`, `ir-errors`).
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub max_rows: usize,
    pub deadline: Option<Instant>,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            max_rows: 10_000_000,
            deadline: None,
        }
    }
}

/// One evaluation over a read view.
#[allow(missing_debug_implementations)]
pub struct Exec<'a> {
    pub kv: &'a dyn KvRead,
    pub dict: Dict,
    pub budget: Budget,
    transient: TermTable,
    fctx: RefCell<FnContext>,
    scopes: ScopeCache,
    /// Datalog relations visible to `Rel` scans: (full, delta).
    pub rels: RelTable,
    /// Standard SPARQL mode: virtual membership triples are not matched.
    pub standard: bool,
    /// Plans that wanted an on-demand ordering (PSO, SOP, OPS) that is not
    /// ready, counted for the background build of [[adr:0041]].
    pub wanted: std::cell::Cell<[u64; 3]>,
    /// Statistics for join ordering, when loaded.
    pub stats: Option<crate::query::plan::Stats>,
    /// Counters for tests of join behaviour: store probes issued.
    pub probes: std::cell::Cell<u64>,
    /// Triple-term IDs built for statements (statement → transient ID).
    tt_cache: RefCell<HashMap<Id, Id>>,
}

fn now_lexical() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = literal::civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

impl<'a> Exec<'a> {
    pub fn new(kv: &'a dyn KvRead, dict: Dict) -> Exec<'a> {
        Exec {
            kv,
            dict,
            budget: Budget::default(),
            transient: RefCell::new((Vec::new(), HashMap::new())),
            fctx: RefCell::new(FnContext::new(&now_lexical())),
            scopes: RefCell::new(HashMap::new()),
            rels: RefCell::new(HashMap::new()),
            stats: None,
            wanted: std::cell::Cell::new([0; 3]),
            standard: false,
            probes: std::cell::Cell::new(0),
            tt_cache: RefCell::new(HashMap::new()),
        }
    }

    fn check_budget(&self, rows: usize, op: &str) -> Result<()> {
        if rows > self.budget.max_rows {
            return Err(Error::Invalid(format!(
                "query exceeded its row budget of {} in {op}",
                self.budget.max_rows
            )));
        }
        if self.budget.deadline.is_some_and(|d| Instant::now() > d) {
            return Err(Error::Invalid(format!(
                "query exceeded its time budget in {op}"
            )));
        }
        Ok(())
    }

    // ---- terms ----------------------------------------------------------------

    /// The term of an ID (store dictionary or transient table). In standard
    /// mode a statement reads as its alias IRI, or else as a blank node: the
    /// RDF 1.2 form of a reifier
    /// ([[22-implementation-decisions#Triple terms map to statement IDs]]).
    pub fn decode(&self, id: Id) -> Result<Term> {
        if id.raw() & TRANSIENT == TRANSIENT && id.kind() == Kind::System {
            let t = self.transient.borrow();
            return Ok(t.0[(id.raw() & !TRANSIENT) as usize].clone());
        }
        if self.standard && id.kind() == Kind::Statement {
            if let Some(e) =
                crate::storage::prefix_iter(self.kv, &crate::tables::ALIAS_REV, &id.be()).next()
            {
                let alias = Id::from_be(&e?.0[8..]);
                return self.dict.decode(self.kv, alias);
            }
            return Ok(Term::Blank(format!("{STMT_BLANK}{}", id.raw())));
        }
        self.dict.decode(self.kv, id)
    }

    // ---- triple terms ----------------------------------------------------------

    /// The triple term a statement names, read from its stored triple. A
    /// statement ID in its object position is a nested triple term.
    pub fn triple_term_of(&self, stmt: Id) -> Result<Option<Term>> {
        if stmt.kind() != Kind::Statement {
            return Ok(None);
        }
        let Some((t, _)) = store::stmt_get(self.kv, stmt)? else {
            return Ok(None);
        };
        let o = match self.triple_term_of(t.o)? {
            Some(x) => x,
            None => self.decode(t.o)?,
        };
        Ok(Some(Term::triple(self.decode(t.s)?, self.decode(t.p)?, o)))
    }

    /// The transient ID of the triple term of a statement.
    pub fn triple_id_of(&self, stmt: Id) -> Result<Option<Id>> {
        if let Some(id) = self.tt_cache.borrow().get(&stmt) {
            return Ok(Some(*id));
        }
        let Some(t) = self.triple_term_of(stmt)? else {
            return Ok(None);
        };
        let id = self.transient_id(&t);
        self.tt_cache.borrow_mut().insert(stmt, id);
        Ok(Some(id))
    }

    /// The stored statement (asserted or unasserted) a triple term names.
    pub fn triple_stmt(&self, t: &TripleTerm) -> Result<Option<Id>> {
        let (Some(s), Some(p), Some(o)) = (
            self.stored_id(&t.subject)?,
            self.stored_id(&t.predicate)?,
            self.stored_id(&t.object)?,
        ) else {
            return Ok(None);
        };
        store::stmt_lookup(self.kv, s, p, o)
    }

    /// The stored ID of a term, if it has one.
    fn stored_id(&self, t: &Term) -> Result<Option<Id>> {
        match t {
            Term::Triple(x) => self.triple_stmt(x),
            Term::Blank(l) => {
                if let Some(id) = stmt_blank(l) {
                    return Ok(Some(id));
                }
                self.dict.lookup(self.kv, t)
            }
            _ => {
                let id = self.encode(t)?;
                Ok((!self.is_transient(id)).then_some(id))
            }
        }
    }

    /// The triple term behind a transient ID, if it is one.
    fn triple_of_id(&self, id: Id) -> Option<TripleTerm> {
        if !self.is_transient(id) {
            return None;
        }
        match &self.transient.borrow().0[(id.raw() & !TRANSIENT) as usize] {
            Term::Triple(t) => Some((**t).clone()),
            _ => None,
        }
    }

    /// The statement an ID stands for: itself, or the stored statement of a
    /// triple-term ID.
    fn stmt_of(&self, id: Id) -> Result<Option<Id>> {
        if id.kind() == Kind::Statement {
            return Ok(Some(id));
        }
        match self.triple_of_id(id) {
            Some(t) => self.triple_stmt(&t),
            None => Ok(None),
        }
    }

    /// A value ready to probe the store with: a triple-term ID becomes its
    /// statement ID; `None` when it names no stored statement.
    fn probe_id(&self, id: Id) -> Result<Option<Id>> {
        if self.triple_of_id(id).is_some() {
            return self.stmt_of(id);
        }
        Ok(Some(id))
    }

    /// A reifier term compared with a triple term is read as the triple term
    /// it names (rule `sparql-triple-term-lazy`).
    fn lift(&self, a: Term, other: &Term) -> Term {
        if !matches!(other, Term::Triple(_)) {
            return a;
        }
        let stmt = match &a {
            Term::Iri(i) => self.special_iri(i).ok().flatten(),
            Term::Blank(l) => stmt_blank(l),
            _ => None,
        };
        match stmt.map(|x| self.triple_term_of(x)) {
            Some(Ok(Some(t))) => t,
            _ => a,
        }
    }

    fn lift_pair(&self, a: Term, b: Term) -> (Term, Term) {
        let a = self.lift(a, &b);
        let b = self.lift(b, &a);
        (a, b)
    }

    /// The ID of `rdf:reifies` (transient when the dictionary lacks it).
    fn reifies_id(&self) -> Result<Id> {
        self.encode(&Term::Iri(RDF_REIFIES.into()))
    }

    /// Standard mode: the virtual `(r, rdf:reifies, <<( s p o )>>)` triples
    /// matching a pattern, each with the graphs that hold one of the
    /// annotations of `r`. A statement without annotations reifies nothing.
    fn virtual_reifies(
        &self,
        s: Option<Id>,
        p: Option<Id>,
        o: Option<Id>,
        stored: &[Match],
    ) -> Result<Vec<(Match, Vec<Id>)>> {
        let reifies = self.reifies_id()?;
        if !self.standard || p.is_some_and(|p| p != reifies) {
            return Ok(Vec::new());
        }
        let mut cands: Vec<Id> = match (s, o) {
            (Some(s), _) => vec![s],
            (None, Some(o)) => vec![o],
            (None, None) if p.is_none() => stored.iter().map(|m| m.s).collect(),
            (None, None) => store::find_stored(self.kv, None, None, None)?
                .into_iter()
                .map(|m| m.s)
                .collect(),
        };
        cands.retain(|c| c.kind() == Kind::Statement && o.is_none_or(|o| o == *c));
        let cands: BTreeSet<Id> = cands.into_iter().collect();
        let mut out = Vec::new();
        for r in cands {
            let mut graphs = Vec::new();
            let mut any = false;
            for m in store::find_stored(self.kv, Some(r), None, None)? {
                any = true;
                for g in graph::match_graphs(self.kv, &m)? {
                    if !graphs.contains(&g) {
                        graphs.push(g);
                    }
                }
            }
            if !any {
                continue;
            }
            let Some(tt) = self.triple_id_of(r)? else {
                continue;
            };
            out.push((
                Match {
                    s: r,
                    p: reifies,
                    o: tt,
                    id: None,
                },
                graphs,
            ));
        }
        Ok(out)
    }

    /// The ID of a term: inline, dictionary, a reifier IRI, or a transient ID.
    /// Never writes.
    pub fn encode(&self, t: &Term) -> Result<Id> {
        // A triple term is a computed value; scans and joins map it to its
        // statement where one is stored.
        if let Term::Triple(_) = t {
            return Ok(self.transient_id(t));
        }
        if let Term::Iri(i) = t {
            if let Some(id) = self.special_iri(i)? {
                return Ok(id);
            }
        }
        if let Term::Literal(l) = t {
            if let literal::Canon::Inline(id) = literal::canonicalize(l) {
                return Ok(id);
            }
        }
        if !matches!(t, Term::Blank(_)) {
            if let Some(id) = self.dict.lookup(self.kv, t)? {
                return Ok(id);
            }
        }
        Ok(self.transient_id(t))
    }

    /// The transient terms of this evaluation, for callers that keep IDs
    /// after the evaluation ends.
    pub fn transient_terms(&self) -> HashMap<Id, Term> {
        self.transient
            .borrow()
            .1
            .iter()
            .map(|(t, id)| (*id, t.clone()))
            .collect()
    }

    /// Number of transient terms created so far (a plan that needed one is
    /// not cacheable).
    pub fn transient_count(&self) -> usize {
        self.transient.borrow().0.len()
    }

    fn is_transient(&self, id: Id) -> bool {
        id.raw() & TRANSIENT == TRANSIENT && id.kind() == Kind::System
    }

    /// Base IRI for `IRI()` / `URI()` (the query's `BASE` or the caller's).
    pub fn set_base_iri(&self, base: Option<String>) {
        self.fctx.borrow_mut().base_iri = base;
    }

    fn transient_id(&self, t: &Term) -> Id {
        let mut tt = self.transient.borrow_mut();
        if let Some(id) = tt.1.get(t) {
            return *id;
        }
        let id = Id(TRANSIENT | tt.0.len() as u64);
        tt.0.push(t.clone());
        tt.1.insert(t.clone(), id);
        id
    }

    /// Reifier IRIs, aliases and graph names map straight to their IDs
    /// (rule `sparql-reifier-iri`).
    pub fn special_iri(&self, iri: &str) -> Result<Option<Id>> {
        if let Some(n) = iri
            .strip_prefix(crate::term::STMT_IRI_PREFIX)
            .and_then(|n| n.parse::<u64>().ok())
        {
            let id = Id(n);
            if matches!(id.kind(), Kind::Statement | Kind::Hyperedge) {
                return Ok(Some(id));
            }
        }
        if let Some(iri_id) = self.dict.lookup(self.kv, &Term::Iri(iri.into()))? {
            if let Some(s) = store::alias_target(self.kv, iri_id)? {
                return Ok(Some(s));
            }
        }
        Ok(None)
    }

    /// The ID a graph name resolves to (`GRAPH :g`).
    pub fn graph_id(&self, name: &Term) -> Result<Id> {
        match graph::graph_named(self.kv, &self.dict, name)? {
            Some(g) => Ok(g),
            None => Ok(self.transient_id(name)),
        }
    }

    /// Owned literals are equal by content (rule `plan-owned-equality`): a
    /// canonical representative per content.
    /// A graph ID stands for its name IRI, so `?s :p ?g . GRAPH ?g { … }`
    /// joins the name and the graph.
    fn canon(&self, id: Id) -> Result<Id> {
        if self.triple_of_id(id).is_some() {
            return Ok(self.stmt_of(id)?.unwrap_or(id));
        }
        if id.is_owned_literal() {
            let t = self.decode(id)?;
            return Ok(self.transient_id(&t));
        }
        if id.kind() == Kind::Graph {
            let t = self.decode(id)?;
            return self.encode(&t);
        }
        Ok(id)
    }

    fn same(&self, a: Id, b: Id) -> Result<bool> {
        if a == b {
            return Ok(true);
        }
        if a.is_owned_literal()
            || b.is_owned_literal()
            || a.kind() == Kind::Graph
            || b.kind() == Kind::Graph
        {
            return Ok(self.canon(a)? == self.canon(b)?);
        }
        // A triple term and the statement that stores it are one value.
        if self.triple_of_id(a).is_some() || self.triple_of_id(b).is_some() {
            return Ok(match (self.stmt_of(a)?, self.stmt_of(b)?) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            });
        }
        Ok(false)
    }

    /// Of two equal bindings, the one to keep: a triple-term value wins over
    /// the statement ID that stores it, so a variable bound to a triple term
    /// projects as one whatever the join order.
    fn keep(&self, cur: Id, new: Id) -> Id {
        if cur.kind() == Kind::Statement && self.triple_of_id(new).is_some() {
            new
        } else {
            cur
        }
    }

    /// The graph a bound graph variable names: a graph ID, or the graph
    /// whose name is the bound IRI.
    fn as_graph(&self, id: Id) -> Result<Id> {
        if id.kind() == Kind::Graph || id.is_null() {
            return Ok(id);
        }
        let t = self.decode(id)?;
        self.graph_id(&t)
    }

    // ---- scopes -----------------------------------------------------------------

    fn scope_set(&self, g: Id, direct: bool) -> Result<std::rc::Rc<HashSet<Id>>> {
        if let Some(s) = self.scopes.borrow().get(&(g, direct)) {
            return Ok(s.clone());
        }
        let set: HashSet<Id> = graph::scope_graphs(self.kv, g, direct)?
            .into_iter()
            .collect();
        let rc = std::rc::Rc::new(set);
        self.scopes.borrow_mut().insert((g, direct), rc.clone());
        Ok(rc)
    }

    /// Matches of a pattern with the given bound positions, with the graph
    /// each one is bound to when the graph is a variable.
    fn find_scoped(
        &self,
        s: Option<Id>,
        p: Option<Id>,
        o: Option<Id>,
        graph: GraphSpec,
        g_bound: Option<Id>,
    ) -> Result<Vec<(Match, Id)>> {
        self.probes.set(self.probes.get() + 1);
        // The object of a virtual `rdf:reifies` is a triple term, never a
        // reifier: only a triple-term value matches it.
        let o_reifier = o.is_some_and(|x| self.triple_of_id(x).is_none());
        // Triple-term values probe with their statement; one without a
        // stored statement matches nothing.
        let (s, p, o) = match (
            s.map(|x| self.probe_id(x)).transpose()?,
            p.map(|x| self.probe_id(x)).transpose()?,
            o.map(|x| self.probe_id(x)).transpose()?,
        ) {
            (Some(None), _, _) | (_, Some(None), _) | (_, _, Some(None)) => return Ok(Vec::new()),
            (s, p, o) => (s.flatten(), p.flatten(), o.flatten()),
        };
        // A transient constant never matches stored data.
        let mut all = if [s, p, o]
            .iter()
            .flatten()
            .any(|i| i.raw() & TRANSIENT == TRANSIENT && i.kind() == Kind::System)
        {
            Vec::new()
        } else {
            store::find(self.kv, s, p, o)?
        };
        if self.standard {
            if let Some(in_id) = graph::sys_id(self.kv, crate::tables::mk::SYS_IN)? {
                all.retain(|m| {
                    !(m.p == in_id
                        && matches!(m.s.kind(), Kind::Statement | Kind::Hyperedge | Kind::Graph))
                });
            }
        }
        let virt = if o_reifier {
            Vec::new()
        } else {
            self.virtual_reifies(s, p, o, &all)?
        };
        // Each match with its graphs: computed on demand for stored ones.
        let mut items: Vec<(Match, Option<Vec<Id>>)> = all.into_iter().map(|m| (m, None)).collect();
        items.extend(virt.into_iter().map(|(m, g)| (m, Some(g))));
        let graphs_of = |m: &Match, pre: Option<Vec<Id>>| -> Result<Vec<Id>> {
            match pre {
                Some(g) => Ok(g),
                None => graph::match_graphs(self.kv, m),
            }
        };
        let mut out = Vec::with_capacity(items.len());
        match graph {
            GraphSpec::Union => out.extend(items.into_iter().map(|(m, _)| (m, Id(0)))),
            GraphSpec::DefaultOnly => {
                for (m, pre) in items {
                    if graphs_of(&m, pre)?.contains(&DEFAULT_GRAPH) {
                        out.push((m, Id(0)));
                    }
                }
            }
            GraphSpec::Graph { g, direct } => {
                let fixed = match g {
                    Pt::Const(c) => Some(c),
                    Pt::Var(_) => g_bound.map(|b| self.as_graph(b)).transpose()?,
                };
                match fixed {
                    Some(c) => {
                        let scope = self.scope_set(c, direct)?;
                        for (m, pre) in items {
                            if graphs_of(&m, pre)?.iter().any(|x| scope.contains(x)) {
                                out.push((m, c));
                            }
                        }
                    }
                    None => {
                        for (m, pre) in items {
                            for x in graphs_of(&m, pre)? {
                                if x != DEFAULT_GRAPH && x != SYSTEM_GRAPH {
                                    out.push((m, x));
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    // ---- evaluation ----------------------------------------------------------------

    /// Evaluates `node` once for each input row; every output row extends the
    /// input row whose index it carries.
    pub fn eval(&self, node: &Node, input: &[Row]) -> Result<Vec<(usize, Row)>> {
        let out = self.eval_inner(node, input)?;
        self.check_budget(out.len(), node_name(node))?;
        Ok(out)
    }

    /// Evaluates a node on its own (one empty input row).
    pub fn run(&self, node: &Node, width: usize) -> Result<Vec<Row>> {
        Ok(self
            .eval(node, &[vec![Id(0); width]])?
            .into_iter()
            .map(|(_, r)| r)
            .collect())
    }

    fn eval_inner(&self, node: &Node, input: &[Row]) -> Result<Vec<(usize, Row)>> {
        match node {
            Node::Unit => Ok(input.iter().cloned().enumerate().collect()),
            Node::Empty => Ok(Vec::new()),
            Node::Triple(t) => self.eval_triple(t, input),
            Node::Path { s, path, o, graph } => self.eval_path(*s, path, *o, *graph, input),
            Node::NamedGraphs(slot) => {
                let mut out = Vec::new();
                let graphs: Vec<Id> = graph::all_graphs(self.kv)?
                    .into_iter()
                    .filter(|g| *g != DEFAULT_GRAPH && *g != SYSTEM_GRAPH)
                    .collect();
                for (i, r) in input.iter().enumerate() {
                    for g in &graphs {
                        if r[*slot].is_null() || self.same(r[*slot], *g)? {
                            let mut nr = r.clone();
                            nr[*slot] = *g;
                            out.push((i, nr));
                        }
                    }
                }
                Ok(out)
            }
            Node::PerGraph {
                var,
                active,
                graphs,
                inner,
            } => {
                let graphs: Vec<Id> = match graphs {
                    Some(l) => l.clone(),
                    None => graph::all_graphs(self.kv)?
                        .into_iter()
                        .filter(|g| *g != DEFAULT_GRAPH && *g != SYSTEM_GRAPH)
                        .collect(),
                };
                let mut out = Vec::new();
                for g in graphs {
                    let mut fixed = (**inner).clone();
                    fixed.rebind_graph(*active, Pt::Const(g));
                    let keep: Vec<usize> = (0..input.len())
                        .filter(|i| {
                            input[*i][*var].is_null()
                                || self.same(input[*i][*var], g).unwrap_or(false)
                        })
                        .collect();
                    if keep.is_empty() {
                        continue;
                    }
                    let sub: Vec<Row> = keep.iter().map(|i| input[*i].clone()).collect();
                    for (k, mut r) in self.eval(&fixed, &sub)? {
                        if r[*var].is_null() {
                            r[*var] = g;
                        } else if !self.same(r[*var], g)? {
                            continue;
                        }
                        out.push((keep[k], r));
                    }
                }
                Ok(out)
            }
            Node::Join(parts) => self.eval_join(parts, input),
            Node::LeftJoin(a, b, cond) => {
                let left = self.eval(a, input)?;
                let rows: Vec<Row> = left.iter().map(|(_, r)| r.clone()).collect();
                let right = self.join_into(b, &rows)?;
                let mut by_origin: Vec<Vec<Row>> = vec![Vec::new(); rows.len()];
                for (i, r) in right {
                    if match cond {
                        Some(c) => self.ebv(c, &r)?,
                        None => true,
                    } {
                        by_origin[i].push(r);
                    }
                }
                let mut out = Vec::new();
                for ((orig, l), ext) in left.into_iter().zip(by_origin) {
                    if ext.is_empty() {
                        out.push((orig, l));
                    } else {
                        out.extend(ext.into_iter().map(|r| (orig, r)));
                    }
                }
                Ok(out)
            }
            Node::Minus(a, b) => {
                let left = self.eval(a, input)?;
                let width = input.first().map_or(0, Vec::len);
                let right = self.run(b, width)?;
                let mut out = Vec::new();
                'l: for (i, l) in left {
                    for r in &right {
                        let mut shared = false;
                        let mut compatible = true;
                        for k in 0..l.len() {
                            if !l[k].is_null() && !r[k].is_null() {
                                shared = true;
                                if !self.same(l[k], r[k])? {
                                    compatible = false;
                                    break;
                                }
                            }
                        }
                        if shared && compatible {
                            continue 'l;
                        }
                    }
                    out.push((i, l));
                }
                Ok(out)
            }
            Node::AntiJoin(a, b) => {
                let left = self.eval(a, input)?;
                let rows: Vec<Row> = left.iter().map(|(_, r)| r.clone()).collect();
                let hits: HashSet<usize> = self
                    .join_into(b, &rows)?
                    .into_iter()
                    .map(|(i, _)| i)
                    .collect();
                Ok(left
                    .into_iter()
                    .enumerate()
                    .filter(|(k, _)| !hits.contains(k))
                    .map(|(_, x)| x)
                    .collect())
            }
            Node::Union(parts) => {
                let mut out = Vec::new();
                for p in parts {
                    out.extend(self.eval(p, input)?);
                }
                Ok(out)
            }
            Node::Filter(e, inner) => {
                let rows = self.eval(inner, input)?;
                let mut out = Vec::with_capacity(rows.len());
                for (i, r) in rows {
                    self.fctx.borrow_mut().next_row();
                    if self.ebv(e, &r)? {
                        out.push((i, r));
                    }
                }
                Ok(out)
            }
            Node::Extend(inner, slot, e) => {
                let rows = self.eval(inner, input)?;
                let mut out = Vec::with_capacity(rows.len());
                for (i, mut r) in rows {
                    // The solution without values computed by expressions
                    // (transient IDs), so chained BINDs share BNODE(str).
                    let key = r
                        .iter()
                        .map(|v| if self.is_transient(*v) { 0 } else { v.raw() })
                        .collect();
                    self.fctx.borrow_mut().set_row(key);
                    if let Ok(t) = self.expr(e, &r) {
                        let id = self.encode(&t)?;
                        if !r[*slot].is_null() {
                            if !self.same(r[*slot], id)? {
                                continue;
                            }
                            r[*slot] = self.keep(r[*slot], id);
                        } else {
                            r[*slot] = id;
                        }
                    }
                    out.push((i, r));
                }
                Ok(out)
            }
            Node::Values(slots, rows) => {
                let mut out = Vec::new();
                for (i, r) in input.iter().enumerate() {
                    'v: for vr in rows {
                        let mut nr = r.clone();
                        for (s, v) in slots.iter().zip(vr) {
                            if let Some(v) = v {
                                if nr[*s].is_null() {
                                    nr[*s] = *v;
                                } else if self.same(nr[*s], *v)? {
                                    nr[*s] = self.keep(nr[*s], *v);
                                } else {
                                    continue 'v;
                                }
                            }
                        }
                        out.push((i, nr));
                    }
                }
                Ok(out)
            }
            Node::Aggregate {
                input: inner,
                keys,
                aggs,
            } => self.per_input(input, |row| self.eval_aggregate(inner, keys, aggs, row)),
            Node::Distinct(inner) => {
                let rows = self.eval(inner, input)?;
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                for (i, r) in rows {
                    let mut k = Vec::with_capacity(r.len() + 1);
                    k.push(Id(i as u64));
                    for v in &r {
                        k.push(self.canon(*v)?);
                    }
                    if seen.insert(k) {
                        out.push((i, r));
                    }
                }
                Ok(out)
            }
            Node::OrderBy(inner, keys) => {
                let rows = self.eval(inner, input)?;
                let mut keyed = Vec::with_capacity(rows.len());
                for (i, r) in rows {
                    let ks: Vec<Option<Term>> =
                        keys.iter().map(|(e, _)| self.expr(e, &r).ok()).collect();
                    keyed.push((ks, i, r));
                }
                keyed.sort_by(|a, b| {
                    for (k, (_, desc)) in keys.iter().enumerate() {
                        let c = func::order_cmp(a.0[k].as_ref(), b.0[k].as_ref());
                        let c = if *desc { c.reverse() } else { c };
                        if c != Ordering::Equal {
                            return c;
                        }
                    }
                    Ordering::Equal
                });
                Ok(keyed.into_iter().map(|(_, i, r)| (i, r)).collect())
            }
            Node::Slice(inner, start, len) => {
                let rows = self.eval(inner, input)?;
                let mut count: HashMap<usize, usize> = HashMap::new();
                let mut out = Vec::new();
                for (i, r) in rows {
                    let c = count.entry(i).or_default();
                    let k = *c;
                    *c += 1;
                    if k >= *start && len.is_none_or(|l| k < start + l) {
                        out.push((i, r));
                    }
                }
                Ok(out)
            }
            Node::Project(inner, keep) => {
                // A projection is evaluated on its own: outer bindings do not
                // reach inside a subquery.
                self.per_input(input, |row| {
                    let width = row.len();
                    let inner_rows = self.run(inner, width)?;
                    let mut out = Vec::new();
                    'r: for r in inner_rows {
                        let mut nr = row.clone();
                        for s in keep {
                            if r[*s].is_null() {
                                continue;
                            }
                            if nr[*s].is_null() {
                                nr[*s] = r[*s];
                            } else if !self.same(nr[*s], r[*s])? {
                                continue 'r;
                            }
                        }
                        out.push(nr);
                    }
                    Ok(out)
                })
            }
            Node::Rel {
                name,
                args,
                version,
            } => self.eval_rel(name, args, *version, input),
            Node::Datalog {
                program,
                goal,
                args,
            } => crate::datalog::eval::eval_goal(self, program, goal, args, input),
        }
    }

    /// Runs `f` once per distinct input row (operators that do not take
    /// outer bindings into account).
    fn per_input(
        &self,
        input: &[Row],
        f: impl Fn(&Row) -> Result<Vec<Row>>,
    ) -> Result<Vec<(usize, Row)>> {
        let mut out = Vec::new();
        let mut memo: HashMap<&Row, Vec<Row>> = HashMap::new();
        for (i, r) in input.iter().enumerate() {
            if !memo.contains_key(r) {
                memo.insert(r, f(r)?);
            }
            out.extend(memo[r].iter().cloned().map(|x| (i, x)));
        }
        Ok(out)
    }

    /// Evaluates `node` for each row of `rows` as bindings when that cannot
    /// change its meaning, else on its own followed by a compatible join.
    pub fn join_into(&self, node: &Node, rows: &[Row]) -> Result<Vec<(usize, Row)>> {
        if node.sideways_safe() {
            let mut out = Vec::new();
            for (k, chunk) in rows.chunks(BATCH).enumerate() {
                for (i, r) in self.eval(node, chunk)? {
                    out.push((k * BATCH + i, r));
                }
            }
            return Ok(out);
        }
        let width = rows.first().map_or(0, Vec::len);
        let right = self.run(node, width)?;
        self.hash_join(rows, &right)
    }

    /// Compatible-mapping join of `left` rows with `right` rows (hash join on
    /// the slots bound in every right row, nested loop on the rest).
    fn hash_join(&self, left: &[Row], right: &[Row]) -> Result<Vec<(usize, Row)>> {
        let width = left.first().map_or(0, Vec::len);
        let always: Vec<usize> = (0..width)
            .filter(|k| !right.is_empty() && right.iter().all(|r| !r[*k].is_null()))
            .collect();
        let mut table: HashMap<Vec<Id>, Vec<&Row>> = HashMap::new();
        for r in right {
            let k = always
                .iter()
                .map(|s| self.canon(r[*s]))
                .collect::<Result<Vec<_>>>()?;
            table.entry(k).or_default().push(r);
        }
        let mut out = Vec::new();
        for (i, l) in left.iter().enumerate() {
            let candidates: Vec<&Row> = if always.iter().all(|s| !l[*s].is_null()) {
                let k = always
                    .iter()
                    .map(|s| self.canon(l[*s]))
                    .collect::<Result<Vec<_>>>()?;
                table.get(&k).cloned().unwrap_or_default()
            } else {
                right.iter().collect()
            };
            for r in candidates {
                if let Some(m) = self.merge(l, r)? {
                    out.push((i, m));
                }
            }
        }
        Ok(out)
    }

    fn merge(&self, a: &Row, b: &Row) -> Result<Option<Row>> {
        let mut m = a.clone();
        for k in 0..a.len() {
            if b[k].is_null() {
                continue;
            }
            if m[k].is_null() {
                m[k] = b[k];
            } else if self.same(m[k], b[k])? {
                m[k] = self.keep(m[k], b[k]);
            } else {
                return Ok(None);
            }
        }
        Ok(Some(m))
    }

    /// n-ary join: sideways-safe parts are ordered by estimated selectivity
    /// and run as index nested-loop joins; the others are evaluated on their
    /// own and hash-joined; a cyclic pattern of triples goes to Leapfrog
    /// Triejoin (rule `plan-join-choice`).
    fn eval_join(&self, parts: &[Node], input: &[Row]) -> Result<Vec<(usize, Row)>> {
        if parts.is_empty() {
            return Ok(input.iter().cloned().enumerate().collect());
        }
        if let Some(out) = crate::query::lftj::try_join(self, parts, input)? {
            return Ok(out);
        }
        let order = crate::query::plan::order_join(self, parts, input);
        let mut cur: Vec<(usize, Row)> = input.iter().cloned().enumerate().collect();
        for idx in order {
            let rows: Vec<Row> = cur.iter().map(|(_, r)| r.clone()).collect();
            let next = self.join_into(&parts[idx], &rows)?;
            cur = next.into_iter().map(|(k, r)| (cur[k].0, r)).collect();
            self.check_budget(cur.len(), "Join")?;
            if cur.is_empty() {
                break;
            }
        }
        Ok(cur)
    }

    fn eval_triple(&self, t: &TriplePat, input: &[Row]) -> Result<Vec<(usize, Row)>> {
        let val = |p: &Pt, r: &Row| match p {
            Pt::Const(c) => Some(*c),
            Pt::Var(v) if !r[*v].is_null() => Some(r[*v]),
            Pt::Var(_) => None,
        };
        let gvar = match t.graph {
            GraphSpec::Graph { g: Pt::Var(v), .. } => Some(v),
            _ => None,
        };
        // Reifier bound: one `stmt` lookup (relation `reifies`, r bound).
        if let Some(id_pt) = &t.id {
            let mut out = Vec::new();
            let mut needs_scan = Vec::new();
            for (i, r) in input.iter().enumerate() {
                match val(id_pt, r) {
                    Some(rid) => {
                        self.probes.set(self.probes.get() + 1);
                        let Some(rid) = self.stmt_of(rid)? else {
                            continue;
                        };
                        let Some(tr) = self.reified(rid)? else {
                            continue;
                        };
                        let m = Match {
                            s: tr.s,
                            p: tr.p,
                            o: tr.o,
                            id: Some(rid),
                        };
                        let graphs =
                            self.graphs_for(&m, t.graph, gvar.and_then(|g| val(&Pt::Var(g), r)))?;
                        for g in graphs {
                            if let Some(nr) = self.bind_match(t, r, &m, g, gvar)? {
                                out.push((i, nr));
                            }
                        }
                    }
                    None => needs_scan.push(i),
                }
            }
            if needs_scan.len() < input.len() {
                let sub: Vec<Row> = needs_scan.iter().map(|i| input[*i].clone()).collect();
                for (k, r) in self.scan_triple(t, &sub, gvar)? {
                    out.push((needs_scan[k], r));
                }
                return Ok(out);
            }
        }
        self.scan_triple(t, input, gvar)
    }

    fn scan_triple(
        &self,
        t: &TriplePat,
        input: &[Row],
        gvar: Option<Slot>,
    ) -> Result<Vec<(usize, Row)>> {
        let val = |p: &Pt, r: &Row| match p {
            Pt::Const(c) => Some(*c),
            Pt::Var(v) if !r[*v].is_null() => Some(r[*v]),
            Pt::Var(_) => None,
        };
        // Distinct probe keys of the batch, in key order (rule `plan-inlj`).
        let mut keys: BTreeSet<ProbeKey> = BTreeSet::new();
        let key_of = |r: &Row| {
            (
                val(&t.s, r),
                val(&t.p, r),
                val(&t.o, r),
                gvar.and_then(|g| val(&Pt::Var(g), r)),
            )
        };
        for r in input {
            keys.insert(key_of(r));
        }
        let mut results: HashMap<ProbeKey, Vec<(Match, Id)>> = HashMap::new();
        for k in keys {
            let (s, p, o, g) = k;
            // literal subjects and non-IRI predicates match nothing
            if s.is_some_and(|s| s.is_literal()) || p.is_some_and(|p| p.is_literal()) {
                results.insert(k, Vec::new());
                continue;
            }
            let mut found = self.find_scoped(s, p, o, t.graph, g)?;
            // A triple-term or reifier pattern also names triples that are
            // not asserted ([[adr:0032]]).
            if t.id.is_some() && t.graph == GraphSpec::Union {
                let probe = |x: Option<Id>| -> Result<Option<Option<Id>>> {
                    Ok(match x {
                        None => Some(None),
                        Some(v) => self.probe_id(v)?.map(Some),
                    })
                };
                if let (Some(s), Some(p), Some(o)) = (probe(s)?, probe(p)?, probe(o)?) {
                    found.extend(
                        store::find_unasserted(self.kv, s, p, o)?
                            .into_iter()
                            .map(|m| (m, Id(0))),
                    );
                }
            }
            results.insert(k, found);
        }
        let mut out = Vec::new();
        for (i, r) in input.iter().enumerate() {
            for (m, g) in &results[&key_of(r)] {
                if let Some(nr) = self.bind_match(t, r, m, *g, gvar)? {
                    out.push((i, nr));
                }
            }
        }
        Ok(out)
    }

    fn graphs_for(&self, m: &Match, spec: GraphSpec, gbound: Option<Id>) -> Result<Vec<Id>> {
        Ok(match spec {
            GraphSpec::Union => vec![Id(0)],
            GraphSpec::DefaultOnly => {
                if graph::match_graphs(self.kv, m)?.contains(&DEFAULT_GRAPH) {
                    vec![Id(0)]
                } else {
                    vec![]
                }
            }
            GraphSpec::Graph { g, direct } => {
                let fixed = match g {
                    Pt::Const(c) => Some(c),
                    Pt::Var(_) => gbound.map(|b| self.as_graph(b)).transpose()?,
                };
                match fixed {
                    Some(c) => {
                        let scope = self.scope_set(c, direct)?;
                        if graph::match_graphs(self.kv, m)?
                            .iter()
                            .any(|x| scope.contains(x))
                        {
                            vec![c]
                        } else {
                            vec![]
                        }
                    }
                    None => graph::match_graphs(self.kv, m)?
                        .into_iter()
                        .filter(|x| *x != DEFAULT_GRAPH && *x != SYSTEM_GRAPH)
                        .collect(),
                }
            }
        })
    }

    /// The triple a reifier names: its statement, a promoted role or
    /// membership, or an unasserted statement.
    fn reified(&self, rid: Id) -> Result<Option<crate::term::Triple>> {
        if rid.kind() != Kind::Statement {
            return Ok(None);
        }
        Ok(store::stmt_get(self.kv, rid)?.map(|(t, _)| t))
    }

    /// Extends row `r` with match `m` (and graph `g`), checking repeated
    /// variables and bound positions.
    fn bind_match(
        &self,
        t: &TriplePat,
        r: &Row,
        m: &Match,
        g: Id,
        gvar: Option<Slot>,
    ) -> Result<Option<Row>> {
        let mut nr = r.clone();
        let put = |p: &Pt, v: Id, nr: &mut Row| -> Result<bool> {
            match p {
                Pt::Const(c) => self.same(*c, v),
                Pt::Var(s) => {
                    if nr[*s].is_null() {
                        nr[*s] = v;
                        Ok(true)
                    } else {
                        self.same(nr[*s], v)
                    }
                }
            }
        };
        // Standard mode: a statement in object position is a triple term.
        let o = match t.o {
            Pt::Var(v) if self.standard && nr[v].is_null() && m.o.kind() == Kind::Statement => {
                self.triple_id_of(m.o)?.unwrap_or(m.o)
            }
            _ => m.o,
        };
        if !put(&t.s, m.s, &mut nr)? || !put(&t.p, m.p, &mut nr)? || !put(&t.o, o, &mut nr)? {
            return Ok(None);
        }
        if let Some(idp) = &t.id {
            let Some(id) = m.id else {
                return Ok(None);
            };
            if !put(idp, id, &mut nr)? {
                return Ok(None);
            }
        }
        if let Some(gv) = gvar {
            if !put(&Pt::Var(gv), g, &mut nr)? {
                return Ok(None);
            }
        }
        Ok(Some(nr))
    }

    // ---- property paths (rule `plan-path-bfs`) -----------------------------------

    fn step(&self, from: Id, pred: &PathExpr, graph: GraphSpec, fwd: bool) -> Result<Vec<Id>> {
        let mut out = Vec::new();
        match pred {
            PathExpr::Pred(p) => {
                let ms = if fwd {
                    self.find_scoped(Some(from), Some(*p), None, graph, None)?
                } else {
                    self.find_scoped(None, Some(*p), Some(from), graph, None)?
                };
                out.extend(ms.into_iter().map(|(m, _)| if fwd { m.o } else { m.s }));
            }
            PathExpr::Neg { fwd: fs, inv } => {
                if fwd {
                    for (m, _) in self.find_scoped(Some(from), None, None, graph, None)? {
                        if !fs.contains(&m.p) && (!fs.is_empty() || inv.is_empty()) {
                            out.push(m.o);
                        }
                    }
                    for (m, _) in self.find_scoped(None, None, Some(from), graph, None)? {
                        if !inv.is_empty() && !inv.contains(&m.p) {
                            out.push(m.s);
                        }
                    }
                } else {
                    for (m, _) in self.find_scoped(None, None, Some(from), graph, None)? {
                        if !fs.contains(&m.p) && (!fs.is_empty() || inv.is_empty()) {
                            out.push(m.s);
                        }
                    }
                    for (m, _) in self.find_scoped(Some(from), None, None, graph, None)? {
                        if !inv.is_empty() && !inv.contains(&m.p) {
                            out.push(m.o);
                        }
                    }
                }
            }
            _ => out.extend(self.path_from(from, pred, graph, fwd)?),
        }
        Ok(out)
    }

    /// Every node reachable from `from` along `path` (forward) or against it.
    fn path_from(&self, from: Id, path: &PathExpr, graph: GraphSpec, fwd: bool) -> Result<Vec<Id>> {
        Ok(match path {
            PathExpr::Pred(_) | PathExpr::Neg { .. } => self.step(from, path, graph, fwd)?,
            PathExpr::Inv(p) => self.path_from(from, p, graph, !fwd)?,
            PathExpr::Seq(a, b) => {
                let (first, second) = if fwd { (a, b) } else { (b, a) };
                let mut out = Vec::new();
                for x in self.path_from(from, first, graph, fwd)? {
                    out.extend(self.path_from(x, second, graph, fwd)?);
                }
                out
            }
            PathExpr::Alt(a, b) => {
                let mut out = self.path_from(from, a, graph, fwd)?;
                out.extend(self.path_from(from, b, graph, fwd)?);
                out
            }
            PathExpr::Opt(p) => {
                let mut out = vec![from];
                for x in self.path_from(from, p, graph, fwd)? {
                    if x != from {
                        out.push(x);
                    }
                }
                dedup_keep_order(out)
            }
            PathExpr::Star(p) | PathExpr::Plus(p) => {
                // Breadth-first search with a visited set; each level is one
                // expansion of the frontier.
                let star = matches!(path, PathExpr::Star(_));
                let mut seen: HashSet<Id> = HashSet::new();
                let mut out = Vec::new();
                if star {
                    seen.insert(from);
                    out.push(from);
                }
                let mut frontier: VecDeque<Id> = VecDeque::from([from]);
                while let Some(x) = frontier.pop_front() {
                    for y in self.path_from(x, p, graph, fwd)? {
                        if seen.insert(y) {
                            out.push(y);
                            frontier.push_back(y);
                        }
                    }
                    self.check_budget(seen.len(), "Path")?;
                }
                out
            }
        })
    }

    /// Bidirectional breadth-first search: whether `b` is reachable from `a`
    /// along `path` (`*` or `+`).
    fn meet(&self, a: Id, b: Id, path: &PathExpr, graph: GraphSpec) -> Result<bool> {
        let (PathExpr::Star(step) | PathExpr::Plus(step)) = path else {
            unreachable!("meet is called for * and + paths")
        };
        if matches!(path, PathExpr::Star(_)) && a == b {
            return Ok(true);
        }
        let mut seen_f: HashSet<Id> = HashSet::from([a]);
        let mut seen_b: HashSet<Id> = HashSet::from([b]);
        let mut front_f = vec![a];
        let mut front_b = vec![b];
        // `+` needs at least one step: a node met at depth 0 on both sides
        // only counts once a step has been taken.
        let mut steps = 0usize;
        while !front_f.is_empty() && !front_b.is_empty() {
            let forward = front_f.len() <= front_b.len();
            let (front, seen, other, fwd) = if forward {
                (&mut front_f, &mut seen_f, &seen_b, true)
            } else {
                (&mut front_b, &mut seen_b, &seen_f, false)
            };
            let mut next = Vec::new();
            for x in front.iter() {
                for y in self.path_from(*x, step, graph, fwd)? {
                    if other.contains(&y) {
                        return Ok(true);
                    }
                    if seen.insert(y) {
                        next.push(y);
                    }
                }
            }
            *front = next;
            steps += 1;
            self.check_budget(seen_f.len() + seen_b.len(), "Path")?;
        }
        let _ = steps;
        Ok(false)
    }

    /// Path results keep duplicates for non-recursive paths (SPARQL counts
    /// each path), sets for `*`, `+` and `?`.
    fn eval_path(
        &self,
        s: Pt,
        path: &PathExpr,
        o: Pt,
        graph: GraphSpec,
        input: &[Row],
    ) -> Result<Vec<(usize, Row)>> {
        let val = |p: &Pt, r: &Row| match p {
            Pt::Const(c) => Some(*c),
            Pt::Var(v) if !r[*v].is_null() => Some(r[*v]),
            Pt::Var(_) => None,
        };
        // `GRAPH ?g { path }`: the path is evaluated inside one graph at a
        // time and binds ?g to it.
        if let GraphSpec::Graph {
            g: Pt::Var(gv),
            direct,
        } = graph
        {
            let named: Vec<Id> = graph::all_graphs(self.kv)?
                .into_iter()
                .filter(|g| *g != DEFAULT_GRAPH && *g != SYSTEM_GRAPH)
                .collect();
            let mut out = Vec::new();
            for (i, r) in input.iter().enumerate() {
                let cands = if r[gv].is_null() {
                    named.clone()
                } else {
                    vec![self.as_graph(r[gv])?]
                };
                for g in cands {
                    let spec = GraphSpec::Graph {
                        g: Pt::Const(g),
                        direct,
                    };
                    for (_, mut x) in self.eval_path(s, path, o, spec, std::slice::from_ref(r))? {
                        if x[gv].is_null() {
                            x[gv] = g;
                        } else if !self.same(x[gv], g)? {
                            continue;
                        }
                        out.push((i, x));
                    }
                }
            }
            return Ok(out);
        }
        let zero_len = matches!(path, PathExpr::Star(_) | PathExpr::Opt(_));
        let mut out = Vec::new();
        for (i, r) in input.iter().enumerate() {
            let (sv, ov) = (val(&s, r), val(&o, r));
            let pairs: Vec<(Id, Id)> = match (sv, ov) {
                // Both ends bound on a `*` or `+` path: search from both ends,
                // each level expanding the smaller frontier (rule `plan-path-bfs`).
                (Some(a), Some(b)) if matches!(path, PathExpr::Star(_) | PathExpr::Plus(_)) => {
                    if self.meet(a, b, path, graph)? {
                        vec![(a, b)]
                    } else {
                        Vec::new()
                    }
                }
                (Some(a), _) => self
                    .path_from(a, path, graph, true)?
                    .into_iter()
                    .map(|b| (a, b))
                    .collect(),
                (None, Some(b)) => self
                    .path_from(b, path, graph, false)?
                    .into_iter()
                    .map(|a| (a, b))
                    .collect(),
                (None, None) => {
                    let mut pairs = Vec::new();
                    for a in self.path_nodes(graph, zero_len)? {
                        for b in self.path_from(a, path, graph, true)? {
                            pairs.push((a, b));
                        }
                    }
                    pairs
                }
            };
            for (a, b) in pairs {
                let mut nr = r.clone();
                let mut ok = true;
                for (p, v) in [(&s, a), (&o, b)] {
                    match p {
                        Pt::Const(c) => ok &= self.same(*c, v)?,
                        Pt::Var(x) => {
                            if nr[*x].is_null() {
                                nr[*x] = v;
                            } else {
                                ok &= self.same(nr[*x], v)?;
                            }
                        }
                    }
                }
                if ok {
                    out.push((i, nr));
                }
            }
        }
        Ok(out)
    }

    /// Candidate start nodes for a path with both ends unbound: every subject
    /// and object of the graph (zero-length paths match each node).
    fn path_nodes(&self, graph: GraphSpec, _zero_len: bool) -> Result<Vec<Id>> {
        let mut set = BTreeSet::new();
        for (m, _) in self.find_scoped(None, None, None, graph, None)? {
            set.insert(m.s);
            set.insert(m.o);
        }
        Ok(set.into_iter().collect())
    }

    // ---- Datalog relations --------------------------------------------------------

    fn eval_rel(
        &self,
        name: &str,
        args: &[Pt],
        version: Version,
        input: &[Row],
    ) -> Result<Vec<(usize, Row)>> {
        let rels = self.rels.borrow();
        let Some((full, delta)) = rels.get(name) else {
            return Ok(Vec::new());
        };
        let rows = match version {
            Version::Full => full,
            Version::Delta => delta,
        };
        // Hash index on the argument positions each input row binds, built
        // once per binding pattern: a probe costs O(matches), not O(relation).
        let bound_at = |r: &Row| -> Vec<bool> {
            args.iter()
                .map(|a| match a {
                    Pt::Const(_) => true,
                    Pt::Var(s) => !r[*s].is_null(),
                })
                .collect()
        };
        let mut indexes: HashMap<Vec<bool>, HashMap<Vec<Id>, Vec<usize>>> = HashMap::new();
        let mut out = Vec::new();
        for (i, r) in input.iter().enumerate() {
            let mask = bound_at(r);
            if !indexes.contains_key(&mask) {
                let mut idx: HashMap<Vec<Id>, Vec<usize>> = HashMap::new();
                for (k, tuple) in rows.iter().enumerate() {
                    let key = tuple
                        .iter()
                        .zip(&mask)
                        .filter(|(_, b)| **b)
                        .map(|(v, _)| self.canon(*v))
                        .collect::<Result<Vec<_>>>()?;
                    idx.entry(key).or_default().push(k);
                }
                indexes.insert(mask.clone(), idx);
            }
            let key = args
                .iter()
                .zip(&mask)
                .filter(|(_, b)| **b)
                .map(|(a, _)| match a {
                    Pt::Const(c) => self.canon(*c),
                    Pt::Var(s) => self.canon(r[*s]),
                })
                .collect::<Result<Vec<_>>>()?;
            let Some(hits) = indexes[&mask].get(&key) else {
                continue;
            };
            'f: for k in hits {
                let tuple = &rows[*k];
                let mut nr = r.clone();
                for (a, v) in args.iter().zip(tuple) {
                    if let Pt::Var(s) = a {
                        if nr[*s].is_null() {
                            nr[*s] = *v;
                        } else if !self.same(nr[*s], *v)? {
                            // a variable repeated inside the atom
                            continue 'f;
                        }
                    }
                }
                out.push((i, nr));
            }
        }
        Ok(out)
    }

    // ---- aggregates -----------------------------------------------------------------

    fn eval_aggregate(
        &self,
        inner: &Node,
        keys: &[Slot],
        aggs: &[(Slot, Agg)],
        row: &Row,
    ) -> Result<Vec<Row>> {
        let rows = self.run(inner, row.len())?;
        let mut groups: Vec<(Vec<Id>, Vec<Row>)> = Vec::new();
        let mut index: HashMap<Vec<Id>, usize> = HashMap::new();
        for r in rows {
            let k = keys
                .iter()
                .map(|s| self.canon(r[*s]))
                .collect::<Result<Vec<_>>>()?;
            match index.get(&k) {
                Some(g) => groups[*g].1.push(r),
                None => {
                    index.insert(k.clone(), groups.len());
                    groups.push((k, vec![r]));
                }
            }
        }
        if groups.is_empty() && keys.is_empty() {
            groups.push((Vec::new(), Vec::new()));
        }
        let mut out = Vec::new();
        'g: for (_, members) in groups {
            let mut nr = row.clone();
            if let Some(first) = members.first() {
                for s in keys {
                    nr[*s] = first[*s];
                }
            }
            for (slot, agg) in aggs {
                if let Ok(Some(t)) = self.aggregate(agg, &members) {
                    let id = self.encode(&t)?;
                    if !nr[*slot].is_null() && !self.same(nr[*slot], id)? {
                        continue 'g;
                    }
                    nr[*slot] = id;
                }
            }
            out.push(nr);
        }
        Ok(out)
    }

    pub(crate) fn aggregate(
        &self,
        agg: &Agg,
        rows: &[Row],
    ) -> std::result::Result<Option<Term>, ExprError> {
        let err = |e: Error| ExprError(e.to_string());
        let values = |e: &Expr| -> std::result::Result<Vec<Term>, ExprError> {
            let mut v = Vec::new();
            let mut seen = HashSet::new();
            for r in rows {
                match self.expr(e, r) {
                    Ok(t) => {
                        if !agg.distinct || seen.insert(t.clone()) {
                            v.push(t);
                        }
                    }
                    Err(x) if matches!(agg.kind, AggKind::Count(_) | AggKind::Sample(_)) => {
                        let _ = x;
                    }
                    Err(x) => return Err(x),
                }
            }
            Ok(v)
        };
        let int = |n: usize| Term::Literal(Literal::integer(n as i64));
        Ok(Some(match &agg.kind {
            AggKind::CountAll => {
                if agg.distinct {
                    let mut seen = HashSet::new();
                    for r in rows {
                        let k = r
                            .iter()
                            .map(|v| self.canon(*v))
                            .collect::<Result<Vec<_>>>()
                            .map_err(err)?;
                        seen.insert(k);
                    }
                    int(seen.len())
                } else {
                    int(rows.len())
                }
            }
            AggKind::Count(e) => int(values(e)?.len()),
            AggKind::Sum(e) => {
                let mut acc = Term::Literal(Literal::integer(0));
                for v in values(e)? {
                    acc = func::add(&acc, &v)?;
                }
                acc
            }
            AggKind::Avg(e) => {
                let vs = values(e)?;
                if vs.is_empty() {
                    return Ok(Some(Term::Literal(Literal::integer(0))));
                }
                let mut acc = Term::Literal(Literal::integer(0));
                for v in &vs {
                    acc = func::add(&acc, v)?;
                }
                func::div(&acc, &Term::Literal(Literal::integer(vs.len() as i64)))?
            }
            AggKind::Min(e) | AggKind::Max(e) => {
                let vs = values(e)?;
                let max = matches!(agg.kind, AggKind::Max(_));
                let best = vs.into_iter().reduce(|a, b| {
                    let c = func::order_cmp(Some(&a), Some(&b));
                    if (c == Ordering::Greater) == max {
                        a
                    } else {
                        b
                    }
                });
                match best {
                    Some(b) => b,
                    None => return Ok(None),
                }
            }
            AggKind::Sample(e) => match values(e)?.into_iter().next() {
                Some(v) => v,
                None => return Ok(None),
            },
            AggKind::GroupConcat(e, sep) => {
                let vs = values(e)?;
                let mut parts = Vec::new();
                for v in &vs {
                    let Term::Literal(l) = v else {
                        return Err(ExprError("GROUP_CONCAT of a non-literal".into()));
                    };
                    parts.push(l.lexical.clone());
                }
                // The result is a simple literal, whatever the language tags
                // of the parts (W3C tests agg-groupconcat-04, -06).
                Term::Literal(Literal::string(parts.join(sep)))
            }
        }))
    }

    // ---- expressions -------------------------------------------------------------------

    fn term_of(&self, id: Id) -> std::result::Result<Term, ExprError> {
        if id.is_null() {
            return Err(ExprError("unbound variable".into()));
        }
        self.decode(id).map_err(|e| ExprError(e.to_string()))
    }

    /// Effective boolean value of an expression; errors are false.
    pub fn ebv(&self, e: &Expr, r: &Row) -> Result<bool> {
        Ok(match self.expr(e, r) {
            Ok(t) => func::ebv(&t).unwrap_or(false),
            Err(_) => false,
        })
    }

    pub fn expr(&self, e: &Expr, r: &Row) -> std::result::Result<Term, ExprError> {
        let b = |v: bool| Term::Literal(Literal::boolean(v));
        Ok(match e {
            Expr::Const(t) => t.clone(),
            Expr::ConstId(id) => self.term_of(*id)?,
            Expr::Var(s) => {
                // Fast path for inline comparisons happens in `cmp_ids`.
                self.term_of(r[*s])?
            }
            Expr::Or(a, c) => {
                let x = self.expr(a, r).and_then(|t| func::ebv(&t));
                let y = self.expr(c, r).and_then(|t| func::ebv(&t));
                match (x, y) {
                    (Ok(true), _) | (_, Ok(true)) => b(true),
                    (Ok(false), Ok(false)) => b(false),
                    (Err(e), _) | (_, Err(e)) => return Err(e),
                }
            }
            Expr::And(a, c) => {
                let x = self.expr(a, r).and_then(|t| func::ebv(&t));
                let y = self.expr(c, r).and_then(|t| func::ebv(&t));
                match (x, y) {
                    (Ok(false), _) | (_, Ok(false)) => b(false),
                    (Ok(true), Ok(true)) => b(true),
                    (Err(e), _) | (_, Err(e)) => return Err(e),
                }
            }
            Expr::Eq(a, c) => {
                if let Some(o) = self.cmp_ids(a, c, r) {
                    return Ok(b(o == Ordering::Equal));
                }
                let (x, y) = self.lift_pair(self.expr(a, r)?, self.expr(c, r)?);
                b(func::equal(&x, &y)?)
            }
            Expr::SameTerm(a, c) => {
                let (x, y) = self.lift_pair(self.expr(a, r)?, self.expr(c, r)?);
                b(func::same_term(&x, &y))
            }
            Expr::Gt(a, c) | Expr::Ge(a, c) | Expr::Lt(a, c) | Expr::Le(a, c) => {
                let o = match self.cmp_ids(a, c, r) {
                    Some(o) => o,
                    None => func::compare(&self.expr(a, r)?, &self.expr(c, r)?)?,
                };
                b(match e {
                    Expr::Gt(..) => o == Ordering::Greater,
                    Expr::Ge(..) => o != Ordering::Less,
                    Expr::Lt(..) => o == Ordering::Less,
                    _ => o != Ordering::Greater,
                })
            }
            Expr::In(a, list) => {
                let x = self.expr(a, r)?;
                let mut err = None;
                for item in list {
                    let eq = self.expr(item, r).and_then(|y| {
                        let (x, y) = self.lift_pair(x.clone(), y);
                        func::equal(&x, &y)
                    });
                    match eq {
                        Ok(true) => return Ok(b(true)),
                        Ok(false) => {}
                        Err(e) => err = Some(e),
                    }
                }
                match err {
                    Some(e) => return Err(e),
                    None => b(false),
                }
            }
            Expr::Add(a, c) => func::add(&self.expr(a, r)?, &self.expr(c, r)?)?,
            Expr::Sub(a, c) => func::sub(&self.expr(a, r)?, &self.expr(c, r)?)?,
            Expr::Mul(a, c) => func::mul(&self.expr(a, r)?, &self.expr(c, r)?)?,
            Expr::Div(a, c) => func::div(&self.expr(a, r)?, &self.expr(c, r)?)?,
            Expr::Plus(a) => func::pos(&self.expr(a, r)?)?,
            Expr::Minus(a) => func::neg(&self.expr(a, r)?)?,
            Expr::Not(a) => func::not(&self.expr(a, r)?)?,
            Expr::Exists(n) => {
                let found = self
                    .eval(n, std::slice::from_ref(r))
                    .map_err(|e| ExprError(e.to_string()))?;
                b(!found.is_empty())
            }
            Expr::Bound(s) => b(!r[*s].is_null()),
            Expr::IsKind(s, k) => {
                let id = r[*s];
                b(!id.is_null()
                    && if k & 0x80 != 0 {
                        id.is_literal()
                    } else {
                        id.kind() as u8 == *k
                    })
            }
            Expr::If(c, x, y) => {
                if func::ebv(&self.expr(c, r)?)? {
                    self.expr(x, r)?
                } else {
                    self.expr(y, r)?
                }
            }
            Expr::Coalesce(list) => {
                for item in list {
                    if let Ok(t) = self.expr(item, r) {
                        return Ok(t);
                    }
                }
                return Err(ExprError("COALESCE: no bound argument".into()));
            }
            Expr::Call(f, args) => {
                let vals = args
                    .iter()
                    .map(|a| self.expr(a, r))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                func::call(f, &vals, &mut self.fctx.borrow_mut())?
            }
        })
    }

    /// Comparisons of two inline literals of the same subtype are integer
    /// comparisons on the IDs (rule `ir-builtin-inline-compare`).
    fn cmp_ids(&self, a: &Expr, c: &Expr, r: &Row) -> Option<Ordering> {
        let id = |e: &Expr| match e {
            Expr::Var(s) => Some(r[*s]),
            Expr::ConstId(i) => Some(*i),
            _ => None,
        };
        let (x, y) = (id(a)?, id(c)?);
        if x.is_null() || y.is_null() || !x.is_inline() || !y.is_inline() {
            return None;
        }
        // Same inline class and subtype; doubles are class 0b10.
        let same_class = x.raw() >> 62 == y.raw() >> 62
            && (x.raw() >> 62 == 0b10 || (x.subtype().is_some() && x.subtype() == y.subtype()));
        // Strings compare by code point, which equals the byte order of the
        // packed UTF-8 only for plain strings; language strings differ by tag.
        if !same_class || matches!(x.subtype(), Some(crate::id::Subtype::LangString)) {
            return None;
        }
        // dateTime with and without timezone are not totally ordered.
        if matches!(
            x.subtype(),
            Some(crate::id::Subtype::DateTime | crate::id::Subtype::Date)
        ) {
            return None;
        }
        Some(x.raw().cmp(&y.raw()))
    }
}

/// The statement a standard-mode reifier blank node label names.
fn stmt_blank(label: &str) -> Option<Id> {
    let id = Id(label.strip_prefix(STMT_BLANK)?.parse().ok()?);
    (id.kind() == Kind::Statement).then_some(id)
}

fn dedup_keep_order(v: Vec<Id>) -> Vec<Id> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|x| seen.insert(*x)).collect()
}

fn node_name(n: &Node) -> &'static str {
    match n {
        Node::Unit => "Unit",
        Node::Empty => "Empty",
        Node::Triple(_) => "Scan",
        Node::Path { .. } => "Path",
        Node::NamedGraphs(_) => "Scan(graphs)",
        Node::PerGraph { .. } => "PerGraph",
        Node::Join(_) => "Join",
        Node::LeftJoin(..) => "LeftJoin",
        Node::Minus(..) => "Minus",
        Node::AntiJoin(..) => "AntiJoin",
        Node::Union(_) => "Union",
        Node::Filter(..) => "Filter",
        Node::Extend(..) => "Extend",
        Node::Values(..) => "Values",
        Node::Aggregate { .. } => "Aggregate",
        Node::Distinct(_) => "Distinct",
        Node::OrderBy(..) => "OrderBy",
        Node::Slice(..) => "Slice",
        Node::Project(..) => "Project",
        Node::Rel { .. } => "Scan(rel)",
        Node::Datalog { .. } => "Datalog",
    }
}
