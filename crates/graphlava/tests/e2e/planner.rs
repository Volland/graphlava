//! Planner and executor checks of [[14-planner-execution#Test plan]].

use std::collections::BTreeSet;

use graphlava::query::exec::Exec;
use graphlava::{CoreOp, Database, Durability, Reader, Term, WriteRequest};

use crate::fixture::*;

const P: &str = "PREFIX : <http://ex.org/>\n";

fn bulk(db: &Database, triples: Vec<(String, String, String)>) {
    for chunk in triples.chunks(2000) {
        let ops = chunk
            .iter()
            .map(|(s, p, o)| CoreOp::Insert {
                s: iri(s).into(),
                p: iri(p).into(),
                o: iri(o).into(),
                graph: None,
            })
            .collect();
        db.submit(WriteRequest { ops }, Durability::Relaxed)
            .unwrap();
    }
}

/// Rule `plan-inlj`: a 2-hop traversal from a bound node probes the store
/// once per frontier node, not once per `:worksAt` statement.
#[test]
fn two_hop_traversal_reads_the_frontier() {
    let f = Fixture::new();
    let db = f.open();
    let mut t = Vec::new();
    for i in 0..40 {
        t.push(("alice".into(), "knows".into(), format!("p{i}")));
        t.push((format!("p{i}"), "worksAt".into(), format!("c{}", i % 5)));
    }
    for i in 0..5000 {
        t.push((
            format!("other{i}"),
            "worksAt".into(),
            format!("c{}", i % 50),
        ));
    }
    bulk(&db, t);
    let r = db.read().unwrap();
    let mut ex = Exec::new(r.kv(), r.dict());
    ex.stats = Some(graphlava::query::plan::Stats::load(r.kv()).unwrap());
    let res = graphlava::sparql::query(
        &ex,
        &format!("{P}SELECT ?x ?c WHERE {{ :alice :knows ?x . ?x :worksAt ?c }}"),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(res.rows().len(), 40);
    let probes = ex.probes.get();
    assert!(probes <= 41 + 5, "{probes} probes for a 40-node frontier");
}

/// Leapfrog Triejoin on a triangle equals a brute-force enumeration.
#[test]
fn triangle_matches_brute_force() {
    let f = Fixture::new();
    let db = f.open();
    let mut x: u64 = 3;
    let mut edges = BTreeSet::new();
    for _ in 0..300 {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let a = (x >> 33) % 25;
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let b = (x >> 33) % 25;
        edges.insert((a, b));
    }
    bulk(
        &db,
        edges
            .iter()
            .map(|(a, b)| (format!("n{a}"), "knows".into(), format!("n{b}")))
            .collect(),
    );
    let mut want = BTreeSet::new();
    for (a, b) in &edges {
        for (b2, c) in &edges {
            if b2 == b && edges.contains(&(*c, *a)) {
                want.insert(format!("n{a} n{b} n{c}"));
            }
        }
    }
    let res = db
        .sparql(&format!(
            "{P}SELECT ?a ?b ?c WHERE {{ ?a :knows ?b . ?b :knows ?c . ?c :knows ?a }}"
        ))
        .unwrap();
    let name = |t: &Option<Term>| match t {
        Some(Term::Iri(i)) => i.trim_start_matches("http://ex.org/").to_string(),
        other => panic!("{other:?}"),
    };
    let got: BTreeSet<String> = res
        .rows()
        .iter()
        .map(|r| format!("{} {} {}", name(&r[0]), name(&r[1]), name(&r[2])))
        .collect();
    assert_eq!(got.len(), res.rows().len(), "no duplicate rows");
    assert_eq!(got, want);
}

/// Rules `plan-cache-key` and `plan-cache-invalidate`: a repeated query hits
/// the cache; a 4x drift in a predicate's count invalidates the plan.
#[test]
fn plan_cache_hits_and_drift_invalidation() {
    let f = Fixture::new();
    let db = f.open();
    bulk(
        &db,
        (0..10)
            .map(|i| (format!("s{i}"), "p".into(), "o".into()))
            .collect(),
    );
    let q = format!("{P}SELECT ?s WHERE {{ ?s :p :o }}");
    db.sparql(&q).unwrap();
    db.sparql(&q).unwrap();
    let (hits, _, inval) = db.plan_cache_stats();
    assert_eq!((hits, inval), (1, 0));
    bulk(
        &db,
        (10..100)
            .map(|i| (format!("s{i}"), "p".into(), "o".into()))
            .collect(),
    );
    assert_eq!(db.sparql(&q).unwrap().rows().len(), 100);
    let (_, _, inval) = db.plan_cache_stats();
    assert_eq!(inval, 1, "10 -> 100 statements is a drift above 4x");
}

/// Differential test: random BGPs over a random graph agree with a naive
/// evaluator.
#[test]
fn random_bgps_agree_with_naive_evaluation() {
    let f = Fixture::new();
    let db = f.open();
    let mut x: u64 = 11;
    let mut next = |m: u64| {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) % m
    };
    let mut triples = BTreeSet::new();
    for _ in 0..200 {
        triples.insert((next(15), next(3), next(15)));
    }
    bulk(
        &db,
        triples
            .iter()
            .map(|(s, p, o)| (format!("n{s}"), format!("p{p}"), format!("n{o}")))
            .collect(),
    );
    for _ in 0..30 {
        // two or three patterns over variables ?v0..?v2 and constants
        let n = 2 + next(2) as usize;
        let mut pats = Vec::new();
        for _ in 0..n {
            let term = |k: u64, next: &mut dyn FnMut(u64) -> u64| -> (String, Option<u64>) {
                if next(3) == 0 {
                    let c = next(15);
                    (format!(":n{c}"), Some(c + 100))
                } else {
                    (format!("?v{k}"), None)
                }
            };
            let (s, sc) = term(next(3), &mut next);
            let p = next(3);
            let (o, oc) = term(next(3), &mut next);
            pats.push((s, sc, p, o, oc));
        }
        let q = format!(
            "{P}SELECT * WHERE {{ {} }}",
            pats.iter()
                .map(|(s, _, p, o, _)| format!("{s} :p{p} {o} ."))
                .collect::<String>()
        );
        let got = db.sparql(&q).unwrap().rows().len();
        // naive: enumerate assignments of ?v0..?v2 over the 15 nodes
        let mut count = 0;
        let vars_used: BTreeSet<String> = pats
            .iter()
            .flat_map(|(s, _, _, o, _)| [s.clone(), o.clone()])
            .filter(|t| t.starts_with('?'))
            .collect();
        let vars: Vec<String> = vars_used.into_iter().collect();
        let total = 15usize.pow(vars.len() as u32);
        for code in 0..total {
            let mut c = code;
            let val: Vec<u64> = (0..vars.len())
                .map(|_| {
                    let v = (c % 15) as u64;
                    c /= 15;
                    v
                })
                .collect();
            let resolve = |t: &str, k: &Option<u64>| -> u64 {
                match k {
                    Some(c) => c - 100,
                    None => val[vars.iter().position(|v| v == t).unwrap()],
                }
            };
            if pats
                .iter()
                .all(|(s, sc, p, o, oc)| triples.contains(&(resolve(s, sc), *p, resolve(o, oc))))
            {
                count += 1;
            }
        }
        assert_eq!(got, count, "{q}");
    }
}

/// [[adr:0041]]: a cyclic query that wants a missing on-demand ordering runs
/// with the fallback plan and counts the miss; the background build then
/// creates the ordering and the answers stay the same.
#[test]
fn missing_ordering_fallback_and_build() {
    let f = Fixture::new();
    let db = f.open();
    let mut t = Vec::new();
    for i in 0..20 {
        t.push((
            format!("n{i}"),
            "knows".into(),
            format!("n{}", (i + 1) % 20),
        ));
        t.push((
            format!("n{i}"),
            "likes".into(),
            format!("n{}", (i + 2) % 20),
        ));
        t.push((
            format!("n{}", (i + 2) % 20),
            "trusts".into(),
            format!("n{i}"),
        ));
    }
    bulk(&db, t);
    // ?a :knows ?b . ?b :knows ?c . ?a :likes ?c . ?c :trusts ?a is cyclic;
    // with ?p unbound in one atom, an atom wants a key order that only an
    // on-demand ordering has.
    let q = format!(
        "{P}SELECT ?a ?b ?c WHERE {{ ?a :knows ?b . ?b :knows ?c . ?c ?p ?a . ?a :likes ?c }}"
    );
    let before = db.sparql(&q).unwrap();
    assert!(!before.rows().is_empty());
    let misses = db.ordering_misses();
    assert!(
        misses.iter().sum::<u64>() > 0,
        "a missing ordering was wanted: {misses:?}"
    );
    let built = db.admin().build_wanted_orderings(1).unwrap();
    assert!(!built.is_empty());
    let after = db.sparql(&q).unwrap();
    assert_eq!(before.rows().len(), after.rows().len());
    assert_invariants(&db);
}

/// Rule `plan-path-bfs`: the bidirectional search with both ends bound
/// agrees with the one-sided search for `*` and `+`.
#[test]
fn bidirectional_path_agrees() {
    let f = Fixture::new();
    let db = f.open();
    let mut x: u64 = 5;
    let mut t = Vec::new();
    for _ in 0..120 {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let a = (x >> 33) % 40;
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let b = (x >> 33) % 40;
        t.push((format!("n{a}"), "e".into(), format!("n{b}")));
    }
    bulk(&db, t);
    for (a, b) in [(0, 1), (3, 3), (5, 39), (7, 20), (11, 2)] {
        for op in ["*", "+"] {
            let both = db
                .sparql(&format!("{P}ASK {{ :n{a} :e{op} :n{b} }}"))
                .unwrap();
            let one = db
                .sparql(&format!(
                    "{P}SELECT ?y WHERE {{ :n{a} :e{op} ?y FILTER(?y = :n{b}) }}"
                ))
                .unwrap();
            assert_eq!(
                both,
                graphlava::QueryResult::Boolean(!one.rows().is_empty()),
                "n{a} {op} n{b}"
            );
        }
    }
}
