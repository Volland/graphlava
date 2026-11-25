//! Scenarios of `specs/datalog-query/spec.md`.

use graphlava::{Database, Durability, Reader, Term};

use crate::fixture::*;

const PRE: &str = "@prefix : <http://ex.org/> .\n";
const REACH: &str =
    "reach(X,Y) :- triple(X, :knows, Y).\nreach(X,Z) :- reach(X,Y), triple(Y, :knows, Z).\n";

fn chain(db: &Database) {
    db.sparql_update(
        "PREFIX : <http://ex.org/> INSERT DATA { :a :knows :b . :b :knows :c . :c :knows :a . :c :knows :d . :x :knows :y . }",
        Durability::Ordered,
    )
    .unwrap();
}

fn sorted(r: &graphlava::QueryResult) -> Vec<Vec<Option<Term>>> {
    let mut v = r.rows().to_vec();
    v.sort_by_key(|row| format!("{row:?}"));
    v
}

// @tg: verifies:: [[openspec:datalog-query#Standalone Datalog programs#Transitive closure query]] {level: e2e}
#[test]
fn transitive_closure_query() {
    let f = Fixture::new();
    let db = f.open();
    chain(&db);
    let db = f.reopen(db);
    let r = db
        .datalog(&format!("{PRE}{REACH}?- reach(:a, Y)."))
        .unwrap();
    assert_eq!(
        sorted(&r),
        ["a", "b", "c", "d"]
            .iter()
            .map(|x| vec![Some(iri(x))])
            .collect::<Vec<_>>(),
        "every node reachable from :a, once"
    );
}

// @tg: verifies:: [[openspec:datalog-query#Standalone Datalog programs#Asserted head writes base data]] {level: e2e}
#[test]
fn asserted_head_writes_base_data() {
    let f = Fixture::new();
    let db = f.open();
    db.sparql_update(
        "PREFIX : <http://ex.org/> INSERT DATA { :ann :worksAt :acme . :bob :worksAt :acme . :cid :worksAt :other . }",
        Durability::Ordered,
    )
    .unwrap();
    let prog = format!(
        "{PRE}@assert triple(X, :colleague, Y) :- triple(X, :worksAt, O), triple(Y, :worksAt, O), X != Y."
    );
    // read transaction: rejected
    let err = db.datalog(&prog).unwrap_err();
    assert!(err.to_string().contains("@assert"), "{err}");
    db.datalog_update(&prog, Durability::Ordered).unwrap();
    let db = f.reopen(db);
    let r = db
        .sparql("PREFIX : <http://ex.org/> SELECT ?x ?y ?r WHERE { << ?x :colleague ?y ~ ?r >> } ORDER BY ?x")
        .unwrap();
    assert_eq!(r.rows().len(), 2, "{r:?}");
    assert!(
        r.column("r").iter().all(|x| x.is_some()),
        "the statements have IDs"
    );
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:datalog-query#Nemo-compatible syntax#Goal shorthand]] {level: e2e}
#[test]
fn goal_shorthand() {
    let f = Fixture::new();
    let db = f.open();
    chain(&db);
    let goal = db
        .datalog(&format!("{PRE}{REACH}?- reach(:a, Y)."))
        .unwrap();
    let output = db
        .datalog(&format!(
            "{PRE}{REACH}from_a(Y) :- reach(:a, Y).\n@output from_a ."
        ))
        .unwrap();
    assert_eq!(sorted(&goal), sorted(&output));
    // Nemo spelling: ?variables and ~negation
    let nemo = db
        .datalog(&format!(
            "{PRE}reach(?x, ?y) :- :knows(?x, ?y) .\nreach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .\n?- reach(:a, ?y) ."
        ))
        .unwrap();
    assert_eq!(sorted(&nemo), sorted(&goal));
}

// @tg: verifies:: [[openspec:datalog-query#Nemo-compatible syntax#Unsupported construct]] {level: e2e}
#[test]
fn unsupported_construct() {
    let f = Fixture::new();
    let db = f.open();
    let e = db
        .datalog("@import edge :- csv{resource=\"edges.csv\"} .\n?- edge(?x, ?y) .")
        .unwrap_err();
    assert!(e.to_string().contains("@import"), "{e}");
    let e = db
        .datalog(&format!("{PRE}p(?x, !y) :- q(?x) ."))
        .unwrap_err();
    assert!(e.to_string().contains("existential"), "{e}");
}

// @tg: verifies:: [[openspec:datalog-query#Stratified negation and aggregation#Unstratifiable program]] {level: e2e}
#[test]
fn unstratifiable_program() {
    let f = Fixture::new();
    let db = f.open();
    let e = db
        .datalog(&format!("{PRE}q(:a).\np(X) :- q(X), not p(X).\n?- p(X)."))
        .unwrap_err();
    let m = e.to_string();
    assert!(m.contains("not stratifiable") && m.contains('p'), "{m}");
    // a stratified program with negation and aggregation runs
    chain(&db);
    let ok = db
        .datalog(&format!(
            "{PRE}{REACH}node(X) :- triple(X, :knows, Y).\nunreached(X) :- node(X), not reach(:a, X).\nn(#count(X)) :- reach(:a, X).\n?- unreached(X)."
        ))
        .unwrap();
    assert_eq!(sorted(&ok), vec![vec![Some(iri("x"))]]);
    let n = db
        .datalog(&format!(
            "{PRE}{REACH}n(#count(X)) :- reach(:a, X).\n?- n(C)."
        ))
        .unwrap();
    assert_eq!(n.rows(), &[vec![Some(Term::integer(4))]]);
    // unsafe rule names the variable
    let e = db
        .datalog(&format!("{PRE}p(X, Z) :- q(X).\nq(:a).\n?- p(X, Z)."))
        .unwrap_err();
    assert!(e.to_string().contains("`Z`"), "{e}");
}

// @tg: verifies:: [[openspec:datalog-query#On-demand evaluation#Results follow the data]] {level: e2e}
#[test]
fn results_follow_the_data() {
    let f = Fixture::new();
    let db = f.open();
    let id = write(&db, |w| {
        w.insert(iri("a"), iri("knows"), iri("b")).unwrap().unwrap()
    });
    let q = format!("{PRE}{REACH}?- reach(:a, Y).");
    assert_eq!(sorted(&db.datalog(&q).unwrap()), vec![vec![Some(iri("b"))]]);
    write(&db, |w| w.update(id, iri("c"), None).unwrap());
    assert_eq!(sorted(&db.datalog(&q).unwrap()), vec![vec![Some(iri("c"))]]);
    // nothing derived was stored
    assert!(db.read().unwrap().lookup(&iri("reach")).unwrap().is_none());
}

// @tg: verifies:: [[openspec:datalog-query#Metagraph access from Datalog#Rule over annotations]] {level: e2e}
#[test]
fn rule_over_annotations() {
    let f = Fixture::new();
    let db = f.open();
    db.sparql_update(
        "PREFIX : <http://ex.org/> INSERT DATA { :a :worksAt :acme {| :confidence 0.9 |} . :b :worksAt :acme {| :confidence 0.5 |} . }",
        Durability::Ordered,
    )
    .unwrap();
    let r = db
        .datalog(&format!(
            "{PRE}sure(S, X, P, O) :- triple(X, P, O, S), prop(S, :confidence, C), C > 0.8.\n?- sure(S, X, P, O)."
        ))
        .unwrap();
    assert_eq!(r.rows().len(), 1, "{r:?}");
    let row = &r.rows()[0];
    assert_eq!(row[1], Some(iri("a")));
    let Some(Term::Iri(sid)) = &row[0] else {
        panic!()
    };
    assert!(sid.starts_with("https://graphlava.org/ns/stmt/"));
    // the same relation SPARQL sees
    let s = db
        .sparql("PREFIX : <http://ex.org/> SELECT ?r WHERE { << :a :worksAt :acme ~ ?r >> }")
        .unwrap();
    assert_eq!(s.column("r")[0], row[0]);
    // graph membership and hyperedge roles as relations
    write(&db, |w| {
        w.nest(iri("ep1"), iri("agent")).unwrap();
        w.insert_in(iri("ep1"), iri("m"), iri("p"), iri("n"))
            .unwrap();
    });
    let g = db
        .datalog(&format!(
            "{PRE}scoped(S) :- triple(:m, :p, :n, S), in_star(S, :agent).\n?- scoped(S)."
        ))
        .unwrap();
    assert_eq!(g.rows().len(), 1, "{g:?}");
    let r2 = db.read().unwrap();
    let _ = r2.lookup(&iri("m")).unwrap();
}
