//! Datalog compilation and evaluation ([[13-datalog]]): rules become IR
//! nodes over the same relations SPARQL sees, programs are checked for
//! safety (rule `dl-safety`) and stratification (rule `dl-stratified`),
//! rewritten with magic sets when a goal binds arguments (rule
//! `dl-magic-sets`), and run stratum by stratum with semi-naive evaluation
//! (rule `dl-semi-naive`). Nothing derived is stored (rule
//! `dl-default-on-demand`); `@assert` heads are returned as writes.

// @tg: implements:: [[openspec:datalog-query#Standalone Datalog programs]]
// @tg: implements:: [[openspec:datalog-query#Stratified negation and aggregation]]
// @tg: implements:: [[openspec:datalog-query#On-demand evaluation]]
// @tg: implements:: [[openspec:datalog-query#Metagraph access from Datalog]]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use spargebra::algebra::Function;

use super::ast::{self, AggFunc, Arg, BodyItem, CmpOp, TermAst};
use crate::error::{Error, Result};
use crate::graph;
use crate::id::{Id, Kind};
use crate::query::exec::{Exec, Row};
use crate::query::ir::*;
use crate::tables::mk;
use crate::term::{Term, Triple};

fn err<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(m.into()))
}

/// A compiled rule: its body as an IR node over `nvars` slots, and the head.
#[derive(Debug, Clone, PartialEq)]
pub struct CRule {
    pub head: String,
    pub head_args: Vec<HeadArg>,
    pub body: Node,
    /// Derived predicates read positively by the body, with the body
    /// position of their `Rel` node (for semi-naive deltas).
    pub derived_pos: Vec<String>,
    pub nvars: usize,
    pub assert: bool,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HeadArg {
    Pt(Pt),
    Agg(AggFunc, Slot),
}

/// A compiled program.
#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    pub rules: Vec<CRule>,
    /// Strata in evaluation order; each is a set of head predicates.
    pub strata: Vec<Vec<String>>,
    pub arity: HashMap<String, usize>,
    /// Result relations: goals (`?-`, with their variable names) then `@output`s.
    pub outputs: Vec<(String, Vec<String>)>,
    /// Whether the program writes (`@assert`).
    pub writes: bool,
}

/// Store relations visible to programs ([[13-datalog#Surface syntax]]).
const STORE_RELS: [&str; 5] = ["triple", "prop", "role", "in", "in_star"];

struct RuleCtx<'e, 'a> {
    ex: &'e Exec<'a>,
    vars: Vec<String>,
}

impl RuleCtx<'_, '_> {
    fn var(&mut self, name: &str) -> Slot {
        match self.vars.iter().position(|v| v == name) {
            Some(i) => i,
            None => {
                self.vars.push(name.to_string());
                self.vars.len() - 1
            }
        }
    }
    fn fresh(&mut self) -> Slot {
        let n = format!("_anon{}", self.vars.len());
        self.var(&n)
    }
    fn pt(&mut self, t: &TermAst) -> Result<Pt> {
        Ok(match t {
            TermAst::Var(v) => Pt::Var(self.var(v)),
            TermAst::Anon => Pt::Var(self.fresh()),
            TermAst::Iri(i) => Pt::Const(self.ex.encode(&Term::Iri(i.clone()))?),
            TermAst::Literal(l) => Pt::Const(self.ex.encode(&Term::Literal(l.clone()))?),
        })
    }
    fn expr(&mut self, e: &ast::Expr) -> Result<Expr> {
        Ok(match e {
            ast::Expr::Term(TermAst::Var(v)) => Expr::Var(self.var(v)),
            ast::Expr::Term(TermAst::Anon) => return err("`_` cannot appear in an expression"),
            ast::Expr::Term(TermAst::Iri(i)) => Expr::Const(Term::Iri(i.clone())),
            ast::Expr::Term(TermAst::Literal(l)) => Expr::Const(Term::Literal(l.clone())),
            ast::Expr::Bin { op, lhs, rhs } => {
                let (a, b) = (Box::new(self.expr(lhs)?), Box::new(self.expr(rhs)?));
                match op {
                    ast::BinOp::Add => Expr::Add(a, b),
                    ast::BinOp::Sub => Expr::Sub(a, b),
                    ast::BinOp::Mul => Expr::Mul(a, b),
                    ast::BinOp::Div => Expr::Div(a, b),
                }
            }
            ast::Expr::Neg(a) => Expr::Minus(Box::new(self.expr(a)?)),
            ast::Expr::Call { name, args } => {
                let args = args
                    .iter()
                    .map(|a| self.expr(a))
                    .collect::<Result<Vec<_>>>()?;
                Expr::Call(builtin(name)?, args)
            }
        })
    }
}

/// Nemo built-in names mapped to the shared function library.
fn builtin(name: &str) -> Result<Function> {
    Ok(match name {
        "STRLEN" => Function::StrLen,
        "CONCAT" => Function::Concat,
        "UCASE" | "UPPERCASE" => Function::UCase,
        "LCASE" | "LOWERCASE" => Function::LCase,
        "ABS" => Function::Abs,
        "CEIL" => Function::Ceil,
        "FLOOR" => Function::Floor,
        "ROUND" => Function::Round,
        "STR" | "FULLSTR" => Function::Str,
        "CONTAINS" => Function::Contains,
        "STRSTARTS" => Function::StrStarts,
        "STRENDS" => Function::StrEnds,
        "STRBEFORE" => Function::StrBefore,
        "STRAFTER" => Function::StrAfter,
        "SUBSTR" => Function::SubStr,
        "REGEX" => Function::Regex,
        "LANG" => Function::Lang,
        "DATATYPE" => Function::Datatype,
        "ISIRI" => Function::IsIri,
        "ISNUMERIC" => Function::IsNumeric,
        "ISSTRING" | "ISLITERAL" => Function::IsLiteral,
        other => return err(format!("unsupported built-in `{other}`")),
    })
}

fn rel_pred(a: &ast::Atom) -> String {
    if a.iri_pred {
        format!("<{}>", a.pred)
    } else {
        a.pred.clone()
    }
}

/// Compiles a parsed program against a view (constants are resolved to IDs).
pub fn compile(ex: &Exec<'_>, prog: &ast::Program, allow_writes: bool) -> Result<Compiled> {
    let mut rules: Vec<ast::Rule> = prog.rules.clone();
    let mut outputs = Vec::new();
    for (k, g) in prog.goals.iter().enumerate() {
        // `?- body .` is `@output` of a fresh predicate over the goal's
        // variables in first-occurrence order ([[adr:0040]]).
        let mut vars = Vec::new();
        let mut note = |t: &TermAst| {
            if let TermAst::Var(v) = t {
                if !vars.contains(v) {
                    vars.push(v.clone());
                }
            }
        };
        for item in &g.body {
            match item {
                BodyItem::Pos(a) | BodyItem::Neg(a) => a.args.iter().for_each(|x| {
                    if let Arg::Term(t) = x {
                        note(t)
                    }
                }),
                BodyItem::Cmp { .. } => {}
            }
        }
        let name = format!("__goal{k}");
        rules.push(ast::Rule {
            head: ast::Atom {
                pred: name.clone(),
                iri_pred: false,
                args: vars
                    .iter()
                    .map(|v| Arg::Term(TermAst::Var(v.clone())))
                    .collect(),
            },
            body: g.body.clone(),
            assert: false,
            mode: None,
            line: g.line,
        });
        outputs.push((name, vars));
    }
    let heads: HashSet<String> = rules
        .iter()
        .filter(|r| !r.assert)
        .map(|r| rel_pred(&r.head))
        .collect();
    for o in &prog.outputs {
        if !heads.contains(o) {
            return err(format!("@output `{o}` names no rule head"));
        }
        let arity = rules
            .iter()
            .find(|r| &rel_pred(&r.head) == o)
            .map_or(0, |r| r.head.args.len());
        outputs.push((o.clone(), (0..arity).map(|i| format!("{o}_{i}")).collect()));
    }
    // Modes: one per head predicate (rule `dl-annotation-scope`).
    let mut modes: HashMap<String, Option<ast::RuleMode>> = HashMap::new();
    for r in &rules {
        let h = rel_pred(&r.head);
        match (modes.get(&h), r.mode) {
            (Some(Some(a)), Some(b)) if *a != b => {
                return err(format!(
                    "line {}: conflicting annotations on rules of `{h}`",
                    r.line
                ));
            }
            (Some(Some(_)), None) => {}
            _ => {
                modes.insert(h, r.mode);
            }
        }
    }
    if let Some((h, _)) = modes
        .iter()
        .find(|(_, m)| matches!(m, Some(ast::RuleMode::Materialized { .. })))
    {
        return err(format!(
            "`{h}` is @materialized: materialized rules are installed through Admin (v1), not run inline"
        ));
    }
    let mut arity: HashMap<String, usize> = HashMap::new();
    let mut compiled = Vec::new();
    let mut writes = false;
    for r in &rules {
        let h = rel_pred(&r.head);
        if r.assert {
            if !allow_writes {
                return err(format!(
                    "line {}: @assert writes base data and needs a write transaction",
                    r.line
                ));
            }
            writes = true;
        } else {
            if STORE_RELS.contains(&h.as_str()) || r.head.iri_pred {
                return err(format!(
                    "line {}: `{h}` is a store relation; mark the rule @assert to write it",
                    r.line
                ));
            }
            match arity.get(&h) {
                Some(n) if *n != r.head.args.len() => {
                    return err(format!("line {}: `{h}` used with two arities", r.line));
                }
                _ => {
                    arity.insert(h.clone(), r.head.args.len());
                }
            }
        }
    }
    for r in &rules {
        compiled.push(compile_rule(ex, r, &heads)?);
    }
    let strata = stratify(&compiled)?;
    Ok(Compiled {
        rules: compiled,
        strata,
        arity,
        outputs,
        writes,
    })
}

fn compile_rule(ex: &Exec<'_>, r: &ast::Rule, heads: &HashSet<String>) -> Result<CRule> {
    let mut cx = RuleCtx {
        ex,
        vars: Vec::new(),
    };
    let mut positives: Vec<Node> = Vec::new();
    let mut negatives: Vec<Node> = Vec::new();
    let mut tests: Vec<(CmpOp, ast::Expr, ast::Expr)> = Vec::new();
    let mut derived_pos = Vec::new();
    for item in &r.body {
        match item {
            BodyItem::Pos(a) => {
                let n = atom_node(&mut cx, a, heads, r.line)?;
                if let Node::Rel { name, .. } = &n {
                    derived_pos.push(name.clone());
                }
                positives.push(n);
            }
            BodyItem::Neg(a) => negatives.push(atom_node(&mut cx, a, heads, r.line)?),
            BodyItem::Cmp { op, lhs, rhs } => tests.push((*op, lhs.clone(), rhs.clone())),
        }
    }
    // Variables bound by positive atoms.
    let mut bound: Vec<Slot> = Vec::new();
    for n in &positives {
        n.bound_slots(&mut bound);
    }
    let mut body = match positives.len() {
        0 => Node::Unit,
        1 => positives.pop().expect("one"),
        _ => Node::Join(positives),
    };
    // Comparisons: `?v = expr` with `?v` not bound is an assignment.
    for (op, lhs, rhs) in tests {
        let assign = match (&op, &lhs) {
            (CmpOp::Eq, ast::Expr::Term(TermAst::Var(v))) if !bound.contains(&cx.var(v)) => {
                Some(cx.var(v))
            }
            _ => None,
        };
        let rhs_e = cx.expr(&rhs)?;
        let mut used = Vec::new();
        expr_vars(&rhs_e, &mut used);
        if let Some(slot) = assign {
            check_safe(&cx, &used, &bound, r.line)?;
            body = Node::Extend(Box::new(body), slot, rhs_e);
            bound.push(slot);
            continue;
        }
        let lhs_e = cx.expr(&lhs)?;
        expr_vars(&lhs_e, &mut used);
        check_safe(&cx, &used, &bound, r.line)?;
        let (a, b) = (Box::new(lhs_e), Box::new(rhs_e));
        let e = match op {
            CmpOp::Eq => Expr::Eq(a, b),
            CmpOp::Ne => Expr::Not(Box::new(Expr::Eq(a, b))),
            CmpOp::Lt => Expr::Lt(a, b),
            CmpOp::Le => Expr::Le(a, b),
            CmpOp::Gt => Expr::Gt(a, b),
            CmpOp::Ge => Expr::Ge(a, b),
        };
        body = Node::Filter(e, Box::new(body));
    }
    for n in negatives {
        let mut used = Vec::new();
        n.bound_slots(&mut used);
        // anonymous variables in negated atoms are existential inside it
        let named: Vec<Slot> = used
            .into_iter()
            .filter(|s| !cx.vars[*s].starts_with("_anon"))
            .collect();
        check_safe(&cx, &named, &bound, r.line)?;
        body = Node::AntiJoin(Box::new(body), Box::new(n));
    }
    let mut head_args = Vec::new();
    for a in &r.head.args {
        head_args.push(match a {
            Arg::Term(t) => {
                let p = cx.pt(t)?;
                if let Pt::Var(s) = p {
                    check_safe(&cx, &[s], &bound, r.line)?;
                }
                HeadArg::Pt(p)
            }
            Arg::Aggregate { func, var } => {
                let s = cx.var(var);
                check_safe(&cx, &[s], &bound, r.line)?;
                HeadArg::Agg(*func, s)
            }
        });
    }
    Ok(CRule {
        head: rel_pred(&r.head),
        head_args,
        body,
        derived_pos,
        nvars: cx.vars.len(),
        assert: r.assert,
        line: r.line,
    })
}

/// Rule `dl-safety`: every variable used outside positive atoms is bound by one.
fn check_safe(cx: &RuleCtx<'_, '_>, used: &[Slot], bound: &[Slot], line: usize) -> Result<()> {
    for s in used {
        if !bound.contains(s) {
            return err(format!(
                "line {line}: unsafe rule: variable `{}` does not appear in a positive body atom",
                cx.vars[*s]
            ));
        }
    }
    Ok(())
}

fn expr_vars(e: &Expr, out: &mut Vec<Slot>) {
    match e {
        Expr::Var(s) | Expr::Bound(s) => out.push(*s),
        Expr::Const(_) | Expr::ConstId(_) | Expr::Exists(_) | Expr::IsKind(..) => {}
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
            expr_vars(a, out);
            expr_vars(b, out);
        }
        Expr::Plus(a) | Expr::Minus(a) | Expr::Not(a) => expr_vars(a, out),
        Expr::In(a, l) => {
            expr_vars(a, out);
            l.iter().for_each(|x| expr_vars(x, out));
        }
        Expr::If(a, b, c) => {
            expr_vars(a, out);
            expr_vars(b, out);
            expr_vars(c, out);
        }
        Expr::Coalesce(l) | Expr::Call(_, l) => l.iter().for_each(|x| expr_vars(x, out)),
    }
}

/// The IR node of one body atom: a store relation, an IRI predicate, or a
/// derived relation.
fn atom_node(
    cx: &mut RuleCtx<'_, '_>,
    a: &ast::Atom,
    heads: &HashSet<String>,
    line: usize,
) -> Result<Node> {
    let mut args = Vec::new();
    for x in &a.args {
        match x {
            Arg::Term(t) => args.push(cx.pt(t)?),
            Arg::Aggregate { .. } => return err(format!("line {line}: aggregate in a rule body")),
        }
    }
    let name = rel_pred(a);
    let n = args.len();
    let triple = |s: Pt, p: Pt, o: Pt, id: Option<Pt>| {
        Node::Triple(TriplePat {
            s,
            p,
            o,
            id,
            graph: GraphSpec::Union,
        })
    };
    if heads.contains(&name) {
        return Ok(Node::Rel {
            name,
            args,
            version: Version::Full,
        });
    }
    if a.iri_pred {
        if n != 2 {
            return err(format!("line {line}: `{}(…)` takes two arguments", a.pred));
        }
        let p = cx.pt(&TermAst::Iri(a.pred.clone()))?;
        return Ok(triple(args[0], p, args[1], None));
    }
    let kind_filter = |slot: Pt, e: fn(Slot) -> Expr, node: Node| match slot {
        Pt::Var(s) => Node::Filter(e(s), Box::new(node)),
        Pt::Const(_) => node,
    };
    match (name.as_str(), n) {
        ("triple", 3) => Ok(triple(args[0], args[1], args[2], None)),
        ("triple", 4) => Ok(triple(args[0], args[1], args[2], Some(args[3]))),
        // Inline properties: literal-valued statements.
        ("prop", 3 | 4) => Ok(kind_filter(
            args[2],
            |s| Expr::IsKind(s, Kind::InlineLiteral as u8 | 0x80),
            triple(args[0], args[1], args[2], args.get(3).copied()),
        )),
        // Hyperedge roles.
        ("role", 3) => Ok(kind_filter(
            args[0],
            |s| Expr::IsKind(s, Kind::Hyperedge as u8),
            triple(args[0], args[1], args[2], None),
        )),
        ("in" | "in_star", 2) => {
            // a constant graph is named by its IRI: use its graph ID
            if let Pt::Const(c) = args[1] {
                if c.kind() == Kind::Iri {
                    args[1] = Pt::Const(match graph::graph_by_name_id(cx.ex.kv, c)? {
                        Some(g) => g,
                        None => cx
                            .ex
                            .encode(&Term::Iri(format!("urn:graphlava:no-graph:{c}")))?,
                    });
                }
            }
            let in_id = graph::sys_id(cx.ex.kv, mk::SYS_IN)?
                .ok_or_else(|| Error::Invalid("this file has no `meta:in` IRI".into()))?;
            if name == "in" {
                return Ok(triple(args[0], Pt::Const(in_id), args[1], None));
            }
            let d = Pt::Var(cx.fresh());
            Ok(Node::Join(vec![
                triple(args[0], Pt::Const(in_id), d, None),
                Node::Path {
                    s: d,
                    path: PathExpr::Star(Box::new(PathExpr::Pred(in_id))),
                    o: args[1],
                    graph: GraphSpec::Union,
                },
            ]))
        }
        (r, _) if STORE_RELS.contains(&r) => {
            err(format!("line {line}: `{r}` used with {n} arguments"))
        }
        (other, _) => err(format!("line {line}: unknown predicate `{other}`")),
    }
}

/// Splits head predicates into strata (Tarjan SCCs in dependency order),
/// rejecting a cycle through negation or aggregation (rule `dl-stratified`).
fn stratify(rules: &[CRule]) -> Result<Vec<Vec<String>>> {
    let preds: Vec<String> = {
        let mut v: Vec<String> = rules
            .iter()
            .filter(|r| !r.assert)
            .map(|r| r.head.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let idx: HashMap<&str, usize> = preds
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_str(), i))
        .collect();
    // edges: body pred -> head pred, with a "negative" flag
    let mut edges: Vec<Vec<(usize, bool)>> = vec![Vec::new(); preds.len()];
    let all_deps = |r: &CRule, edges: &mut Vec<Vec<(usize, bool)>>| {
        let Some(&h) = idx.get(r.head.as_str()) else {
            return;
        };
        let agg = r.head_args.iter().any(|a| matches!(a, HeadArg::Agg(..)));
        let visit = |n: &Node, neg: bool, edges: &mut Vec<Vec<(usize, bool)>>| {
            let mut stack = vec![(n, neg)];
            while let Some((n, neg)) = stack.pop() {
                match n {
                    Node::Rel { name, .. } => {
                        if let Some(&b) = idx.get(name.as_str()) {
                            edges[b].push((h, neg || agg));
                        }
                    }
                    Node::AntiJoin(a, b) => {
                        stack.push((a, neg));
                        stack.push((b, true));
                    }
                    Node::Join(v) | Node::Union(v) => v.iter().for_each(|x| stack.push((x, neg))),
                    Node::Filter(_, a) | Node::Extend(a, _, _) => stack.push((a, neg)),
                    _ => {}
                }
            }
        };
        visit(&r.body, false, edges);
    };
    for r in rules {
        all_deps(r, &mut edges);
    }
    // Tarjan
    let n = preds.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0; n];
    let mut on = vec![false; n];
    let mut stack = Vec::new();
    let mut comps: Vec<Vec<usize>> = Vec::new();
    let mut counter = 0;
    #[allow(clippy::too_many_arguments)]
    fn strong(
        v: usize,
        edges: &[Vec<(usize, bool)>],
        index: &mut [usize],
        low: &mut [usize],
        on: &mut [bool],
        stack: &mut Vec<usize>,
        comps: &mut Vec<Vec<usize>>,
        counter: &mut usize,
    ) {
        index[v] = *counter;
        low[v] = *counter;
        *counter += 1;
        stack.push(v);
        on[v] = true;
        for (w, _) in &edges[v] {
            if index[*w] == usize::MAX {
                strong(*w, edges, index, low, on, stack, comps, counter);
                low[v] = low[v].min(low[*w]);
            } else if on[*w] {
                low[v] = low[v].min(index[*w]);
            }
        }
        if low[v] == index[v] {
            let mut c = Vec::new();
            while let Some(w) = stack.pop() {
                on[w] = false;
                c.push(w);
                if w == v {
                    break;
                }
            }
            comps.push(c);
        }
    }
    for v in 0..n {
        if index[v] == usize::MAX {
            strong(
                v,
                &edges,
                &mut index,
                &mut low,
                &mut on,
                &mut stack,
                &mut comps,
                &mut counter,
            );
        }
    }
    for c in &comps {
        for v in c {
            for (w, neg) in &edges[*v] {
                if *neg && c.contains(w) {
                    let names: Vec<&str> = c.iter().map(|i| preds[*i].as_str()).collect();
                    return err(format!(
                        "program is not stratifiable: `{}` depends on `{}` through negation or aggregation in the cycle {{{}}}",
                        preds[*w],
                        preds[*v],
                        names.join(", ")
                    ));
                }
            }
        }
    }
    // Tarjan emits SCCs in reverse topological order of the edge direction
    // (dependencies point body -> head, so heads come first): reverse.
    comps.reverse();
    Ok(comps
        .into_iter()
        .map(|c| c.into_iter().map(|i| preds[i].clone()).collect())
        .collect())
}

// ---- magic sets (rule `dl-magic-sets`) ----------------------------------------

/// Rewrites `prog` for a goal on `goal` with bound positions `adorn`:
/// adorned copies of the derived predicates reachable from the goal, magic
/// predicates holding the bindings that reach them, and the seed relation
/// `magic_<goal>_<adorn>` that the caller fills. Returns `None` when the
/// rewrite does not apply (no bound argument, or negation or aggregation
/// among the reachable rules, which magic sets would make unstratified).
pub fn magic(prog: &Compiled, goal: &str, adorn: &[bool]) -> Option<(Compiled, String, String)> {
    if !adorn.iter().any(|b| *b) {
        return None;
    }
    let derived: HashSet<&str> = prog
        .rules
        .iter()
        .filter(|r| !r.assert)
        .map(|r| r.head.as_str())
        .collect();
    let ad = |a: &[bool]| {
        a.iter()
            .map(|b| if *b { 'b' } else { 'f' })
            .collect::<String>()
    };
    let mut out_rules = Vec::new();
    let mut todo = vec![(goal.to_string(), adorn.to_vec())];
    let mut done: HashSet<(String, Vec<bool>)> = HashSet::new();
    while let Some((p, a)) = todo.pop() {
        if !done.insert((p.clone(), a.clone())) {
            continue;
        }
        let pa = format!("{p}^{}", ad(&a));
        let magic_pa = format!("magic_{pa}");
        for r in prog.rules.iter().filter(|r| r.head == p && !r.assert) {
            if r.head_args.iter().any(|h| matches!(h, HeadArg::Agg(..))) {
                return None;
            }
            // Flatten the body into an ordered list of conjuncts.
            let conj = conjuncts(&r.body)?;
            let mut nvars = r.nvars;
            // bound head args feed the magic predicate
            let magic_args: Vec<Pt> = r
                .head_args
                .iter()
                .zip(&a)
                .filter(|(_, b)| **b)
                .map(|(h, _)| match h {
                    HeadArg::Pt(p) => *p,
                    HeadArg::Agg(..) => unreachable!(),
                })
                .collect();
            let mut bound: HashSet<Slot> = magic_args
                .iter()
                .filter_map(|p| if let Pt::Var(s) = p { Some(*s) } else { None })
                .collect();
            // An all-free adornment has no magic predicate: the rule runs unrestricted.
            let (mut body, mut derived_pos) = if a.iter().any(|b| *b) {
                (
                    vec![Node::Rel {
                        name: magic_pa.clone(),
                        args: magic_args.clone(),
                        version: Version::Full,
                    }],
                    vec![magic_pa.clone()],
                )
            } else {
                (Vec::new(), Vec::new())
            };
            for c in conj {
                match c {
                    Conj::Node(Node::Rel { name, args, .. }) if derived.contains(name.as_str()) => {
                        let qa: Vec<bool> = args
                            .iter()
                            .map(|x| match x {
                                Pt::Const(_) => true,
                                Pt::Var(s) => bound.contains(s),
                            })
                            .collect();
                        let qname = format!("{name}^{}", ad(&qa));
                        if qa.iter().any(|b| *b) {
                            // magic_q(bound args) :- magic_p(..), conjuncts so far
                            let margs: Vec<Pt> = args
                                .iter()
                                .zip(&qa)
                                .filter(|(_, b)| **b)
                                .map(|(x, _)| *x)
                                .collect();
                            out_rules.push(CRule {
                                head: format!("magic_{qname}"),
                                head_args: margs.iter().map(|x| HeadArg::Pt(*x)).collect(),
                                body: Node::Join(body.clone()),
                                derived_pos: derived_pos.clone(),
                                nvars,
                                assert: false,
                                line: r.line,
                            });
                        }
                        todo.push((name.clone(), qa));
                        derived_pos.push(qname.clone());
                        body.push(Node::Rel {
                            name: qname,
                            args: args.clone(),
                            version: Version::Full,
                        });
                        for x in args {
                            if let Pt::Var(s) = x {
                                bound.insert(*s);
                            }
                        }
                    }
                    Conj::Node(n) => {
                        let mut vs = Vec::new();
                        n.bound_slots(&mut vs);
                        bound.extend(vs);
                        body.push(n.clone());
                    }
                    Conj::Filter(e) => {
                        let inner = Node::Join(std::mem::take(&mut body));
                        body = vec![Node::Filter(e.clone(), Box::new(inner))];
                    }
                    Conj::Extend(s, e) => {
                        let inner = Node::Join(std::mem::take(&mut body));
                        body = vec![Node::Extend(Box::new(inner), s, e.clone())];
                        bound.insert(s);
                    }
                }
            }
            nvars = nvars.max(r.nvars);
            out_rules.push(CRule {
                head: pa.clone(),
                head_args: r.head_args.clone(),
                body: Node::Join(body),
                derived_pos,
                nvars,
                assert: false,
                line: r.line,
            });
        }
    }
    let seed = format!("magic_{goal}^{}", ad(adorn));
    let goal_ad = format!("{goal}^{}", ad(adorn));
    let strata = stratify(&out_rules).ok()?;
    let mut arity = prog.arity.clone();
    for r in &out_rules {
        arity.insert(r.head.clone(), r.head_args.len());
    }
    arity.insert(seed.clone(), adorn.iter().filter(|b| **b).count());
    Some((
        Compiled {
            rules: out_rules,
            strata,
            arity,
            outputs: Vec::new(),
            writes: false,
        },
        seed,
        goal_ad,
    ))
}

enum Conj<'a> {
    Node(&'a Node),
    Filter(&'a Expr),
    Extend(Slot, &'a Expr),
}

/// The body as conjuncts in order; `None` if it contains negation.
fn conjuncts(n: &Node) -> Option<Vec<Conj<'_>>> {
    match n {
        Node::Join(v) => {
            let mut out = Vec::new();
            for x in v {
                out.extend(conjuncts(x)?);
            }
            Some(out)
        }
        Node::Filter(e, a) => {
            let mut out = conjuncts(a)?;
            out.push(Conj::Filter(e));
            Some(out)
        }
        Node::Extend(a, s, e) => {
            let mut out = conjuncts(a)?;
            out.push(Conj::Extend(*s, e));
            Some(out)
        }
        Node::Unit => Some(Vec::new()),
        Node::AntiJoin(..) => None,
        other => Some(vec![Conj::Node(other)]),
    }
}

// ---- evaluation ------------------------------------------------------------------

/// Derived relations after a fixpoint, as sorted sets of tuples.
pub type Relations = HashMap<String, BTreeMap<Vec<Id>, ()>>;

/// Runs `prog` to its fixpoint with `seeds` as extra base tuples.
pub fn run(ex: &Exec<'_>, prog: &Compiled, seeds: &[(String, Vec<Vec<Id>>)]) -> Result<Relations> {
    let mut full: HashMap<String, HashSet<Vec<Id>>> = HashMap::new();
    for (name, rows) in seeds {
        full.entry(name.clone())
            .or_default()
            .extend(rows.iter().cloned());
    }
    let publish = |full: &HashMap<String, HashSet<Vec<Id>>>,
                   delta: &HashMap<String, Vec<Vec<Id>>>| {
        let mut rels = ex.rels.borrow_mut();
        rels.clear();
        for (k, v) in full {
            rels.insert(
                k.clone(),
                (
                    v.iter().cloned().collect(),
                    delta.get(k).cloned().unwrap_or_default(),
                ),
            );
        }
        for (k, v) in delta {
            rels.entry(k.clone())
                .or_insert_with(|| (Vec::new(), Vec::new()))
                .1 = v.clone();
        }
    };
    for stratum in &prog.strata {
        let rules: Vec<&CRule> = prog
            .rules
            .iter()
            .filter(|r| !r.assert && stratum.contains(&r.head))
            .collect();
        let recursive = |r: &CRule| r.derived_pos.iter().any(|p| stratum.contains(p));
        // First round: every rule over the full relations.
        publish(&full, &HashMap::new());
        let mut delta: HashMap<String, Vec<Vec<Id>>> = HashMap::new();
        for r in &rules {
            for t in fire(ex, r, None)? {
                if full.entry(r.head.clone()).or_default().insert(t.clone()) {
                    delta.entry(r.head.clone()).or_default().push(t);
                }
            }
        }
        // Semi-naive rounds: each recursive rule once per recursive body atom,
        // that atom reading only the previous round's delta.
        let mut rounds = 0;
        while delta.values().any(|v| !v.is_empty()) {
            rounds += 1;
            if rounds > 1_000_000 {
                return err("recursion depth budget exceeded");
            }
            publish(&full, &delta);
            let mut next: HashMap<String, Vec<Vec<Id>>> = HashMap::new();
            for r in rules.iter().filter(|r| recursive(r)) {
                let positions: Vec<usize> = r
                    .derived_pos
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| {
                        stratum.contains(p) && delta.get(*p).is_some_and(|d| !d.is_empty())
                    })
                    .map(|(i, _)| i)
                    .collect();
                for k in positions {
                    for t in fire(ex, r, Some(k))? {
                        if full.entry(r.head.clone()).or_default().insert(t.clone()) {
                            next.entry(r.head.clone()).or_default().push(t);
                        }
                    }
                }
            }
            delta = next;
            let total: usize = full.values().map(HashSet::len).sum();
            if total > ex.budget.max_rows {
                return err(format!(
                    "Datalog evaluation exceeded its row budget of {} in stratum {{{}}}",
                    ex.budget.max_rows,
                    stratum.join(", ")
                ));
            }
        }
    }
    Ok(full
        .into_iter()
        .map(|(k, v)| (k, v.into_iter().map(|t| (t, ())).collect()))
        .collect())
}

/// The `k`-th derived atom of the body (counting `Rel` nodes left to right)
/// reads the delta.
fn with_delta(n: &Node, k: usize, seen: &mut usize) -> Node {
    match n {
        Node::Rel { name, args, .. } => {
            let v = if *seen == k {
                Version::Delta
            } else {
                Version::Full
            };
            *seen += 1;
            Node::Rel {
                name: name.clone(),
                args: args.clone(),
                version: v,
            }
        }
        Node::Join(v) => Node::Join(v.iter().map(|x| with_delta(x, k, seen)).collect()),
        Node::Filter(e, a) => Node::Filter(e.clone(), Box::new(with_delta(a, k, seen))),
        Node::Extend(a, s, e) => Node::Extend(Box::new(with_delta(a, k, seen)), *s, e.clone()),
        Node::AntiJoin(a, b) => {
            let a = with_delta(a, k, seen);
            // negated atoms read lower, complete strata: never a delta
            let mut never = usize::MAX;
            Node::AntiJoin(
                Box::new(a),
                Box::new(with_delta(b, usize::MAX - 1, &mut never)),
            )
        }
        other => other.clone(),
    }
}

/// Evaluates one rule, returning head tuples (aggregated when the head has
/// aggregates).
fn fire(ex: &Exec<'_>, r: &CRule, delta_at: Option<usize>) -> Result<Vec<Vec<Id>>> {
    let body = match delta_at {
        Some(k) => with_delta(&r.body, k, &mut 0),
        None => r.body.clone(),
    };
    let rows = ex.run(&body, r.nvars)?;
    let aggs: Vec<(usize, AggFunc, Slot)> = r
        .head_args
        .iter()
        .enumerate()
        .filter_map(|(i, h)| match h {
            HeadArg::Agg(f, s) => Some((i, *f, *s)),
            _ => None,
        })
        .collect();
    let val = |p: &Pt, row: &Row| match p {
        Pt::Const(c) => *c,
        Pt::Var(s) => row[*s],
    };
    if aggs.is_empty() {
        return Ok(rows
            .iter()
            .map(|row| {
                r.head_args
                    .iter()
                    .map(|h| match h {
                        HeadArg::Pt(p) => val(p, row),
                        HeadArg::Agg(..) => unreachable!(),
                    })
                    .collect()
            })
            .collect());
    }
    // group by the non-aggregate head arguments; aggregate over distinct values
    let mut groups: BTreeMap<Vec<Id>, Vec<Row>> = BTreeMap::new();
    for row in rows {
        let key: Vec<Id> = r
            .head_args
            .iter()
            .filter_map(|h| match h {
                HeadArg::Pt(p) => Some(val(p, &row)),
                _ => None,
            })
            .collect();
        groups.entry(key).or_default().push(row);
    }
    let mut out = Vec::new();
    for (key, members) in groups {
        let mut t = Vec::with_capacity(r.head_args.len());
        let mut ki = key.into_iter();
        for (i, h) in r.head_args.iter().enumerate() {
            match h {
                HeadArg::Pt(_) => t.push(ki.next().expect("key")),
                HeadArg::Agg(..) => {
                    let (_, f, s) = aggs.iter().find(|(j, _, _)| *j == i).expect("agg");
                    let kind = match f {
                        AggFunc::Count => AggKind::Count(Expr::Var(*s)),
                        AggFunc::Sum => AggKind::Sum(Expr::Var(*s)),
                        AggFunc::Min => AggKind::Min(Expr::Var(*s)),
                        AggFunc::Max => AggKind::Max(Expr::Var(*s)),
                    };
                    let agg = Agg {
                        kind,
                        distinct: true,
                    };
                    match ex.aggregate(&agg, &members) {
                        Ok(Some(term)) => t.push(ex.encode(&term)?),
                        _ => t.push(Id(0)),
                    }
                }
            }
        }
        if !t.iter().any(|x| x.is_null()) {
            out.push(t);
        }
    }
    Ok(out)
}

/// The tuples `@assert` rules derive, as triples to write (after every
/// stratum has been evaluated: rule `ir-write-after-read`).
pub fn asserted(ex: &Exec<'_>, prog: &Compiled, rels: &Relations) -> Result<Vec<Triple>> {
    {
        let mut m = ex.rels.borrow_mut();
        m.clear();
        for (k, v) in rels {
            m.insert(k.clone(), (v.keys().cloned().collect(), Vec::new()));
        }
    }
    let mut out = Vec::new();
    for r in prog.rules.iter().filter(|r| r.assert) {
        for t in fire(ex, r, None)? {
            let triple = if r.head.starts_with('<') {
                let p = ex.encode(&Term::Iri(r.head[1..r.head.len() - 1].to_string()))?;
                (t.len() == 2).then(|| Triple {
                    s: t[0],
                    p,
                    o: t[1],
                })
            } else if r.head == "triple" && t.len() == 3 {
                Some(Triple {
                    s: t[0],
                    p: t[1],
                    o: t[2],
                })
            } else {
                None
            };
            match triple {
                Some(tr) => out.push(tr),
                None => {
                    return err(format!(
                        "line {}: an @assert head is `triple(s, p, o)` or `:p(s, o)`",
                        r.line
                    ));
                }
            }
        }
    }
    Ok(out)
}

/// `Node::Datalog`: evaluates `goal` for every input row, passing the bound
/// arguments in as magic-set seeds, and joins the answers back.
pub fn eval_goal(
    ex: &Exec<'_>,
    prog: &Arc<Compiled>,
    goal: &str,
    args: &[Pt],
    input: &[Row],
) -> Result<Vec<(usize, Row)>> {
    let saved = ex.rels.replace(HashMap::new());
    let result = (|| {
        let val = |p: &Pt, r: &Row| match p {
            Pt::Const(c) => Some(*c),
            Pt::Var(s) if !r[*s].is_null() => Some(r[*s]),
            Pt::Var(_) => None,
        };
        // Group input rows by their adornment.
        let mut by_adorn: BTreeMap<Vec<bool>, Vec<usize>> = BTreeMap::new();
        for (i, r) in input.iter().enumerate() {
            by_adorn
                .entry(args.iter().map(|a| val(a, r).is_some()).collect())
                .or_default()
                .push(i);
        }
        let mut out = Vec::new();
        for (adorn, rows) in by_adorn {
            let (rels, rel_name) = match magic(prog, goal, &adorn) {
                Some((mprog, seed, goal_ad)) => {
                    let seeds: Vec<Vec<Id>> = rows
                        .iter()
                        .map(|i| args.iter().filter_map(|a| val(a, &input[*i])).collect())
                        .collect();
                    (run(ex, &mprog, &[(seed, seeds)])?, goal_ad)
                }
                None => (run(ex, prog, &[])?, goal.to_string()),
            };
            let empty = BTreeMap::new();
            let tuples = rels.get(&rel_name).unwrap_or(&empty);
            for i in rows {
                let r = &input[i];
                'f: for t in tuples.keys() {
                    let mut nr = r.clone();
                    for (a, v) in args.iter().zip(t) {
                        match a {
                            Pt::Const(c) => {
                                if c != v {
                                    continue 'f;
                                }
                            }
                            Pt::Var(s) => {
                                if nr[*s].is_null() {
                                    nr[*s] = *v;
                                } else if nr[*s] != *v {
                                    continue 'f;
                                }
                            }
                        }
                    }
                    out.push((i, nr));
                }
            }
        }
        Ok(out)
    })();
    ex.rels.replace(saved);
    result
}
