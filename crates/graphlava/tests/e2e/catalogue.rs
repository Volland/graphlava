//! Scenarios of the end-to-end catalogue [[21-e2e-tests]] that are not a
//! single requirement's scenario.

use std::collections::BTreeSet;

use graphlava::{
    CoreOp, Database, DeleteMode, Durability, Error, HedgeTypeDecl, Reader, RoleSpec, Term,
    WriteRequest,
};

use crate::fixture::*;

const P: &str = "PREFIX : <http://ex.org/>\nPREFIX meta: <https://graphlava.org/ns#>\n";

fn upd(db: &Database, u: &str) -> graphlava::Result<graphlava::UpdateReport> {
    db.sparql_update(&format!("{P}{u}"), Durability::Ordered)
}

fn sel(db: &Database, q: &str) -> graphlava::QueryResult {
    db.sparql(&format!("{P}{q}"))
        .unwrap_or_else(|e| panic!("{q}: {e}"))
}

fn one(r: &graphlava::QueryResult, var: &str) -> Term {
    let col = r.column(var);
    assert_eq!(col.len(), 1, "{r:?}");
    col[0].clone().expect("bound")
}

// @tg: verifies:: [[21-e2e-tests#Agent memory session]] {level: e2e}
#[test]
fn agent_memory_session() {
    let f = Fixture::new();
    let db = f.open();
    upd(&db, "INSERT DATA { :episode1 meta:in :agent }").unwrap();
    upd(
        &db,
        "INSERT DATA { GRAPH :episode1 { :alice :worksAt :acme {| :confidence 0.9 |} . :alice :age 42 } }",
    )
    .unwrap();
    // annotating the inline :age property promotes it
    upd(
        &db,
        "INSERT DATA { << :alice :age 42 ~ _:r >> :source :doc5 }",
    )
    .unwrap();
    let r_before = one(
        &sel(&db, "SELECT ?r WHERE { << :alice :worksAt :acme ~ ?r >> }"),
        "r",
    );
    let age_id = one(
        &sel(&db, "SELECT ?r WHERE { << :alice :age 42 ~ ?r >> }"),
        "r",
    );
    upd(
        &db,
        "DELETE { :alice :worksAt :acme } INSERT { << :alice :worksAt :globex ~ ?r >> } WHERE { << :alice :worksAt :acme ~ ?r >> }",
    )
    .unwrap();
    let db = f.reopen(db);
    let r = sel(
        &db,
        "SELECT ?o ?c ?r WHERE { GRAPH :agent { << :alice :worksAt ?o ~ ?r >> . ?r :confidence ?c } }",
    );
    assert_eq!(r.rows().len(), 1, "{r:?}");
    assert_eq!(one(&r, "o"), iri("globex"));
    assert_eq!(one(&r, "c"), Term::lit(graphlava::Literal::decimal("0.9")));
    assert_eq!(one(&r, "r"), r_before, "reifier unchanged by the update");
    let src = sel(
        &db,
        "SELECT ?r ?s WHERE { << :alice :age 42 ~ ?r >> . ?r :source ?s }",
    );
    assert_eq!(one(&src, "r"), age_id, ":age statement ID is stable");
    assert_eq!(one(&src, "s"), iri("doc5"));
    assert!(
        sel(
            &db,
            "SELECT ?o WHERE { GRAPH! :agent { :alice :worksAt ?o } }"
        )
        .rows()
        .is_empty()
    );
    assert_invariants(&db);
}

// @tg: verifies:: [[21-e2e-tests#Same answers on every surface]] {level: e2e}
#[test]
fn same_answers_on_every_surface() {
    let f = Fixture::new();
    let db = f.open();
    let mut x: u64 = 42;
    let mut next = |m: u64| {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) % m
    };
    let mut edges = BTreeSet::new();
    while edges.len() < 10_000 {
        edges.insert((next(3000), next(3000)));
    }
    let edges: Vec<(u64, u64)> = edges.into_iter().collect();
    for chunk in edges.chunks(2000) {
        let ops = chunk
            .iter()
            .flat_map(|(a, b)| {
                [
                    CoreOp::Insert {
                        s: iri(&format!("n{a}")).into(),
                        p: iri("knows").into(),
                        o: iri(&format!("n{b}")).into(),
                        graph: None,
                    },
                    CoreOp::Insert {
                        s: iri(&format!("n{a}")).into(),
                        p: iri("name").into(),
                        o: Term::string(format!("node {a}")).into(),
                        graph: None,
                    },
                ]
            })
            .collect();
        db.submit(WriteRequest { ops }, Durability::Relaxed)
            .unwrap();
    }
    // a few annotations
    upd(&db, "INSERT DATA { << :n1 :knows :n2 ~ _:r >> :weight 1 }").ok();
    let db = f.reopen(db);
    let set = |r: graphlava::QueryResult| -> BTreeSet<String> {
        r.rows().iter().map(|row| format!("{:?}", row[0])).collect()
    };
    let rules = "reach(?x, ?y) :- triple(?x, :knows, ?y) .\nreach(?x, ?z) :- reach(?x, ?y), triple(?y, :knows, ?z) .";
    for _ in 0..20 {
        let start = edges[next(edges.len() as u64) as usize].0;
        let sparql = set(sel(
            &db,
            &format!("SELECT DISTINCT ?y WHERE {{ :n{start} :knows+ ?y }}"),
        ));
        let datalog = set(db
            .datalog(&format!(
                "@prefix : <http://ex.org/> .\n{rules}\n?- reach(:n{start}, ?y) ."
            ))
            .unwrap());
        let service = set(sel(
            &db,
            &format!(
                "SELECT ?y WHERE {{ SERVICE <meta:datalog> {{ \"\"\" @prefix : <http://ex.org/> . {rules} \"\"\" :program :inline . :n{start} :reach ?y }} }}"
            ),
        ));
        assert_eq!(sparql, datalog, "start n{start}");
        assert_eq!(sparql, service, "start n{start}");
    }
}

// @tg: verifies:: [[21-e2e-tests#Graph scope and nesting]] {level: e2e}
#[test]
fn graph_scope_and_nesting() {
    let f = Fixture::new();
    let db = f.open();
    upd(
        &db,
        "INSERT DATA { :episode1 meta:in :agent . :episode1 meta:in :project . :project meta:in :agent .
           GRAPH :episode1 { :e1 :p :x . :e2 :p :x }
           GRAPH :project { :pr :p :x }
           GRAPH :agent { :ag :p :x } }",
    )
    .unwrap();
    let rows = sel(
        &db,
        "SELECT ?s WHERE { GRAPH :agent { ?s :p :x } } ORDER BY ?s",
    );
    assert_eq!(
        rows.column("s"),
        ["ag", "e1", "e2", "pr"]
            .iter()
            .map(|n| Some(iri(n)))
            .collect::<Vec<_>>(),
        "each statement of :episode1 once under :agent"
    );
    let cycle = upd(&db, "INSERT DATA { :agent meta:in :episode1 }");
    assert!(matches!(cycle, Err(Error::GraphCycle { .. })), "{cycle:?}");
    let parents = sel(&db, "SELECT ?p WHERE { :agent meta:in ?p }");
    assert!(parents.rows().is_empty(), "nesting unchanged: {parents:?}");
    upd(&db, "DROP GRAPH :project").unwrap();
    // A recursive drop through the core API: :episode1 survives because
    // :agent is still its parent.
    write(&db, |w| {
        w.nest(iri("project"), iri("agent")).unwrap();
        w.nest(iri("episode1"), iri("project")).unwrap();
        w.drop_graph(iri("project"), true).unwrap()
    });
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert!(r.graph(&iri("project")).unwrap().is_none());
    assert!(r.graph(&iri("episode1")).unwrap().is_some());
    drop(r);
    assert_eq!(
        sel(&db, "SELECT ?s WHERE { GRAPH :episode1 { ?s :p :x } }")
            .rows()
            .len(),
        2
    );
    assert_invariants(&db);
}

// @tg: verifies:: [[21-e2e-tests#Cascade and hyperedge guard]] {level: e2e}
#[test]
fn cascade_and_hyperedge_guard() {
    let f = Fixture::new();
    let db = f.open();
    let (s42, s77, s90, e) = write(&db, |w| {
        w.declare_hedge_type(
            iri("Meeting"),
            HedgeTypeDecl {
                roles: vec![RoleSpec::one(iri("agent")), RoleSpec::one(iri("target"))],
                attrs: vec![],
            },
        )
        .unwrap();
        let s42 = w
            .insert(iri("alice"), iri("knows"), iri("bob"))
            .unwrap()
            .unwrap();
        let s77 = w
            .annotate(s42, iri("source"), iri("doc1"))
            .unwrap()
            .unwrap();
        let s90 = w
            .annotate(s77, iri("checkedBy"), iri("carol"))
            .unwrap()
            .unwrap();
        let e = w
            .create_hedge(
                iri("Meeting"),
                None,
                vec![
                    (iri("agent").into(), iri("dan").into()),
                    (iri("target").into(), s42.into()),
                ],
                vec![],
            )
            .unwrap();
        (s42, s77, s90, e)
    });
    let blocked = upd(&db, "DELETE DATA { :alice :knows :bob }");
    assert!(
        matches!(blocked, Err(Error::FillsRole { .. })),
        "{blocked:?}"
    );
    let db = f.reopen(db);
    assert!(
        db.read().unwrap().stmt(s90).unwrap().is_some(),
        "nothing changed"
    );
    let gone = db
        .submit(
            WriteRequest {
                ops: vec![CoreOp::Delete {
                    id: s42,
                    mode: DeleteMode::Detach,
                }],
            },
            Durability::Ordered,
        )
        .unwrap();
    let graphlava::OpResult::Deleted(ids) = &gone.results[0] else {
        panic!()
    };
    let ids: BTreeSet<_> = ids.iter().copied().collect();
    assert_eq!(ids, BTreeSet::from([s42, s77, s90, e]));
    let db = f.reopen(db);
    let r = db.read().unwrap();
    assert!(r.stmt(s42).unwrap().is_none() && r.hedge(e).unwrap().is_none());
    drop(r);
    assert_invariants(&db);
}
