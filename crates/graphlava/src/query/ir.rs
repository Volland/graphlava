//! The shared IR ([[10-ir]]): operators over rows of `u64` IDs, one slot per
//! query variable, `0` meaning unbound (rule `ir-tuple-u64`). SPARQL and
//! Datalog both compile to [`Node`]; only `Project` decodes terms.

use spargebra::algebra::Function;

use crate::id::Id;
use crate::term::Term;

/// A variable slot in a row.
pub type Slot = usize;

/// A position of a pattern: a constant ID or a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pt {
    Const(Id),
    Var(Slot),
}

/// Which graphs a pattern reads ([[11-sparql#Graph semantics]]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphSpec {
    /// No `GRAPH` clause: the union of all graphs (graphlava default).
    Union,
    /// No `GRAPH` clause in standard mode: only the default graph.
    DefaultOnly,
    /// `GRAPH g { … }`: with a constant, the scope of `g` (transitive unless
    /// `direct`); with a variable, each direct graph of the match is bound,
    /// never an ancestor and never the default graph (rule `sparql-graph-var-direct`).
    Graph { g: Pt, direct: bool },
}

/// One triple pattern, optionally binding the statement ID (the reifier,
/// rule `sparql-reifier-is-id`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TriplePat {
    pub s: Pt,
    pub p: Pt,
    pub o: Pt,
    pub id: Option<Pt>,
    pub graph: GraphSpec,
}

/// A property path ([[10-ir#Operators]] `Path`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathExpr {
    Pred(Id),
    Inv(Box<PathExpr>),
    Seq(Box<PathExpr>, Box<PathExpr>),
    Alt(Box<PathExpr>, Box<PathExpr>),
    Star(Box<PathExpr>),
    Plus(Box<PathExpr>),
    Opt(Box<PathExpr>),
    /// `!(p1 | ^p2 …)`: forward excluded, inverse excluded.
    Neg {
        fwd: Vec<Id>,
        inv: Vec<Id>,
    },
}

/// Aggregate functions (`Aggregate` operator).
#[derive(Debug, Clone, PartialEq)]
pub enum AggKind {
    CountAll,
    Count(Expr),
    Sum(Expr),
    Avg(Expr),
    Min(Expr),
    Max(Expr),
    Sample(Expr),
    GroupConcat(Expr, String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Agg {
    pub kind: AggKind,
    pub distinct: bool,
}

/// Expressions of `Filter`, `Extend`, `LeftJoin` conditions and `ORDER BY`.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Const(Term),
    /// A constant that is already an ID (a reifier IRI, a graph).
    ConstId(Id),
    Var(Slot),
    Or(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    SameTerm(Box<Expr>, Box<Expr>),
    Gt(Box<Expr>, Box<Expr>),
    Ge(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    Le(Box<Expr>, Box<Expr>),
    In(Box<Expr>, Vec<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
    Plus(Box<Expr>),
    Minus(Box<Expr>),
    Not(Box<Expr>),
    Exists(Box<Node>),
    Bound(Slot),
    /// The slot holds an ID of this kind tag (`Kind as u8`); with bit 7 set,
    /// any literal. Used by the Datalog `prop` and `role` relations.
    IsKind(Slot, u8),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Coalesce(Vec<Expr>),
    Call(Function, Vec<Expr>),
}

/// Which version of a derived relation a Datalog scan reads (semi-naive
/// evaluation, rule `dl-semi-naive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Version {
    Full,
    Delta,
}

/// The logical operators of [[10-ir#Operators]].
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// One row with nothing bound.
    Unit,
    Empty,
    Triple(TriplePat),
    Path {
        s: Pt,
        path: PathExpr,
        o: Pt,
        graph: GraphSpec,
    },
    /// Binds a graph variable to every named graph (an empty `GRAPH ?g {}`).
    NamedGraphs(Slot),
    /// `GRAPH ?g { P }` evaluated once per named graph: `P`'s graph specs
    /// read the slot `active`, which is replaced by each graph in turn, and
    /// `var` is bound to that graph. Used when `P` holds subqueries,
    /// aggregates, negation or `VALUES`, whose scope a plain graph-variable
    /// scan would leak across graphs.
    PerGraph {
        var: Slot,
        active: Slot,
        /// The named graphs of the dataset (`FROM NAMED`); all when `None`.
        graphs: Option<Vec<Id>>,
        inner: Box<Node>,
    },
    /// n-ary natural join (rule `ir-join-nary`); the executor orders it.
    Join(Vec<Node>),
    /// `OPTIONAL` with its condition inside (rule `ir-leftjoin-scope`).
    LeftJoin(Box<Node>, Box<Node>, Option<Expr>),
    /// SPARQL `MINUS`.
    Minus(Box<Node>, Box<Node>),
    /// Rows of the left side with no compatible row on the right (stratified
    /// negation).
    AntiJoin(Box<Node>, Box<Node>),
    Union(Vec<Node>),
    Filter(Expr, Box<Node>),
    Extend(Box<Node>, Slot, Expr),
    Values(Vec<Slot>, Vec<Vec<Option<Id>>>),
    Aggregate {
        input: Box<Node>,
        keys: Vec<Slot>,
        aggs: Vec<(Slot, Agg)>,
    },
    Distinct(Box<Node>),
    OrderBy(Box<Node>, Vec<(Expr, bool)>),
    Slice(Box<Node>, usize, Option<usize>),
    /// Keep only these slots; the others become unbound.
    Project(Box<Node>, Vec<Slot>),
    /// A scan of a Datalog relation held by the executor.
    Rel {
        name: String,
        args: Vec<Pt>,
        version: Version,
    },
    /// A Datalog program evaluated on demand inside this query
    /// (`SERVICE <meta:datalog>`); `goal` is a relation of the program.
    Datalog {
        program: std::sync::Arc<crate::datalog::eval::Compiled>,
        goal: String,
        args: Vec<Pt>,
    },
}

impl Node {
    /// Whether rows from outside may be passed into this subtree as bindings
    /// without changing its result (sideways information passing). Subtrees
    /// with filters, binds, subqueries or negation see only their own scope,
    /// so they are evaluated on their own and joined afterwards.
    pub fn sideways_safe(&self) -> bool {
        match self {
            Node::Unit
            | Node::Empty
            | Node::Triple(_)
            | Node::Path { .. }
            | Node::NamedGraphs(_)
            | Node::Rel { .. }
            | Node::Datalog { .. } => true,
            Node::Join(v) | Node::Union(v) => v.iter().all(Node::sideways_safe),
            _ => false,
        }
    }

    /// Slots this node can bind.
    pub fn bound_slots(&self, out: &mut Vec<Slot>) {
        let pt = |p: &Pt, out: &mut Vec<Slot>| {
            if let Pt::Var(v) = p {
                out.push(*v);
            }
        };
        match self {
            Node::Unit | Node::Empty => {}
            Node::Triple(t) => {
                for p in [&t.s, &t.p, &t.o] {
                    pt(p, out);
                }
                if let Some(i) = &t.id {
                    pt(i, out);
                }
                if let GraphSpec::Graph { g, .. } = &t.graph {
                    pt(g, out);
                }
            }
            Node::Path { s, o, graph, .. } => {
                pt(s, out);
                pt(o, out);
                if let GraphSpec::Graph { g, .. } = graph {
                    pt(g, out);
                }
            }
            Node::NamedGraphs(g) => out.push(*g),
            Node::PerGraph { var, inner, .. } => {
                out.push(*var);
                inner.bound_slots(out);
            }
            Node::Join(v) | Node::Union(v) => v.iter().for_each(|n| n.bound_slots(out)),
            Node::LeftJoin(a, b, _) => {
                a.bound_slots(out);
                b.bound_slots(out);
            }
            Node::Minus(a, _) | Node::AntiJoin(a, _) | Node::Filter(_, a) => a.bound_slots(out),
            Node::Extend(a, s, _) => {
                a.bound_slots(out);
                out.push(*s);
            }
            Node::Values(v, _) => out.extend(v),
            Node::Aggregate { keys, aggs, .. } => {
                out.extend(keys);
                out.extend(aggs.iter().map(|(s, _)| *s));
            }
            Node::Distinct(a) | Node::OrderBy(a, _) | Node::Slice(a, _, _) => a.bound_slots(out),
            Node::Project(_, v) => out.extend(v),
            Node::Rel { args, .. } | Node::Datalog { args, .. } => {
                args.iter().for_each(|a| pt(a, out))
            }
        }
    }
}

impl Node {
    /// Replaces the graph variable `from` of every graph spec in this subtree
    /// (including `EXISTS` patterns) with `to`.
    pub fn rebind_graph(&mut self, from: Slot, to: Pt) {
        let spec = |g: &mut GraphSpec| {
            if let GraphSpec::Graph { g: Pt::Var(v), .. } = g {
                if *v == from {
                    if let GraphSpec::Graph { g, .. } = g {
                        *g = to;
                    }
                }
            }
        };
        match self {
            Node::Triple(t) => spec(&mut t.graph),
            Node::Path { graph, .. } => spec(graph),
            Node::Join(v) | Node::Union(v) => v.iter_mut().for_each(|n| n.rebind_graph(from, to)),
            Node::LeftJoin(a, b, e) => {
                a.rebind_graph(from, to);
                b.rebind_graph(from, to);
                if let Some(e) = e {
                    e.rebind_graph(from, to);
                }
            }
            Node::Minus(a, b) | Node::AntiJoin(a, b) => {
                a.rebind_graph(from, to);
                b.rebind_graph(from, to);
            }
            Node::Filter(e, a) | Node::Extend(a, _, e) => {
                e.rebind_graph(from, to);
                a.rebind_graph(from, to);
            }
            Node::Aggregate { input, aggs, .. } => {
                input.rebind_graph(from, to);
                for (_, a) in aggs {
                    match &mut a.kind {
                        AggKind::CountAll => {}
                        AggKind::Count(e)
                        | AggKind::Sum(e)
                        | AggKind::Avg(e)
                        | AggKind::Min(e)
                        | AggKind::Max(e)
                        | AggKind::Sample(e)
                        | AggKind::GroupConcat(e, _) => e.rebind_graph(from, to),
                    }
                }
            }
            Node::OrderBy(a, keys) => {
                a.rebind_graph(from, to);
                keys.iter_mut().for_each(|(e, _)| e.rebind_graph(from, to));
            }
            Node::Distinct(a) | Node::Slice(a, _, _) | Node::Project(a, _) => {
                a.rebind_graph(from, to)
            }
            Node::PerGraph { inner, .. } => inner.rebind_graph(from, to),
            Node::Unit
            | Node::Empty
            | Node::NamedGraphs(_)
            | Node::Values(..)
            | Node::Rel { .. }
            | Node::Datalog { .. } => {}
        }
    }
}

impl Expr {
    /// [`Node::rebind_graph`] for the patterns of `EXISTS` inside an expression.
    pub fn rebind_graph(&mut self, from: Slot, to: Pt) {
        match self {
            Expr::Exists(n) => n.rebind_graph(from, to),
            Expr::Or(a, b)
            | Expr::And(a, b)
            | Expr::Eq(a, b)
            | Expr::SameTerm(a, b)
            | Expr::Gt(a, b)
            | Expr::Ge(a, b)
            | Expr::Lt(a, b)
            | Expr::Le(a, b)
            | Expr::Add(a, b)
            | Expr::Sub(a, b)
            | Expr::Mul(a, b)
            | Expr::Div(a, b) => {
                a.rebind_graph(from, to);
                b.rebind_graph(from, to);
            }
            Expr::Plus(a) | Expr::Minus(a) | Expr::Not(a) => a.rebind_graph(from, to),
            Expr::In(a, l) => {
                a.rebind_graph(from, to);
                l.iter_mut().for_each(|x| x.rebind_graph(from, to));
            }
            Expr::If(a, b, c) => {
                a.rebind_graph(from, to);
                b.rebind_graph(from, to);
                c.rebind_graph(from, to);
            }
            Expr::Coalesce(l) | Expr::Call(_, l) => {
                l.iter_mut().for_each(|x| x.rebind_graph(from, to))
            }
            Expr::Const(_)
            | Expr::ConstId(_)
            | Expr::Var(_)
            | Expr::Bound(_)
            | Expr::IsKind(..) => {}
        }
    }

    /// Whether the expression holds an `EXISTS` pattern.
    pub fn has_exists(&self) -> bool {
        match self {
            Expr::Exists(_) => true,
            Expr::Or(a, b)
            | Expr::And(a, b)
            | Expr::Eq(a, b)
            | Expr::SameTerm(a, b)
            | Expr::Gt(a, b)
            | Expr::Ge(a, b)
            | Expr::Lt(a, b)
            | Expr::Le(a, b)
            | Expr::Add(a, b)
            | Expr::Sub(a, b)
            | Expr::Mul(a, b)
            | Expr::Div(a, b) => a.has_exists() || b.has_exists(),
            Expr::Plus(a) | Expr::Minus(a) | Expr::Not(a) => a.has_exists(),
            Expr::In(a, l) => a.has_exists() || l.iter().any(Expr::has_exists),
            Expr::If(a, b, c) => a.has_exists() || b.has_exists() || c.has_exists(),
            Expr::Coalesce(l) | Expr::Call(_, l) => l.iter().any(Expr::has_exists),
            Expr::Const(_)
            | Expr::ConstId(_)
            | Expr::Var(_)
            | Expr::Bound(_)
            | Expr::IsKind(..) => false,
        }
    }
}
