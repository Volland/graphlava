//! Abstract syntax of a Datalog program ([[13-datalog#Surface syntax]],
//! [[adr:0040]]). Produced by [`crate::datalog::parse`]; prefixes are already
//! resolved, so every IRI is absolute.

use crate::term::Literal;

/// A whole program: rules, facts, outputs and goals in source order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Program {
    pub rules: Vec<Rule>,
    /// `@output p .`
    pub outputs: Vec<String>,
    /// `?- body .` goals, in order. Each compiles to an `@output` of a fresh
    /// predicate whose columns are the goal's variables in first-occurrence order.
    pub goals: Vec<Goal>,
}

/// `head :- body .` A fact is a rule with an empty body and ground head.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub head: Atom,
    pub body: Vec<BodyItem>,
    /// `@assert`: the head writes base statements ([[adr:0040]]).
    pub assert: bool,
    /// `@materialized` / `@on_demand` annotation, if any.
    pub mode: Option<RuleMode>,
    /// 1-based source line of the head, for error messages.
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleMode {
    OnDemand,
    Materialized { force: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Goal {
    pub body: Vec<BodyItem>,
    pub line: usize,
}

/// A predicate applied to arguments. `pred` is either a plain name (`reach`,
/// `triple`, `prop`, `role`, `in`, `in_star`) or, for the shorthand
/// `:knows(?x, ?y)`, the absolute IRI of the predicate with `iri_pred` set.
#[derive(Debug, Clone, PartialEq)]
pub struct Atom {
    pub pred: String,
    pub iri_pred: bool,
    pub args: Vec<Arg>,
}

/// An argument in a head or body atom. Aggregates appear only in heads.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Term(TermAst),
    Aggregate { func: AggFunc, var: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFunc {
    Count,
    Sum,
    Min,
    Max,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TermAst {
    /// A named variable (`?x`, or an identifier starting with an uppercase
    /// letter such as `X`), without the `?`.
    Var(String),
    /// `_` or `?_`: a fresh anonymous variable each time it appears.
    Anon,
    Iri(String),
    Literal(Literal),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BodyItem {
    Pos(Atom),
    /// `~atom` or `not atom`.
    Neg(Atom),
    /// `lhs op rhs` with op one of `= != < <= > >=`. `?v = expr` with `?v`
    /// unbound elsewhere is an assignment; the evaluator decides.
    Cmp {
        op: CmpOp,
        lhs: Expr,
        rhs: Expr,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Term(TermAst),
    Bin {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Neg(Box<Expr>),
    /// A built-in call by its Nemo name, upper-cased (`STRLEN`, `CONCAT`, ...).
    Call {
        name: String,
        args: Vec<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}
