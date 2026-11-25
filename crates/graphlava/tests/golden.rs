//! Portable file round trip ([[21-e2e-tests#Portable file round trip]],
//! rule `tx-recovery-format`).
//!
//! `GRAPHLAVA_GOLDEN_OUT=<dir>` writes `golden.glv` and `expected.txt` from a
//! fixed seed (the Linux `e2e-golden` CI job). `GRAPHLAVA_GOLDEN_DIR=<dir>`
//! opens a copy of that file on another OS and compares every query result
//! byte for byte. With neither set, the test produces and checks a file
//! locally.

use std::path::Path;

use graphlava::{
    CoreOp, Database, Durability, Error, OpenOptions, QueryResult, Term, WriteRequest,
};

const P: &str = "PREFIX : <http://ex.org/>\nPREFIX meta: <https://graphlava.org/ns#>\n";

/// The query set; results are serialised one row per line, sorted.
const QUERIES: &[&str] = &[
    "SELECT ?s ?o WHERE { ?s :knows ?o }",
    "SELECT ?s ?v WHERE { ?s :score ?v FILTER(?v > 0.5) }",
    "SELECT ?s ?n WHERE { ?s :name ?n }",
    "SELECT ?r ?c WHERE { << ?s :worksAt ?o ~ ?r >> . ?r :confidence ?c }",
    "SELECT ?s WHERE { GRAPH :agent { ?s :said ?m } }",
    "SELECT (COUNT(*) AS ?n) WHERE { ?s :knows ?o }",
    "SELECT ?y WHERE { :n0 :knows+ ?y }",
    "ASK { :n1 :knows ?x }",
];

fn produce(dir: &Path) {
    let path = dir.join("golden.glv");
    let _ = std::fs::remove_file(&path);
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    let mut x: u64 = 7;
    let mut next = |m: u64| {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (x >> 33) % m
    };
    let iri = |s: String| Term::iri(format!("http://ex.org/{s}"));
    let mut ops = Vec::new();
    for i in 0..300 {
        let (a, b) = (next(60), next(60));
        ops.push(CoreOp::Insert {
            s: iri(format!("n{a}")).into(),
            p: iri("knows".into()).into(),
            o: iri(format!("n{b}")).into(),
            graph: None,
        });
        ops.push(CoreOp::Insert {
            s: iri(format!("n{i}")).into(),
            p: iri("score".into()).into(),
            o: Term::double(next(1000) as f64 / 1000.0).into(),
            graph: None,
        });
        ops.push(CoreOp::Insert {
            s: iri(format!("n{i}")).into(),
            p: iri("name".into()).into(),
            o: Term::string(format!("node {i} — ünïcode")).into(),
            graph: None,
        });
    }
    db.submit(WriteRequest { ops }, Durability::Sync).unwrap();
    db.sparql_update(
        &format!(
            "{P}INSERT DATA {{ :alice :worksAt :acme {{| :confidence 0.9 |}} . :episode1 meta:in :agent .
               GRAPH :episode1 {{ :alice :said \"hi\"@en }} }}"
        ),
        Durability::Sync,
    )
    .unwrap();
    db.close().unwrap();
    let db = Database::open(&path, OpenOptions::default()).unwrap();
    std::fs::write(dir.join("expected.txt"), run_queries(&db)).unwrap();
    db.close().unwrap();
}

fn serialise(r: &QueryResult) -> Vec<String> {
    let term = |t: &Option<Term>| t.as_ref().map_or("UNDEF".to_string(), |t| t.to_string());
    let mut lines: Vec<String> = match r {
        QueryResult::Solutions { rows, .. } => rows
            .iter()
            .map(|row| row.iter().map(term).collect::<Vec<_>>().join("\t"))
            .collect(),
        QueryResult::Boolean(b) => vec![b.to_string()],
        QueryResult::Graph(ts) => ts
            .iter()
            .map(|t| format!("{} {} {}", t[0], t[1], t[2]))
            .collect(),
    };
    lines.sort();
    lines
}

fn run_queries(db: &Database) -> String {
    let mut out = String::new();
    for q in QUERIES {
        out.push_str(&format!("## {q}\n"));
        for line in serialise(&db.sparql(&format!("{P}{q}")).unwrap()) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

fn check(dir: &Path) {
    let work = tempfile::tempdir().unwrap();
    let copy = work.path().join("golden.glv");
    std::fs::copy(dir.join("golden.glv"), &copy).unwrap();
    let db = Database::open(&copy, OpenOptions::default()).unwrap();
    let got = run_queries(&db);
    db.close().unwrap();
    let want = std::fs::read_to_string(dir.join("expected.txt")).unwrap();
    assert_eq!(
        got, want,
        "query results differ from the golden expectations"
    );
    // A copy with an unknown feature flag is refused and left unchanged.
    let flagged = work.path().join("flagged.glv");
    std::fs::copy(dir.join("golden.glv"), &flagged).unwrap();
    {
        let raw = redb::Database::open(&flagged).unwrap();
        let w = raw.begin_write().unwrap();
        {
            let mut t = w
                .open_table(redb::TableDefinition::<&[u8], &[u8]>::new("meta"))
                .unwrap();
            t.insert(&b"features"[..], &(1u64 << 41).to_le_bytes()[..])
                .unwrap();
        }
        w.commit().unwrap();
    }
    let before = std::fs::read(&flagged).unwrap();
    let err = Database::open(&flagged, OpenOptions::default());
    assert!(matches!(err, Err(Error::FormatMismatch(_))), "{err:?}");
    drop(err);
    assert_eq!(
        std::fs::read(&flagged).unwrap(),
        before,
        "the file is unchanged"
    );
}

// @tg: verifies:: [[21-e2e-tests#Portable file round trip]] {level: e2e}
#[test]
fn portable_file_round_trip() {
    if let Ok(out) = std::env::var("GRAPHLAVA_GOLDEN_OUT") {
        std::fs::create_dir_all(&out).unwrap();
        produce(Path::new(&out));
        return;
    }
    if let Ok(dir) = std::env::var("GRAPHLAVA_GOLDEN_DIR") {
        check(Path::new(&dir));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    produce(dir.path());
    check(dir.path());
}
