//! SPARQL 1.2 query and update ([[11-sparql]], [[adr:0038]]): `spargebra`
//! parses, this module translates the algebra into the shared IR and runs
//! it, and applies updates through the store.
//!
//! graphlava deviations ([[11-sparql#Deviations]]): reifiers are statement
//! IDs, `GRAPH` is transitive (`GRAPH!` direct), a triple in several graphs
//! is one statement. [`QueryOptions::standard`] switches to SPARQL 1.1
//! dataset semantics for the conformance suites.

// @tg: implements:: [[openspec:sparql-query#SPARQL 1.2 query forms]]
// @tg: implements:: [[openspec:sparql-query#SPARQL update]]
// @tg: implements:: [[openspec:sparql-query#Reifiers are statement IDs]]
// @tg: implements:: [[openspec:sparql-query#In-place update through the same reifier]]
// @tg: implements:: [[openspec:sparql-query#Graph scope semantics]]
// @tg: implements:: [[openspec:sparql-query#Datalog through SERVICE]]
// @tg: implements:: [[openspec:sparql-query#Conformance]]

pub mod update;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use spargebra::algebra::{
    AggregateExpression, AggregateFunction, Expression, GraphPattern, OrderExpression,
    PropertyPathExpression,
};
use spargebra::term::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern};
use spargebra::{Query, SparqlParser};

use crate::datalog;
use crate::error::{Error, Result};
use crate::id::Id;
use crate::query::exec::{Exec, Row};
use crate::query::ir::*;
use crate::term::{Literal, Term};

/// Marker IRI the `GRAPH!` pre-pass puts at the start of a direct group.
const DIRECT_MARK: &str = "urn:graphlava:graph-direct";
/// The Datalog service IRI ([[11-sparql#Datalog service]]).
pub const DATALOG_SERVICE: &str = "https://graphlava.org/ns#datalog";
const RDF_REIFIES: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies";

/// Per-query settings.
#[derive(Debug, Clone, Default)]
pub struct QueryOptions {
    /// SPARQL 1.1 semantics: `GRAPH` is direct (rule `sparql-graph-direct-mode`),
    /// the default graph is only the file's default graph, and `DELETE DATA`
    /// without `GRAPH` removes only the default-graph membership.
    pub standard: bool,
    pub base_iri: Option<String>,
    /// Row budget (default 10 million).
    pub max_rows: Option<usize>,
    pub timeout: Option<std::time::Duration>,
}

impl QueryOptions {
    /// The conformance mode of [[11-sparql#Conformance]].
    pub fn standard() -> QueryOptions {
        QueryOptions {
            standard: true,
            ..QueryOptions::default()
        }
    }
}

/// A query result, decoded at the final projection (rule `api-rows-late`).
#[derive(Debug, Clone, PartialEq)]
pub enum QueryResult {
    Solutions {
        vars: Vec<String>,
        rows: Vec<Vec<Option<Term>>>,
    },
    Boolean(bool),
    Graph(Vec<[Term; 3]>),
}

impl QueryResult {
    /// The rows of a solutions result (empty for the other forms).
    pub fn rows(&self) -> &[Vec<Option<Term>>] {
        match self {
            QueryResult::Solutions { rows, .. } => rows,
            _ => &[],
        }
    }
    /// The column of `var` in every row.
    pub fn column(&self, var: &str) -> Vec<Option<Term>> {
        match self {
            QueryResult::Solutions { vars, rows } => match vars.iter().position(|v| v == var) {
                Some(i) => rows.iter().map(|r| r[i].clone()).collect(),
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }
}

fn invalid<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(m.into()))
}

/// Rewrites `GRAPH!` (outside strings, IRIs and comments) to `GRAPH` with a
/// marker triple opening the group, since the parser has no `GRAPH!`.
pub fn prepass(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut pending = false;
    while i < b.len() {
        let c = b[i] as char;
        match c {
            '"' | '\'' => {
                // copy a string literal (long or short form)
                let q = c;
                let long = text[i..].starts_with(&format!("{q}{q}{q}"));
                let start = i;
                if long {
                    i += 3;
                    while i < b.len() && !text[i..].starts_with(&format!("{q}{q}{q}")) {
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                    i = (i + 3).min(b.len());
                } else {
                    i += 1;
                    while i < b.len() && b[i] as char != q && b[i] != b'\n' {
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                    i = (i + 1).min(b.len());
                }
                out.push_str(&text[start..i.min(text.len())]);
                continue;
            }
            '<' => {
                // an IRI if a '>' comes before whitespace
                if let Some(end) = text[i..].find(|ch: char| ch == '>' || ch.is_whitespace()) {
                    if text[i + end..].starts_with('>') {
                        out.push_str(&text[i..i + end + 1]);
                        i += end + 1;
                        continue;
                    }
                }
            }
            '#' => {
                let end = text[i..].find('\n').map_or(b.len(), |e| i + e);
                out.push_str(&text[i..end]);
                i = end;
                continue;
            }
            '{' if pending => {
                out.push_str(&format!(
                    "{{ <{DIRECT_MARK}> <{DIRECT_MARK}> <{DIRECT_MARK}> . "
                ));
                pending = false;
                i += 1;
                continue;
            }
            _ => {}
        }
        if (c == 'G' || c == 'g')
            && text.len() >= i + 6
            && text[i..i + 6].eq_ignore_ascii_case("GRAPH!")
            && (i == 0 || !(b[i - 1] as char).is_alphanumeric())
        {
            out.push_str("GRAPH");
            pending = true;
            i += 6;
            continue;
        }
        out.push(c);
        i += c.len_utf8().max(1);
        // keep multi-byte characters intact
        while i < b.len() && !text.is_char_boundary(i) {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// Converts an `oxrdf` literal to a term.
pub fn literal_term(l: &oxrdf::Literal) -> Term {
    match l.language() {
        Some(lang) => Term::Literal(Literal::lang(l.value(), lang)),
        None => Term::Literal(Literal::typed(l.value(), l.datatype().as_str())),
    }
}

/// The triple term of a ground triple (`<<( … )>>` in `VALUES`).
pub fn ground_triple_term(t: &spargebra::term::GroundTriple) -> Term {
    Term::triple(
        Term::Iri(t.subject.as_str().into()),
        Term::Iri(t.predicate.as_str().into()),
        ground_term(&t.object),
    )
}

/// The term of a ground term.
pub fn ground_term(t: &GroundTerm) -> Term {
    match t {
        GroundTerm::NamedNode(n) => Term::Iri(n.as_str().into()),
        GroundTerm::Literal(l) => literal_term(l),
        GroundTerm::Triple(x) => ground_triple_term(x),
    }
}

/// Translation of the algebra into IR over one view.
#[allow(missing_debug_implementations)]
pub struct Translator<'e, 'a> {
    pub ex: &'e Exec<'a>,
    pub vars: Vec<String>,
    pub opts: QueryOptions,
    /// Whether a constant was unknown to the dictionary (the plan is then not cacheable).
    pub transient_consts: bool,
    /// Standard mode with a dataset clause: the named graphs of `FROM NAMED`
    /// (`USING NAMED`); `GRAPH` sees only these.
    named: Option<Vec<Id>>,
    /// The graphs merged into the default graph by several `FROM` clauses
    /// (read through [`MULTI_DEFAULT`]).
    defaults: Vec<Id>,
}

/// Graph spec standing for the merge of the `FROM` graphs in
/// [`Translator::defaults`]; expanded by every triple pattern and never
/// reaching the IR.
const MULTI_DEFAULT: GraphSpec = GraphSpec::Graph {
    g: Pt::Var(usize::MAX),
    direct: true,
};

impl<'e, 'a> Translator<'e, 'a> {
    pub fn new(ex: &'e Exec<'a>, opts: QueryOptions) -> Self {
        Translator {
            ex,
            vars: Vec::new(),
            opts,
            transient_consts: false,
            named: None,
            defaults: Vec::new(),
        }
    }

    fn graph_const(&mut self, iri: &str) -> Result<Id> {
        let gid = self.ex.graph_id(&Term::Iri(iri.into()))?;
        if gid.kind() == crate::id::Kind::System {
            self.transient_consts = true;
        }
        Ok(gid)
    }

    /// The default graph of a dataset clause (`FROM` / `FROM NAMED`, or
    /// `USING` / `USING NAMED` in an update). In standard mode the clause
    /// defines the whole dataset: the `FROM` graphs merged as the default
    /// graph and only the `FROM NAMED` graphs visible to `GRAPH`.
    pub fn dataset(&mut self, ds: Option<&spargebra::algebra::QueryDataset>) -> Result<GraphSpec> {
        let base = self.default_graph();
        let Some(ds) = ds else {
            return Ok(base);
        };
        if !self.opts.standard {
            if ds.named.as_ref().is_some_and(|n| !n.is_empty()) || ds.default.len() > 1 {
                return invalid("FROM NAMED and several FROM graphs are not supported in v0");
            }
            return Ok(match ds.default.first() {
                // `FROM :g`: the default graph is the scope of :g
                Some(g) => GraphSpec::Graph {
                    g: Pt::Const(self.graph_const(g.as_str())?),
                    direct: false,
                },
                None => base,
            });
        }
        // `named: None` (`WITH`) leaves the named graphs of the store visible.
        if let Some(list) = &ds.named {
            let mut named = Vec::new();
            for g in list {
                named.push(self.graph_const(g.as_str())?);
            }
            self.named = Some(named);
        }
        let mut defaults = Vec::new();
        for g in &ds.default {
            defaults.push(self.graph_const(g.as_str())?);
        }
        Ok(if defaults.len() == 1 {
            GraphSpec::Graph {
                g: Pt::Const(defaults[0]),
                direct: true,
            }
        } else {
            self.defaults = defaults;
            MULTI_DEFAULT
        })
    }

    pub fn var(&mut self, name: &str) -> Slot {
        match self.vars.iter().position(|v| v == name) {
            Some(i) => i,
            None => {
                self.vars.push(name.to_string());
                self.vars.len() - 1
            }
        }
    }

    fn fresh(&mut self) -> Slot {
        let n = format!("__t{}", self.vars.len());
        self.var(&n)
    }

    fn const_id(&mut self, t: &Term) -> Result<Id> {
        let id = self.ex.encode(t)?;
        if id.kind() == crate::id::Kind::System {
            self.transient_consts = true;
        }
        Ok(id)
    }

    fn default_graph(&self) -> GraphSpec {
        if self.opts.standard {
            GraphSpec::DefaultOnly
        } else {
            GraphSpec::Union
        }
    }

    /// A term position; triple terms (`<<( … )>>`) add a reifier pattern and
    /// stand for its statement ID.
    #[allow(clippy::only_used_in_recursion)]
    fn pt(&mut self, t: &TermPattern, graph: GraphSpec, extra: &mut Vec<Node>) -> Result<Pt> {
        Ok(match t {
            TermPattern::NamedNode(n) => Pt::Const(self.const_id(&Term::Iri(n.as_str().into()))?),
            TermPattern::BlankNode(b) => Pt::Var(self.var(&format!("_:{}", b.as_str()))),
            TermPattern::Literal(l) => Pt::Const(self.const_id(&literal_term(l))?),
            TermPattern::Variable(v) => Pt::Var(self.var(v.as_str())),
            TermPattern::Triple(tp) => {
                let r = Pt::Var(self.fresh());
                let s = self.pt(&tp.subject, graph, extra)?;
                let p = self.np(&tp.predicate)?;
                let o = self.pt(&tp.object, graph, extra)?;
                extra.push(Node::Triple(TriplePat {
                    s,
                    p,
                    o,
                    id: Some(r),
                    graph: GraphSpec::Union,
                }));
                r
            }
        })
    }

    fn np(&mut self, n: &NamedNodePattern) -> Result<Pt> {
        Ok(match n {
            NamedNodePattern::NamedNode(n) => {
                Pt::Const(self.const_id(&Term::Iri(n.as_str().into()))?)
            }
            NamedNodePattern::Variable(v) => Pt::Var(self.var(v.as_str())),
        })
    }

    fn triple(&mut self, t: &TriplePattern, graph: GraphSpec) -> Result<Vec<Node>> {
        let mut extra = Vec::new();
        // `?r rdf:reifies <<( s p o )>>` binds ?r to the statement ID (rule
        // `ir-reifies-virtual`): one scan, no join.
        if let (NamedNodePattern::NamedNode(p), TermPattern::Triple(tt)) = (&t.predicate, &t.object)
        {
            if p.as_str() == RDF_REIFIES {
                let r = self.pt(&t.subject, graph, &mut extra)?;
                let s = self.pt(&tt.subject, graph, &mut extra)?;
                let pp = self.np(&tt.predicate)?;
                let o = self.pt(&tt.object, graph, &mut extra)?;
                extra.push(Node::Triple(TriplePat {
                    s,
                    p: pp,
                    o,
                    id: Some(r),
                    graph: GraphSpec::Union,
                }));
                return Ok(extra);
            }
        }
        let s = self.pt(&t.subject, graph, &mut extra)?;
        let p = self.np(&t.predicate)?;
        let o = self.pt(&t.object, graph, &mut extra)?;
        if graph == MULTI_DEFAULT {
            // The RDF merge of the `FROM` graphs: a triple counts once
            // whichever graphs hold it.
            let parts: Vec<Node> = self
                .defaults
                .iter()
                .map(|g| {
                    Node::Triple(TriplePat {
                        s,
                        p,
                        o,
                        id: None,
                        graph: GraphSpec::Graph {
                            g: Pt::Const(*g),
                            direct: true,
                        },
                    })
                })
                .collect();
            let keep: Vec<Slot> = [s, p, o]
                .iter()
                .filter_map(|x| match x {
                    Pt::Var(v) => Some(*v),
                    Pt::Const(_) => None,
                })
                .collect();
            extra.push(if parts.is_empty() {
                Node::Empty
            } else {
                Node::Distinct(Box::new(Node::Project(Box::new(Node::Union(parts)), keep)))
            });
            return Ok(extra);
        }
        extra.push(Node::Triple(TriplePat {
            s,
            p,
            o,
            id: None,
            graph,
        }));
        Ok(extra)
    }

    fn path(&mut self, p: &PropertyPathExpression) -> Result<PathExpr> {
        let iri = |t: &mut Self, n: &oxrdf::NamedNode| t.const_id(&Term::Iri(n.as_str().into()));
        Ok(match p {
            PropertyPathExpression::NamedNode(n) => PathExpr::Pred(iri(self, n)?),
            PropertyPathExpression::Reverse(a) => PathExpr::Inv(Box::new(self.path(a)?)),
            PropertyPathExpression::Sequence(a, b) => {
                PathExpr::Seq(Box::new(self.path(a)?), Box::new(self.path(b)?))
            }
            PropertyPathExpression::Alternative(a, b) => {
                PathExpr::Alt(Box::new(self.path(a)?), Box::new(self.path(b)?))
            }
            PropertyPathExpression::ZeroOrMore(a) => PathExpr::Star(Box::new(self.path(a)?)),
            PropertyPathExpression::OneOrMore(a) => PathExpr::Plus(Box::new(self.path(a)?)),
            PropertyPathExpression::ZeroOrOne(a) => PathExpr::Opt(Box::new(self.path(a)?)),
            PropertyPathExpression::NegatedPropertySet(v) => PathExpr::Neg {
                fwd: v.iter().map(|n| iri(self, n)).collect::<Result<_>>()?,
                inv: Vec::new(),
            },
        })
    }

    /// Translates a graph pattern evaluated against `graph`.
    pub fn pattern(&mut self, gp: &GraphPattern, graph: GraphSpec) -> Result<Node> {
        Ok(match gp {
            GraphPattern::Bgp { patterns } => {
                let mut parts = Vec::new();
                for t in patterns {
                    parts.extend(self.triple(t, graph)?);
                }
                match parts.len() {
                    0 => Node::Unit,
                    1 => parts.pop().expect("one"),
                    _ => Node::Join(parts),
                }
            }
            GraphPattern::Path {
                subject,
                path,
                object,
            } => {
                let mut extra = Vec::new();
                let s = self.pt(subject, graph, &mut extra)?;
                let o = self.pt(object, graph, &mut extra)?;
                let path = self.path(path)?;
                if graph == MULTI_DEFAULT {
                    return invalid(
                        "a property path over several FROM graphs is not supported in v0",
                    );
                }
                extra.push(Node::Path { s, path, o, graph });
                if extra.len() == 1 {
                    extra.pop().expect("one")
                } else {
                    Node::Join(extra)
                }
            }
            GraphPattern::Join { left, right } => {
                let mut parts = Vec::new();
                for x in [self.pattern(left, graph)?, self.pattern(right, graph)?] {
                    match x {
                        Node::Join(v) => parts.extend(v),
                        Node::Unit => {}
                        other => parts.push(other),
                    }
                }
                match parts.len() {
                    0 => Node::Unit,
                    1 => parts.pop().expect("one"),
                    _ => Node::Join(parts),
                }
            }
            GraphPattern::LeftJoin {
                left,
                right,
                expression,
            } => {
                let l = self.pattern(left, graph)?;
                let r = self.pattern(right, graph)?;
                let e = expression
                    .as_ref()
                    .map(|e| self.expr(e, graph))
                    .transpose()?;
                Node::LeftJoin(Box::new(l), Box::new(r), e)
            }
            GraphPattern::Filter { expr, inner } => {
                let i = self.pattern(inner, graph)?;
                Node::Filter(self.expr(expr, graph)?, Box::new(i))
            }
            GraphPattern::Union { left, right } => Node::Union(vec![
                self.pattern(left, graph)?,
                self.pattern(right, graph)?,
            ]),
            GraphPattern::Graph { name, inner } => {
                let (direct_mark, inner) = strip_mark(inner);
                let direct = self.opts.standard || direct_mark;
                if let NamedNodePattern::Variable(v) = name {
                    return self.graph_var(v.as_str(), &inner, direct);
                }
                let NamedNodePattern::NamedNode(n) = name else {
                    unreachable!("graph variables are handled above")
                };
                let gid = self.graph_const(n.as_str())?;
                if self.named.as_ref().is_some_and(|l| !l.contains(&gid)) {
                    // not a named graph of the dataset
                    return Ok(Node::Empty);
                }
                self.pattern(
                    &inner,
                    GraphSpec::Graph {
                        g: Pt::Const(gid),
                        direct,
                    },
                )?
            }
            GraphPattern::Extend {
                inner,
                variable,
                expression,
            } => {
                let i = self.pattern(inner, graph)?;
                let e = self.expr(expression, graph)?;
                Node::Extend(Box::new(i), self.var(variable.as_str()), e)
            }
            GraphPattern::Minus { left, right } => Node::Minus(
                Box::new(self.pattern(left, graph)?),
                Box::new(self.pattern(right, graph)?),
            ),
            GraphPattern::Values {
                variables,
                bindings,
            } => {
                let slots: Vec<Slot> = variables.iter().map(|v| self.var(v.as_str())).collect();
                let mut rows = Vec::new();
                for b in bindings {
                    let mut row = Vec::new();
                    for x in b {
                        row.push(match x {
                            None => None,
                            Some(GroundTerm::NamedNode(n)) => {
                                Some(self.const_id(&Term::Iri(n.as_str().into()))?)
                            }
                            Some(GroundTerm::Literal(l)) => Some(self.const_id(&literal_term(l))?),
                            Some(GroundTerm::Triple(t)) => {
                                Some(self.const_id(&ground_triple_term(t))?)
                            }
                        });
                    }
                    rows.push(row);
                }
                Node::Values(slots, rows)
            }
            GraphPattern::OrderBy { inner, expression } => {
                let i = self.pattern(inner, graph)?;
                let keys = expression
                    .iter()
                    .map(|o| match o {
                        OrderExpression::Asc(e) => Ok((self.expr(e, graph)?, false)),
                        OrderExpression::Desc(e) => Ok((self.expr(e, graph)?, true)),
                    })
                    .collect::<Result<Vec<_>>>()?;
                Node::OrderBy(Box::new(i), keys)
            }
            GraphPattern::Project { inner, variables } => {
                let i = self.pattern(inner, graph)?;
                Node::Project(
                    Box::new(i),
                    variables.iter().map(|v| self.var(v.as_str())).collect(),
                )
            }
            GraphPattern::Distinct { inner } => {
                Node::Distinct(Box::new(self.pattern(inner, graph)?))
            }
            GraphPattern::Reduced { inner } => self.pattern(inner, graph)?,
            GraphPattern::Slice {
                inner,
                start,
                length,
            } => Node::Slice(Box::new(self.pattern(inner, graph)?), *start, *length),
            GraphPattern::Group {
                inner,
                variables,
                aggregates,
            } => {
                let i = self.pattern(inner, graph)?;
                let keys = variables.iter().map(|v| self.var(v.as_str())).collect();
                let mut aggs = Vec::new();
                for (v, a) in aggregates {
                    let slot = self.var(v.as_str());
                    aggs.push((slot, self.aggregate(a, graph)?));
                }
                Node::Aggregate {
                    input: Box::new(i),
                    keys,
                    aggs,
                }
            }
            GraphPattern::Service {
                name,
                inner,
                silent,
            } => {
                let iri = match name {
                    NamedNodePattern::NamedNode(n) => n.as_str().to_string(),
                    NamedNodePattern::Variable(_) => {
                        return invalid("SERVICE with a variable IRI is not supported");
                    }
                };
                if iri == DATALOG_SERVICE || iri == "meta:datalog" {
                    self.datalog_service(inner)?
                } else if *silent {
                    // rule `sparql-service-resolver`: one empty solution
                    Node::Unit
                } else {
                    return invalid(format!(
                        "unknown SERVICE <{iri}>: only <meta:datalog> is local in v0"
                    ));
                }
            }
            #[allow(unreachable_patterns)]
            other => return invalid(format!("unsupported graph pattern: {other}")),
        })
    }

    /// `GRAPH ?g { P }`. When every row of `P` comes from a scan in the graph,
    /// the scans bind `?g` directly. Otherwise (subqueries, aggregates,
    /// negation, `VALUES`, `EXISTS`) `P` is evaluated once per named graph so
    /// that its inner scopes see one active graph at a time.
    fn graph_var(&mut self, var: &str, inner: &GraphPattern, direct: bool) -> Result<Node> {
        let user = self.var(var);
        let active = self.fresh();
        let mut node = self.pattern(
            inner,
            GraphSpec::Graph {
                g: Pt::Var(active),
                direct,
            },
        )?;
        if matches!(node, Node::Unit) {
            // `GRAPH ?g {}` still binds ?g to each named graph.
            return Ok(match &self.named {
                Some(l) => Node::Values(vec![user], l.iter().map(|g| vec![Some(*g)]).collect()),
                None => Node::NamedGraphs(user),
            });
        }
        if self.named.is_none() && binds_graph(&node) {
            node.rebind_graph(active, Pt::Var(user));
            return Ok(node);
        }
        if !direct {
            // `GRAPH ?g` binds direct graphs only (rule `sparql-graph-var-direct`).
            node = self.pattern(
                inner,
                GraphSpec::Graph {
                    g: Pt::Var(active),
                    direct: true,
                },
            )?;
        }
        Ok(Node::PerGraph {
            var: user,
            active,
            graphs: self.named.clone(),
            inner: Box::new(node),
        })
    }

    fn aggregate(&mut self, a: &AggregateExpression, graph: GraphSpec) -> Result<Agg> {
        Ok(match a {
            AggregateExpression::CountSolutions { distinct } => Agg {
                kind: AggKind::CountAll,
                distinct: *distinct,
            },
            AggregateExpression::FunctionCall {
                name,
                expr,
                distinct,
            } => {
                let e = self.expr(expr, graph)?;
                let kind = match name {
                    AggregateFunction::Count => AggKind::Count(e),
                    AggregateFunction::Sum => AggKind::Sum(e),
                    AggregateFunction::Avg => AggKind::Avg(e),
                    AggregateFunction::Min => AggKind::Min(e),
                    AggregateFunction::Max => AggKind::Max(e),
                    AggregateFunction::Sample => AggKind::Sample(e),
                    AggregateFunction::GroupConcat { separator } => {
                        AggKind::GroupConcat(e, separator.clone().unwrap_or_else(|| " ".into()))
                    }
                    AggregateFunction::Custom(n) => {
                        return invalid(format!("unknown aggregate <{}>", n.as_str()));
                    }
                };
                Agg {
                    kind,
                    distinct: *distinct,
                }
            }
        })
    }

    pub fn expr(&mut self, e: &Expression, graph: GraphSpec) -> Result<Expr> {
        let bx = |t: &mut Self, x: &Expression| t.expr(x, graph).map(Box::new);
        Ok(match e {
            Expression::NamedNode(n) => match self.ex.special_iri(n.as_str())? {
                Some(id) => Expr::ConstId(id),
                None => Expr::Const(Term::Iri(n.as_str().into())),
            },
            Expression::Literal(l) => Expr::Const(literal_term(l)),
            Expression::Variable(v) => Expr::Var(self.var(v.as_str())),
            Expression::Or(a, b) => Expr::Or(bx(self, a)?, bx(self, b)?),
            Expression::And(a, b) => Expr::And(bx(self, a)?, bx(self, b)?),
            Expression::Equal(a, b) => Expr::Eq(bx(self, a)?, bx(self, b)?),
            Expression::SameTerm(a, b) => Expr::SameTerm(bx(self, a)?, bx(self, b)?),
            Expression::Greater(a, b) => Expr::Gt(bx(self, a)?, bx(self, b)?),
            Expression::GreaterOrEqual(a, b) => Expr::Ge(bx(self, a)?, bx(self, b)?),
            Expression::Less(a, b) => Expr::Lt(bx(self, a)?, bx(self, b)?),
            Expression::LessOrEqual(a, b) => Expr::Le(bx(self, a)?, bx(self, b)?),
            Expression::In(a, l) => Expr::In(
                bx(self, a)?,
                l.iter()
                    .map(|x| self.expr(x, graph))
                    .collect::<Result<_>>()?,
            ),
            Expression::Add(a, b) => Expr::Add(bx(self, a)?, bx(self, b)?),
            Expression::Subtract(a, b) => Expr::Sub(bx(self, a)?, bx(self, b)?),
            Expression::Multiply(a, b) => Expr::Mul(bx(self, a)?, bx(self, b)?),
            Expression::Divide(a, b) => Expr::Div(bx(self, a)?, bx(self, b)?),
            Expression::UnaryPlus(a) => Expr::Plus(bx(self, a)?),
            Expression::UnaryMinus(a) => Expr::Minus(bx(self, a)?),
            Expression::Not(a) => Expr::Not(bx(self, a)?),
            Expression::Exists(p) => Expr::Exists(Box::new(self.pattern(p, graph)?)),
            Expression::Bound(v) => Expr::Bound(self.var(v.as_str())),
            Expression::If(a, b, c) => Expr::If(bx(self, a)?, bx(self, b)?, bx(self, c)?),
            Expression::Coalesce(l) => Expr::Coalesce(
                l.iter()
                    .map(|x| self.expr(x, graph))
                    .collect::<Result<_>>()?,
            ),
            Expression::FunctionCall(f, args) => Expr::Call(
                f.clone(),
                args.iter()
                    .map(|x| self.expr(x, graph))
                    .collect::<Result<_>>()?,
            ),
        })
    }

    /// `SERVICE <meta:datalog> { "program" ; patterns }` (rules
    /// `sparql-service-datalog-named`, `sparql-service-datalog-inline`).
    /// The inline program arrives as a `VALUES`-like literal pattern: the
    /// parser gives it as a BGP whose first triple has a literal subject, so
    /// the program text is the first string literal found in the group.
    fn datalog_service(&mut self, inner: &GraphPattern) -> Result<Node> {
        let GraphPattern::Bgp { patterns } = inner else {
            return invalid("SERVICE <meta:datalog> takes triple patterns over rule heads");
        };
        let mut program_text = None;
        let mut goals = Vec::new();
        for t in patterns {
            if let TermPattern::Literal(l) = &t.subject {
                if program_text.is_none() && l.datatype().as_str().ends_with("#string") {
                    program_text = Some(l.value().to_string());
                    // the program literal is followed by a dummy predicate/object pair
                    continue;
                }
            }
            goals.push(t);
        }
        let Some(text) = program_text else {
            return invalid(
                "SERVICE <meta:datalog> needs an inline program (named rules arrive with Admin rule installation)",
            );
        };
        let prog = datalog::parse::parse(&text, &[])
            .map_err(|e| Error::Invalid(format!("Datalog line {}: {}", e.line, e.message)))?;
        let compiled = Arc::new(datalog::eval::compile(self.ex, &prog, false)?);
        let mut parts = Vec::new();
        for t in goals {
            let NamedNodePattern::NamedNode(p) = &t.predicate else {
                return invalid("a Datalog service pattern names a rule head");
            };
            let pred = p.as_str();
            // `:reach` maps to the rule head `reach` or `<iri>`
            let local = pred
                .rsplit(['#', '/', ':'])
                .next()
                .unwrap_or(pred)
                .to_string();
            let goal = if compiled.arity.contains_key(&format!("<{pred}>")) {
                format!("<{pred}>")
            } else if compiled.arity.contains_key(&local) {
                local
            } else {
                return invalid(format!(
                    "no rule head named `{local}` in the Datalog service program"
                ));
            };
            if compiled.arity[&goal] != 2 {
                return invalid(format!(
                    "rule head `{goal}` used as a predicate must have two arguments"
                ));
            }
            let mut extra = Vec::new();
            let s = self.pt(&t.subject, GraphSpec::Union, &mut extra)?;
            let o = self.pt(&t.object, GraphSpec::Union, &mut extra)?;
            parts.extend(extra);
            parts.push(Node::Datalog {
                program: compiled.clone(),
                goal,
                args: vec![s, o],
            });
        }
        Ok(match parts.len() {
            0 => Node::Unit,
            1 => parts.pop().expect("one"),
            _ => Node::Join(parts),
        })
    }
}

/// Whether every row of `n` is produced by scans that read the active graph,
/// so a graph-variable scan binds it (see [`Translator::graph_var`]).
fn binds_graph(n: &Node) -> bool {
    let simple = |n: &Node| matches!(n, Node::Unit) || binds_graph(n);
    match n {
        Node::Triple(_) | Node::Path { .. } => true,
        Node::Join(v) => v.iter().all(simple) && v.iter().any(binds_graph),
        Node::Union(v) => v.iter().all(binds_graph),
        Node::LeftJoin(a, b, e) => {
            binds_graph(a) && binds_graph(b) && !e.as_ref().is_some_and(Expr::has_exists)
        }
        Node::Filter(e, a) | Node::Extend(a, _, e) => binds_graph(a) && !e.has_exists(),
        _ => false,
    }
}

/// Removes the `GRAPH!` marker triple from the start of a group.
fn strip_mark(gp: &GraphPattern) -> (bool, GraphPattern) {
    let is_mark = |t: &TriplePattern| matches!(&t.subject, TermPattern::NamedNode(n) if n.as_str() == DIRECT_MARK);
    match gp {
        GraphPattern::Bgp { patterns } if patterns.first().is_some_and(is_mark) => (
            true,
            GraphPattern::Bgp {
                patterns: patterns[1..].to_vec(),
            },
        ),
        GraphPattern::Join { left, right } => {
            let (m, l) = strip_mark(left);
            (
                m,
                GraphPattern::Join {
                    left: Box::new(l),
                    right: right.clone(),
                },
            )
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            let (m, l) = strip_mark(left);
            (
                m,
                GraphPattern::LeftJoin {
                    left: Box::new(l),
                    right: right.clone(),
                    expression: expression.clone(),
                },
            )
        }
        GraphPattern::Filter { expr, inner } => {
            let (m, i) = strip_mark(inner);
            (
                m,
                GraphPattern::Filter {
                    expr: expr.clone(),
                    inner: Box::new(i),
                },
            )
        }
        GraphPattern::Extend {
            inner,
            variable,
            expression,
        } => {
            let (m, i) = strip_mark(inner);
            (
                m,
                GraphPattern::Extend {
                    inner: Box::new(i),
                    variable: variable.clone(),
                    expression: expression.clone(),
                },
            )
        }
        GraphPattern::Minus { left, right } => {
            let (m, l) = strip_mark(left);
            (
                m,
                GraphPattern::Minus {
                    left: Box::new(l),
                    right: right.clone(),
                },
            )
        }
        other => (false, other.clone()),
    }
}

/// Parses query text (after the `GRAPH!` pre-pass).
pub fn parse_query(text: &str, base: Option<&str>) -> Result<Query> {
    let mut p = SparqlParser::new();
    if let Some(b) = base {
        p = p
            .with_base_iri(b)
            .map_err(|e| Error::Invalid(format!("bad base IRI: {e}")))?;
    }
    p = p
        .with_prefix("meta", "https://graphlava.org/ns#")
        .map_err(|e| Error::Invalid(e.to_string()))?;
    p.parse_query(&prepass(text))
        .map_err(|e| Error::Invalid(format!("SPARQL syntax error: {e}")))
}

/// A translated query, ready to run (and to cache).
#[derive(Debug, Clone)]
pub struct Planned {
    pub node: Node,
    pub vars: Vec<String>,
    pub out: Vec<Slot>,
    pub form: Form,
    pub cacheable: bool,
    /// The base IRI `IRI()` resolves against.
    pub base: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Form {
    Select,
    Ask,
    Construct(Vec<TriplePattern>),
    Describe,
}

/// Visible variables of a pattern, for `SELECT *`.
fn visible(n: &Node, out: &mut Vec<Slot>) {
    n.bound_slots(out);
}

/// Translates a parsed query.
pub fn plan(ex: &Exec<'_>, q: &Query, opts: &QueryOptions) -> Result<Planned> {
    let mut tr = Translator::new(ex, opts.clone());
    let (dataset, pattern, form, base) = match q {
        Query::Select {
            dataset,
            pattern,
            base_iri,
        } => (dataset, pattern, Form::Select, base_iri),
        Query::Ask {
            dataset,
            pattern,
            base_iri,
        } => (dataset, pattern, Form::Ask, base_iri),
        Query::Construct {
            dataset,
            pattern,
            template,
            base_iri,
        } => (
            dataset,
            pattern,
            Form::Construct(template.clone()),
            base_iri,
        ),
        Query::Describe {
            dataset,
            pattern,
            base_iri,
        } => (dataset, pattern, Form::Describe, base_iri),
    };
    let graph = tr.dataset(dataset.as_ref())?;
    let node = tr.pattern(pattern, graph)?;
    let projected = variables_of(pattern);
    let out: Vec<Slot> = match &form {
        Form::Select
            if !projected.is_empty() || matches!(pattern, GraphPattern::Project { .. }) =>
        {
            projected.iter().map(|v| tr.var(v)).collect()
        }
        Form::Describe => {
            let mut v = Vec::new();
            visible(&node, &mut v);
            v
        }
        _ => {
            let mut v = Vec::new();
            visible(&node, &mut v);
            let mut seen = HashSet::new();
            v.retain(|s| {
                seen.insert(*s) && !tr.vars[*s].starts_with("_:") && !tr.vars[*s].starts_with("__")
            });
            v
        }
    };
    Ok(Planned {
        node,
        vars: tr.vars.clone(),
        out,
        form,
        cacheable: !tr.transient_consts,
        base: base.as_ref().map(|b| b.as_str().to_string()),
    })
}

fn variables_of(gp: &GraphPattern) -> Vec<String> {
    match gp {
        GraphPattern::Project { variables, .. } => {
            variables.iter().map(|v| v.as_str().to_string()).collect()
        }
        GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } => variables_of(inner),
        _ => Vec::new(),
    }
}

/// A result as IDs, decoded on demand (rules `api-rows-late`,
/// `api-ffi-rows`). `ASK` is one row with a boolean in column `ask`;
/// `CONSTRUCT` and `DESCRIBE` are rows of `s`, `p`, `o`.
#[derive(Debug, Clone, Default)]
pub struct IdRows {
    pub vars: Vec<String>,
    /// Row-major IDs, `0` for unbound.
    pub rows: Vec<Vec<Id>>,
    /// Terms computed by the query that are not in the dictionary.
    pub transient: HashMap<Id, Term>,
}

impl IdRows {
    /// The term of an ID of this result.
    pub fn decode(
        &self,
        kv: &dyn crate::storage::KvRead,
        dict: &crate::dict::Dict,
        id: Id,
    ) -> Result<Term> {
        match self.transient.get(&id) {
            Some(t) => Ok(t.clone()),
            None => dict.decode(kv, id),
        }
    }
}

/// Runs a planned query, keeping IDs.
pub fn execute_ids(ex: &Exec<'_>, p: &Planned, describe_terms: &[Id]) -> Result<IdRows> {
    match &p.form {
        Form::Select => {
            let rows = ex.run(&p.node, p.vars.len())?;
            Ok(IdRows {
                vars: p.out.iter().map(|s| p.vars[*s].clone()).collect(),
                rows: rows
                    .iter()
                    .map(|r| p.out.iter().map(|s| r[*s]).collect())
                    .collect(),
                transient: ex.transient_terms(),
            })
        }
        _ => {
            let res = execute(ex, p, describe_terms)?;
            let (vars, rows): (Vec<String>, Vec<Vec<Id>>) = match res {
                QueryResult::Boolean(b) => (
                    vec!["ask".into()],
                    vec![vec![ex.encode(&Term::Literal(Literal::boolean(b)))?]],
                ),
                QueryResult::Graph(ts) => (
                    vec!["s".into(), "p".into(), "o".into()],
                    ts.iter()
                        .map(|t| t.iter().map(|x| ex.encode(x)).collect::<Result<Vec<_>>>())
                        .collect::<Result<_>>()?,
                ),
                QueryResult::Solutions { .. } => unreachable!(),
            };
            Ok(IdRows {
                vars,
                rows,
                transient: ex.transient_terms(),
            })
        }
    }
}

/// Runs a planned query and decodes the result.
pub fn execute(ex: &Exec<'_>, p: &Planned, describe_terms: &[Id]) -> Result<QueryResult> {
    let width = p.vars.len();
    ex.set_base_iri(p.base.clone());
    let rows = ex.run(&p.node, width)?;
    match &p.form {
        Form::Select => {
            let mut out = Vec::with_capacity(rows.len());
            for r in &rows {
                let mut row = Vec::with_capacity(p.out.len());
                for s in &p.out {
                    row.push(if r[*s].is_null() {
                        None
                    } else {
                        Some(ex.decode(r[*s])?)
                    });
                }
                out.push(row);
            }
            Ok(QueryResult::Solutions {
                vars: p.out.iter().map(|s| p.vars[*s].clone()).collect(),
                rows: out,
            })
        }
        Form::Ask => Ok(QueryResult::Boolean(!rows.is_empty())),
        Form::Construct(template) => {
            let mut triples = Vec::new();
            let mut seen = HashSet::new();
            for (k, r) in rows.iter().enumerate() {
                for t in template {
                    let var = |v: &spargebra::term::Variable| -> Result<Option<Term>> {
                        Ok(match p.vars.iter().position(|x| x == v.as_str()) {
                            Some(s) if !r[s].is_null() => Some(ex.decode(r[s])?),
                            _ => None,
                        })
                    };
                    let np = |n: &NamedNodePattern| -> Result<Option<Term>> {
                        match n {
                            NamedNodePattern::NamedNode(n) => {
                                Ok(Some(Term::Iri(n.as_str().into())))
                            }
                            NamedNodePattern::Variable(v) => var(v),
                        }
                    };
                    fn term(
                        tp: &TermPattern,
                        k: usize,
                        var: &dyn Fn(&spargebra::term::Variable) -> Result<Option<Term>>,
                        np: &dyn Fn(&NamedNodePattern) -> Result<Option<Term>>,
                    ) -> Result<Option<Term>> {
                        Ok(match tp {
                            TermPattern::NamedNode(n) => Some(Term::Iri(n.as_str().into())),
                            TermPattern::Literal(l) => Some(literal_term(l)),
                            TermPattern::BlankNode(b) => {
                                Some(Term::Blank(format!("{}_{k}", b.as_str())))
                            }
                            TermPattern::Variable(v) => var(v)?,
                            // A triple term template: a well-formed triple or nothing.
                            TermPattern::Triple(tt) => match (
                                term(&tt.subject, k, var, np)?,
                                np(&tt.predicate)?,
                                term(&tt.object, k, var, np)?,
                            ) {
                                (
                                    Some(s @ (Term::Iri(_) | Term::Blank(_))),
                                    Some(p @ Term::Iri(_)),
                                    Some(o),
                                ) => Some(Term::triple(s, p, o)),
                                _ => None,
                            },
                        })
                    }
                    let term = |tp: &TermPattern| term(tp, k, &var, &np);
                    let pred = np(&t.predicate)?;
                    if let (Some(s), Some(pr), Some(o)) =
                        (term(&t.subject)?, pred, term(&t.object)?)
                    {
                        if matches!(s, Term::Literal(_) | Term::Triple(_))
                            || !matches!(pr, Term::Iri(_))
                        {
                            continue;
                        }
                        if seen.insert((s.clone(), pr.clone(), o.clone())) {
                            triples.push([s, pr, o]);
                        }
                    }
                }
            }
            Ok(QueryResult::Graph(triples))
        }
        Form::Describe => {
            let mut nodes: HashSet<Id> = describe_terms.iter().copied().collect();
            for r in &rows {
                for s in &p.out {
                    if !r[*s].is_null() {
                        nodes.insert(r[*s]);
                    }
                }
            }
            let mut triples = Vec::new();
            for n in nodes {
                for m in crate::store::find(ex.kv, Some(n), None, None)? {
                    triples.push([ex.decode(m.s)?, ex.decode(m.p)?, ex.decode(m.o)?]);
                }
            }
            Ok(QueryResult::Graph(triples))
        }
    }
}

/// Parses, plans and runs a query against a view.
pub fn query(ex: &Exec<'_>, text: &str, opts: &QueryOptions) -> Result<QueryResult> {
    let q = parse_query(text, opts.base_iri.as_deref())?;
    let p = plan(ex, &q, opts)?;
    execute(ex, &p, &describe_iris(ex, &q)?)
}

/// IRIs named directly by `DESCRIBE <iri>`.
pub fn describe_iris(ex: &Exec<'_>, q: &Query) -> Result<Vec<Id>> {
    let mut out = Vec::new();
    if let Query::Describe { pattern, .. } = q {
        let mut stack = vec![pattern];
        while let Some(gp) = stack.pop() {
            match gp {
                GraphPattern::Values { bindings, .. } => {
                    for b in bindings {
                        for t in b.iter().flatten() {
                            if let GroundTerm::NamedNode(n) = t {
                                out.push(ex.encode(&Term::Iri(n.as_str().into()))?);
                            }
                        }
                    }
                }
                GraphPattern::Project { inner, .. }
                | GraphPattern::Distinct { inner }
                | GraphPattern::Reduced { inner }
                | GraphPattern::Slice { inner, .. } => stack.push(inner),
                GraphPattern::Join { left, right } => {
                    stack.push(left);
                    stack.push(right);
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

/// Rows of a query for callers that work on IDs (the update path).
pub fn rows(ex: &Exec<'_>, node: &Node, width: usize) -> Result<Vec<Row>> {
    ex.run(node, width)
}

/// Convenience map from variable name to slot.
pub fn slots(vars: &[String]) -> HashMap<String, Slot> {
    vars.iter()
        .enumerate()
        .map(|(i, v)| (v.clone(), i))
        .collect()
}
