//! The standalone Datalog surface ([[13-datalog]], [[adr:0040]]).

pub mod ast;
pub mod eval;
pub mod parse;

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::id::{Id, Kind};
use crate::query::exec::Exec;
use crate::query::ir::{Node, Pt};
use crate::sparql::QueryResult;
use crate::term::TermRef;

/// A triple an `@assert` rule writes.
pub type Write = [TermRef; 3];

/// Parses, compiles and runs a program over a view. Returns the rows of the
/// first goal (or, without goals, the first `@output`) and the triples
/// `@assert` rules derive. With `allow_writes` false, `@assert` is rejected.
pub fn run_program(
    ex: &Exec<'_>,
    text: &str,
    allow_writes: bool,
) -> Result<(QueryResult, Vec<Write>)> {
    let prog = parse::parse(text, &[])
        .map_err(|e| Error::Invalid(format!("Datalog line {}: {}", e.line, e.message)))?;
    let compiled = eval::compile(ex, &prog, allow_writes)?;
    let (name, vars) = match compiled.outputs.first() {
        Some(o) => o.clone(),
        None if compiled.writes => (String::new(), Vec::new()),
        None => {
            return Err(Error::Invalid(
                "the program has no `?-` goal and no @output".into(),
            ));
        }
    };
    let rels = goal_relations(ex, &compiled, &name)?;
    let empty = BTreeMap::new();
    let mut rows = Vec::new();
    for t in rels.get(&name).unwrap_or(&empty).keys() {
        let mut row = Vec::with_capacity(t.len());
        for id in t {
            row.push(if id.is_null() {
                None
            } else {
                Some(ex.decode(*id)?)
            });
        }
        rows.push(row);
    }
    let mut writes = Vec::new();
    if compiled.writes {
        let full = eval::run(ex, &compiled, &[])?;
        for tr in eval::asserted(ex, &compiled, &full)? {
            let w = |id: Id| -> Result<TermRef> {
                Ok(if id.kind() == Kind::System {
                    TermRef::Term(ex.decode(id)?)
                } else {
                    TermRef::Id(id)
                })
            };
            writes.push([w(tr.s)?, w(tr.p)?, w(tr.o)?]);
        }
    }
    Ok((QueryResult::Solutions { vars, rows }, writes))
}

/// Evaluates the program for one output relation. A goal whose body is one
/// derived atom with constant arguments runs with magic sets
/// (rule `dl-magic-sets`); anything else runs the whole program.
fn goal_relations(ex: &Exec<'_>, prog: &eval::Compiled, name: &str) -> Result<eval::Relations> {
    if name.is_empty() {
        return Ok(eval::Relations::new());
    }
    let rule = prog.rules.iter().find(|r| r.head == name);
    if let Some(r) = rule {
        if let Node::Rel {
            name: inner, args, ..
        } = &r.body
        {
            let adorn: Vec<bool> = args.iter().map(|a| matches!(a, Pt::Const(_))).collect();
            if let Some((mprog, seed, goal_ad)) = eval::magic(prog, inner, &adorn) {
                let seeds: Vec<Id> = args
                    .iter()
                    .filter_map(|a| if let Pt::Const(c) = a { Some(*c) } else { None })
                    .collect();
                let rels = eval::run(ex, &mprog, &[(seed, vec![seeds])])?;
                // project the adorned answers onto the goal's head
                let mut out = eval::Relations::new();
                let tuples = rels.get(&goal_ad).cloned().unwrap_or_default();
                let mut goal = BTreeMap::new();
                for t in tuples.keys() {
                    let mut row = vec![Id(0); ex_width(r)];
                    let mut ok = true;
                    for (a, v) in args.iter().zip(t) {
                        if let Pt::Var(s) = a {
                            if !row[*s].is_null() && row[*s] != *v {
                                ok = false;
                            }
                            row[*s] = *v;
                        }
                    }
                    if !ok {
                        continue;
                    }
                    let head: Vec<Id> = r
                        .head_args
                        .iter()
                        .map(|h| match h {
                            eval::HeadArg::Pt(Pt::Var(s)) => row[*s],
                            eval::HeadArg::Pt(Pt::Const(c)) => *c,
                            eval::HeadArg::Agg(..) => Id(0),
                        })
                        .collect();
                    goal.insert(head, ());
                }
                out.insert(name.to_string(), goal);
                return Ok(out);
            }
        }
    }
    eval::run(ex, prog, &[])
}

fn ex_width(r: &eval::CRule) -> usize {
    r.nvars
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, OpenOptions, Reader};
    use crate::term::Term;

    /// Rule `dl-magic-sets`: a goal binding the first argument derives only
    /// the tuples reachable from it, not the whole closure.
    #[test]
    fn single_atom_goal_uses_magic_sets() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("m.glv"), OpenOptions::default()).unwrap();
        let mut w = db.write(crate::Durability::Ordered).unwrap();
        for (a, b) in [("a", "b"), ("b", "c"), ("x", "y"), ("y", "z"), ("z", "x")] {
            w.insert(
                Term::iri(format!("http://e/{a}")),
                Term::iri("http://e/k"),
                Term::iri(format!("http://e/{b}")),
            )
            .unwrap();
        }
        w.commit().unwrap();
        let r = db.read().unwrap();
        let ex = Exec::new(r.kv(), r.dict());
        let prog = parse::parse(
            "@prefix : <http://e/> .\nreach(?x, ?y) :- triple(?x, :k, ?y) .\nreach(?x, ?z) :- reach(?x, ?y), triple(?y, :k, ?z) .\n?- reach(:a, ?y) .",
            &[],
        )
        .unwrap();
        let compiled = eval::compile(&ex, &prog, false).unwrap();
        let rels = goal_relations(&ex, &compiled, &compiled.outputs[0].0).unwrap();
        let derived: usize = rels.values().map(|v| v.len()).sum();
        // full closure has 2 + 1 + 9 = 12 reach tuples; from :a only 2
        assert_eq!(rels[&compiled.outputs[0].0].len(), 2);
        assert!(derived <= 2, "only the goal's answers are kept: {derived}");
    }
}
