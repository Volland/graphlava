//! Scenarios of `specs/sparql-query/spec.md`.

use graphlava::id::DEFAULT_GRAPH;
use graphlava::{Database, Durability, Reader, Term};

use crate::fixture::*;

const P: &str = "PREFIX : <http://ex.org/>\n";

fn upd(db: &Database, u: &str) -> graphlava::Result<graphlava::UpdateReport> {
    db.sparql_update(&format!("{P}{u}"), Durability::Ordered)
}

fn sel(db: &Database, q: &str) -> graphlava::QueryResult {
    db.sparql(&format!("{P}{q}"))
        .unwrap_or_else(|e| panic!("{q}: {e}"))
}

fn dec(v: f64) -> Term {
    Term::lit(graphlava::Literal::decimal(format!("{v}")))
}

// @tg: verifies:: [[openspec:sparql-query#SPARQL 1.2 query forms#Optional match]] {level: e2e}
#[test]
fn optional_match() {
    let f = Fixture::new();
    let db = f.open();
    upd(
        &db,
        r#"INSERT DATA { :p1 a :Person ; :email "p1@ex.org" . :p2 a :Person . }"#,
    )
    .unwrap();
    let db = f.reopen(db);
    let r = sel(
        &db,
        "SELECT ?p ?e WHERE { ?p a :Person OPTIONAL { ?p :email ?e } } ORDER BY ?p",
    );
    assert_eq!(
        r.rows(),
        &[
            vec![Some(iri("p1")), Some(Term::string("p1@ex.org"))],
            vec![Some(iri("p2")), None],
        ]
    );
    // the other query forms run too
    assert_eq!(
        sel(&db, "ASK { :p1 a :Person }"),
        graphlava::QueryResult::Boolean(true)
    );
    let n = sel(&db, "SELECT (COUNT(?p) AS ?n) WHERE { ?p a :Person }");
    assert_eq!(n.rows()[0][0], Some(Term::integer(2)));
    let g = sel(&db, "CONSTRUCT { ?p :known true } WHERE { ?p a :Person }");
    let graphlava::QueryResult::Graph(ts) = g else {
        panic!()
    };
    assert_eq!(ts.len(), 2);
    let u = sel(
        &db,
        "SELECT ?x WHERE { { :p1 :email ?x } UNION { BIND(:none AS ?x) } } ORDER BY ?x",
    );
    assert_eq!(u.rows().len(), 2);
    let m = sel(
        &db,
        "SELECT ?p WHERE { ?p a :Person MINUS { ?p :email ?e } }",
    );
    assert_eq!(m.column("p"), vec![Some(iri("p2"))]);
    let s = sel(
        &db,
        "SELECT ?p WHERE { ?p a :Person } ORDER BY DESC(?p) LIMIT 1 OFFSET 0",
    );
    assert_eq!(s.column("p"), vec![Some(iri("p2"))]);
    let v = sel(
        &db,
        "SELECT ?p WHERE { VALUES ?p { :p1 :p3 } ?p a :Person }",
    );
    assert_eq!(v.column("p"), vec![Some(iri("p1"))]);
    let sub = sel(
        &db,
        "SELECT ?n WHERE { { SELECT (COUNT(*) AS ?n) WHERE { ?p a :Person } } }",
    );
    assert_eq!(sub.column("n"), vec![Some(Term::integer(2))]);
    let f2 = sel(
        &db,
        r#"SELECT ?p WHERE { ?p :email ?e FILTER(STRSTARTS(?e, "p1")) }"#,
    );
    assert_eq!(f2.column("p"), vec![Some(iri("p1"))]);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:sparql-query#SPARQL update#Delete-insert is atomic]] {level: e2e}
#[test]
fn delete_insert_is_atomic() {
    let f = Fixture::new();
    let db = f.open();
    upd(&db, r#"INSERT DATA { :a :name "Ann" . :a :knows :b . }"#).unwrap();
    // The insert template puts a literal in subject position: the insert part fails.
    let r = upd(
        &db,
        "DELETE { :a :knows ?x } INSERT { ?n :p :q } WHERE { :a :knows ?x . :a :name ?n }",
    );
    assert!(r.is_err(), "{r:?}");
    let db = f.reopen(db);
    assert_eq!(
        sel(&db, "SELECT ?x WHERE { :a :knows ?x }").column("x"),
        vec![Some(iri("b"))],
        "nothing it deleted is gone"
    );
    // A successful DELETE/INSERT applies both parts.
    upd(
        &db,
        "DELETE { :a :knows ?x } INSERT { :a :likes ?x } WHERE { :a :knows ?x }",
    )
    .unwrap();
    assert!(
        sel(&db, "SELECT ?x WHERE { :a :knows ?x }")
            .rows()
            .is_empty()
    );
    assert_eq!(sel(&db, "SELECT ?x WHERE { :a :likes ?x }").rows().len(), 1);
    // CREATE / DROP / CLEAR
    upd(
        &db,
        "CREATE GRAPH :g ; INSERT DATA { GRAPH :g { :x :y :z } }",
    )
    .unwrap();
    assert_eq!(
        sel(&db, "SELECT * WHERE { GRAPH :g { ?s :y ?o } }")
            .rows()
            .len(),
        1
    );
    upd(&db, "CLEAR GRAPH :g").unwrap();
    assert!(
        sel(&db, "SELECT * WHERE { GRAPH :g { ?s :y ?o } }")
            .rows()
            .is_empty()
    );
    upd(&db, "DROP GRAPH :g").unwrap();
    assert!(upd(&db, "DROP GRAPH :g").is_err());
    upd(&db, "DROP SILENT GRAPH :g").unwrap();
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:sparql-query#SPARQL update#Writes without GRAPH go to the default graph]] {level: e2e}
#[test]
fn writes_without_graph_go_to_the_default_graph() {
    let f = Fixture::new();
    let db = f.open();
    upd(&db, "INSERT DATA { :a :p :b }").unwrap();
    let db = f.reopen(db);
    let r = db.read().unwrap();
    let (a, p, b) = (
        r.lookup(&iri("a")).unwrap(),
        r.lookup(&iri("p")).unwrap(),
        r.lookup(&iri("b")).unwrap(),
    );
    let m = r.find(a, p, b).unwrap();
    let id = m.iter().find_map(|m| m.id).unwrap();
    assert_eq!(r.graphs_of(id).unwrap(), vec![DEFAULT_GRAPH]);
    assert!(
        !r.graphs_of(id)
            .unwrap()
            .contains(&graphlava::id::SYSTEM_GRAPH)
    );
    drop(r);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:sparql-query#Reifiers are statement IDs#Annotate and query back]] {level: e2e}
#[test]
fn annotate_and_query_back() {
    let f = Fixture::new();
    let db = f.open();
    upd(
        &db,
        "INSERT DATA { :alice :worksAt :acme {| :confidence 0.9 |} }",
    )
    .unwrap();
    let db = f.reopen(db);
    let r = sel(
        &db,
        "SELECT ?c WHERE { << :alice :worksAt :acme ~ ?r >> . ?r :confidence ?c }",
    );
    assert_eq!(r.column("c"), vec![Some(dec(0.9))]);
    // one statement: the annotation is on it, not on a second reifier
    let n = sel(
        &db,
        "SELECT (COUNT(*) AS ?n) WHERE { :alice :worksAt :acme }",
    );
    assert_eq!(n.column("n"), vec![Some(Term::integer(1))]);
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:sparql-query#Reifiers are statement IDs#Reifier round-trips as an IRI]] {level: e2e}
#[test]
fn reifier_round_trips_as_an_iri() {
    let f = Fixture::new();
    let db = f.open();
    upd(
        &db,
        "INSERT DATA { :alice :worksAt :acme {| :confidence 0.9 |} }",
    )
    .unwrap();
    let r = sel(&db, "SELECT ?r WHERE { << :alice :worksAt :acme ~ ?r >> }");
    let Some(Term::Iri(reifier)) = r.column("r")[0].clone() else {
        panic!("{r:?}")
    };
    assert!(
        reifier.starts_with("https://graphlava.org/ns/stmt/"),
        "{reifier}"
    );
    let id: u64 = reifier.rsplit('/').next().unwrap().parse().unwrap();
    let db = f.reopen(db);
    let c = sel(
        &db,
        &format!("SELECT ?c WHERE {{ <{reifier}> :confidence ?c }}"),
    );
    assert_eq!(c.column("c"), vec![Some(dec(0.9))]);
    // the IRI names the statement the core API sees
    let t = db.read().unwrap().stmt(graphlava::Id(id)).unwrap().unwrap();
    assert_eq!(db.read().unwrap().decode(t.o).unwrap(), iri("acme"));
}

// @tg: verifies:: [[openspec:sparql-query#Reifiers are statement IDs#Second reifier is an alias]] {level: e2e}
#[test]
fn second_reifier_is_an_alias() {
    let f = Fixture::new();
    let db = f.open();
    upd(
        &db,
        "INSERT DATA { :alice :worksAt :acme {| :confidence 0.9 |} }",
    )
    .unwrap();
    upd(
        &db,
        "INSERT DATA { << :alice :worksAt :acme ~ :r2 >> :source :hr . }",
    )
    .unwrap();
    let db = f.reopen(db);
    let via_alias = sel(&db, "SELECT ?p ?o WHERE { :r2 ?p ?o } ORDER BY ?p");
    let via_id = sel(
        &db,
        "SELECT ?p ?o WHERE { << :alice :worksAt :acme ~ ?r >> . ?r ?p ?o } ORDER BY ?p",
    );
    let ann = |res: &graphlava::QueryResult| -> Vec<Vec<Option<Term>>> {
        res.rows()
            .iter()
            .filter(|r| r[0] != Some(Term::iri(graphlava::term::META_IN)))
            .cloned()
            .collect()
    };
    assert_eq!(ann(&via_alias), ann(&via_id));
    assert_eq!(ann(&via_id).len(), 2, "both annotations: {via_id:?}");
    let n = sel(
        &db,
        "SELECT (COUNT(*) AS ?n) WHERE { << :alice :worksAt :acme ~ ?r >> }",
    );
    assert_eq!(
        n.column("n"),
        vec![Some(Term::integer(1))],
        "no second statement"
    );
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:sparql-query#Reifiers are statement IDs#Unasserted triple is not matched]] {level: e2e}
#[test]
fn unasserted_triple_is_not_matched() {
    let f = Fixture::new();
    let db = f.open();
    upd(
        &db,
        "INSERT DATA { :alice :worksAt :acme . :bob :claims << :alice :worksAt :globex >> . }",
    )
    .unwrap();
    let db = f.reopen(db);
    assert_eq!(
        sel(&db, "SELECT ?o WHERE { :alice :worksAt ?o }").column("o"),
        vec![Some(iri("acme"))]
    );
    let r = sel(
        &db,
        "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> SELECT ?r WHERE { :bob :claims ?r . ?r rdf:reifies <<( :alice :worksAt :globex )>> }",
    );
    assert_eq!(r.rows().len(), 1, "{r:?}");
    let Some(Term::Iri(reifier)) = r.column("r")[0].clone() else {
        panic!()
    };
    let id = graphlava::Id(reifier.rsplit('/').next().unwrap().parse().unwrap());
    assert!(db.read().unwrap().is_unasserted(id).unwrap());
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:sparql-query#Reifiers are statement IDs#Unasserted triple is not matched]] {level: e2e}
#[test]
fn triple_terms_match_their_statements() {
    let f = Fixture::new();
    let db = f.open();
    upd(
        &db,
        "INSERT DATA { :alice :worksAt :acme . :bob :claims <<( :alice :worksAt :globex )>> . :carol :claims <<( :alice :worksAt :acme )>> . }",
    )
    .unwrap();
    let db = f.reopen(db);
    let globex = Term::triple(iri("alice"), iri("worksAt"), iri("globex"));
    // A stored triple term is still unasserted.
    assert_eq!(
        sel(&db, "SELECT ?o WHERE { :alice :worksAt ?o }").column("o"),
        vec![Some(iri("acme"))]
    );
    // VALUES and TRIPLE() match the statement that stores the triple term.
    assert_eq!(
        sel(
            &db,
            "SELECT ?s ?t WHERE { VALUES ?t { <<( :alice :worksAt :globex )>> } ?s :claims ?t }"
        )
        .rows(),
        &[vec![Some(iri("bob")), Some(globex.clone())]]
    );
    assert_eq!(
        sel(
            &db,
            "SELECT ?s WHERE { BIND(TRIPLE(:alice, :worksAt, :acme) AS ?t) ?s :claims ?t }"
        )
        .column("s"),
        vec![Some(iri("carol"))]
    );
    // graphlava mode keeps the reifier IRI; comparing it with a triple term
    // builds the triple term.
    let r = sel(
        &db,
        "SELECT ?r ?sub WHERE { :bob :claims ?r FILTER(?r = <<( :alice :worksAt :globex )>>) BIND(SUBJECT(<<( :alice :worksAt :globex )>>) AS ?sub) }",
    );
    assert_eq!(r.rows().len(), 1, "{r:?}");
    assert!(
        matches!(&r.rows()[0][0], Some(Term::Iri(i)) if i.starts_with(graphlava::term::STMT_IRI_PREFIX)),
        "{r:?}"
    );
    assert_eq!(r.column("sub"), vec![Some(iri("alice"))]);
    // ORDER BY puts triple terms after literals.
    assert_eq!(
        sel(
            &db,
            "SELECT ?v WHERE { VALUES ?v { <<( :a :b :c )>> \"x\" :i } } ORDER BY ?v"
        )
        .column("v"),
        vec![
            Some(iri("i")),
            Some(Term::string("x")),
            Some(Term::triple(iri("a"), iri("b"), iri("c")))
        ]
    );
    assert_invariants(&db);
}

// @tg: verifies:: [[openspec:sparql-query#In-place update through the same reifier#Change employer keeps annotation]] {level: e2e}
#[test]
fn change_employer_keeps_annotation() {
    let f = Fixture::new();
    let db = f.open();
    upd(
        &db,
        "INSERT DATA { :alice :worksAt :acme {| :confidence 0.9 |} }",
    )
    .unwrap();
    let before = sel(&db, "SELECT ?r WHERE { << :alice :worksAt :acme ~ ?r >> }").column("r");
    let rep = upd(
        &db,
        "DELETE { :alice :worksAt :acme } INSERT { << :alice :worksAt :globex ~ ?r >> } WHERE { << :alice :worksAt :acme ~ ?r >> }",
    )
    .unwrap();
    assert_eq!(rep.updated_in_place, 1);
    let db = f.reopen(db);
    let after = sel(
        &db,
        "SELECT ?r ?c WHERE { << :alice :worksAt :globex ~ ?r >> . ?r :confidence ?c }",
    );
    assert_eq!(
        after.column("r"),
        before,
        "?r still identifies the statement"
    );
    assert_eq!(after.column("c"), vec![Some(dec(0.9))]);
    assert!(
        sel(&db, "SELECT ?r WHERE { << :alice :worksAt :acme ~ ?r >> }")
            .rows()
            .is_empty()
    );
    assert_invariants(&db);
}

/// Graphlava graph semantics ([[11-sparql#Graph semantics]]): `GRAPH` is
/// transitive, `GRAPH!` direct, `GRAPH ?g` binds the direct graph.
#[test]
fn graph_scope_transitive_and_direct() {
    let f = Fixture::new();
    let db = f.open();
    write(&db, |w| {
        w.nest(iri("g_ep1"), iri("g_agent")).unwrap();
        w.insert_in(iri("g_ep1"), iri("a"), iri("said"), Term::string("hi"))
            .unwrap();
    });
    let db = f.reopen(db);
    assert_eq!(
        sel(&db, "SELECT ?x WHERE { GRAPH :g_agent { ?x :said ?m } }").column("x"),
        vec![Some(iri("a"))]
    );
    assert!(
        sel(&db, "SELECT ?x WHERE { GRAPH! :g_agent { ?x :said ?m } }")
            .rows()
            .is_empty()
    );
    assert_eq!(
        sel(&db, "SELECT ?g WHERE { GRAPH ?g { ?x :said ?m } }").column("g"),
        vec![Some(iri("g_ep1"))]
    );
    // without GRAPH: the union of all graphs
    assert_eq!(sel(&db, "SELECT ?x WHERE { ?x :said ?m }").rows().len(), 1);
    // standard mode: GRAPH is direct and the default graph is only the default graph
    let std = graphlava::QueryOptions::standard();
    let q = format!("{P}SELECT ?x WHERE {{ GRAPH :g_agent {{ ?x :said ?m }} }}");
    assert!(db.sparql_with(&q, &std).unwrap().rows().is_empty());
    let q = format!("{P}SELECT ?x WHERE {{ ?x :said ?m }}");
    assert!(db.sparql_with(&q, &std).unwrap().rows().is_empty());
}

// @tg: verifies:: [[openspec:sparql-query#Datalog through SERVICE#Reachability from SPARQL]] {level: e2e}
#[test]
fn reachability_from_sparql() {
    let f = Fixture::new();
    let db = f.open();
    upd(
        &db,
        "INSERT DATA { :a :knows :b . :b :knows :c . :c :knows :d . :x :knows :y . }",
    )
    .unwrap();
    let db = f.reopen(db);
    let q = r#"SELECT ?y WHERE {
        SERVICE <meta:datalog> {
          """ @prefix : <http://ex.org/> .
              reach(?x, ?y) :- triple(?x, :knows, ?y) .
              reach(?x, ?z) :- reach(?x, ?y), triple(?y, :knows, ?z) . """ :program :inline .
          :a :reach ?y
        }
      } ORDER BY ?y"#;
    assert_eq!(
        sel(&db, q).column("y"),
        vec![Some(iri("b")), Some(iri("c")), Some(iri("d"))]
    );
    // an unknown SERVICE fails; SILENT yields one empty solution
    assert!(
        db.sparql(&format!(
            "{P}SELECT * WHERE {{ SERVICE <http://remote/> {{ ?s ?p ?o }} }}"
        ))
        .is_err()
    );
    assert_eq!(
        sel(
            &db,
            "SELECT * WHERE { SERVICE SILENT <http://remote/> { ?s ?p ?o } }"
        )
        .rows()
        .len(),
        1
    );
}

/// Property paths and aggregates over the real store.
#[test]
fn paths_and_aggregates() {
    let f = Fixture::new();
    let db = f.open();
    upd(
        &db,
        "INSERT DATA { :a :knows :b . :b :knows :c . :c :knows :a . :a :age 30 . :b :age 40 . }",
    )
    .unwrap();
    assert_eq!(
        sel(&db, "SELECT ?y WHERE { :a :knows/:knows ?y }").column("y"),
        vec![Some(iri("c"))]
    );
    assert_eq!(
        sel(&db, "SELECT ?y WHERE { :a :knows* ?y }").rows().len(),
        3
    );
    assert_eq!(
        sel(&db, "SELECT ?y WHERE { :a ^:knows ?y }").column("y"),
        vec![Some(iri("c"))]
    );
    let avg = sel(
        &db,
        "SELECT (AVG(?a) AS ?m) (SUM(?a) AS ?s) (MAX(?a) AS ?x) WHERE { ?p :age ?a }",
    );
    assert_eq!(avg.column("s"), vec![Some(Term::integer(70))]);
    assert_eq!(avg.column("x"), vec![Some(Term::integer(40))]);
    let g = sel(
        &db,
        "SELECT ?p (COUNT(?y) AS ?n) WHERE { ?p :knows ?y } GROUP BY ?p HAVING (COUNT(?y) > 0) ORDER BY ?p",
    );
    assert_eq!(g.rows().len(), 3);
}
