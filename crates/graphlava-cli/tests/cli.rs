//! End-to-end tests of the `graphlava` binary.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const EX: &str = "PREFIX : <http://example.org/>\n";

const PEOPLE_TTL: &str = r#"@prefix : <http://example.org/> .
:alice :name "Alice" ;
       :age 42 ;
       :knows :bob ;
       :worksAt :acme {| :confidence 0.9 ; :source :hr-export |} .
:bob   :name "Bob"@en ;
       :knows :carol .
:carol :knows [ :name "Dan" ] .
"#;

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_graphlava"))
}

fn finish(o: Output) -> Run {
    Run {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8(o.stdout).unwrap(),
        stderr: String::from_utf8(o.stderr).unwrap(),
    }
}

fn run(args: &[&str]) -> Run {
    finish(bin().args(args).stdin(Stdio::null()).output().unwrap())
}

fn run_stdin(args: &[&str], input: &str) -> Run {
    let mut c = bin()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    c.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    finish(c.wait_with_output().unwrap())
}

#[track_caller]
fn ok(r: &Run) {
    assert_eq!(r.code, 0, "stdout:\n{}\nstderr:\n{}", r.stdout, r.stderr);
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

struct Dir(tempfile::TempDir);

impl Dir {
    fn new() -> Dir {
        Dir(tempfile::tempdir().unwrap())
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let p = self.path(name);
        std::fs::write(&p, text).unwrap();
        p
    }
}

/// A database with the people data.
fn people(d: &Dir) -> PathBuf {
    let db = d.path("people.glv");
    let ttl = d.write("people.ttl", PEOPLE_TTL);
    ok(&run(&["import", s(&db), s(&ttl)]));
    db
}

// ---- general -----------------------------------------------------------------

#[test]
fn version_names_cli_and_core() {
    let r = run(&["--version"]);
    ok(&r);
    assert!(r.stdout.starts_with("graphlava "), "{}", r.stdout);
    assert!(r.stdout.contains(env!("CARGO_PKG_VERSION")));
    assert!(
        r.stdout
            .contains(&format!("graphlava core {}", graphlava::VERSION))
    );
}

#[test]
fn help_and_usage_errors() {
    let r = run(&["--help"]);
    ok(&r);
    for cmd in [
        "query",
        "datalog",
        "update",
        "import",
        "export",
        "shell",
        "info",
        "stats",
        "check",
        "sweep",
        "build-ordering",
        "build-index",
    ] {
        assert!(r.stdout.contains(cmd), "help lists {cmd}");
        ok(&run(&[cmd, "--help"]));
    }
    assert_eq!(run(&[]).code, 2);
    assert_eq!(run(&["query"]).code, 2);
    assert_eq!(run(&["frobnicate"]).code, 2);
    assert_eq!(run(&["build-ordering", "x.glv", "spo"]).code, 2);
    assert_eq!(
        run(&["query", "x.glv", "--timeout", "soon", "ASK {}"]).code,
        2
    );
}

#[test]
fn missing_file_and_locked_file_exit_3() {
    let d = Dir::new();
    let r = run(&["query", s(&d.path("none.glv")), "ASK {}"]);
    assert_eq!(r.code, 3);
    assert!(r.stderr.contains("no such database file"), "{}", r.stderr);
    assert!(
        !d.path("none.glv").exists(),
        "a query does not create a file"
    );

    let db = people(&d);
    let held = graphlava::Database::open(&db, graphlava::OpenOptions::default()).unwrap();
    let r = run(&["query", s(&db), "ASK {}"]);
    assert_eq!(r.code, 3, "{}", r.stderr);
    assert!(r.stderr.contains("locked"), "{}", r.stderr);
    drop(held);
    ok(&run(&["query", s(&db), "ASK {}"]));
}

// ---- query and update -------------------------------------------------------------

#[test]
fn update_then_query_in_every_format() {
    let d = Dir::new();
    let db = d.path("q.glv");
    let r = run(&[
        "update",
        s(&db),
        &format!(
            "{EX}INSERT DATA {{ :alice :knows :bob {{| :since 2020 |}} . :bob :name \"Bob, \\\"B\\\"\"@en }}"
        ),
    ]);
    ok(&r);
    assert!(r.stderr.contains("inserted"), "{}", r.stderr);

    let q = format!("{EX}SELECT ?who ?r ?y WHERE {{ << ?who :knows :bob ~ ?r >> . ?r :since ?y }}");
    let t = run(&["query", s(&db), &q]);
    ok(&t);
    let lines: Vec<&str> = t.stdout.lines().collect();
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        ["?who", "?r", "?y"]
    );
    assert!(lines[1].starts_with("----"));
    assert!(lines[2].starts_with(":alice  stmt:"), "{}", t.stdout);
    assert!(lines[2].ends_with("2020"));
    assert!(t.stderr.contains("1 row"));

    let j = run(&["query", s(&db), &q, "--format", "json"]);
    ok(&j);
    assert!(
        j.stdout
            .starts_with("{\"head\":{\"vars\":[\"who\",\"r\",\"y\"]}"),
        "{}",
        j.stdout
    );
    assert!(
        j.stdout
            .contains("{\"type\":\"uri\",\"value\":\"http://example.org/alice\"}")
    );
    assert!(
        j.stdout
            .contains("\"datatype\":\"http://www.w3.org/2001/XMLSchema#integer\"")
    );

    let c = run(&[
        "query",
        s(&db),
        &format!("{EX}SELECT ?n WHERE {{ :bob :name ?n }}"),
        "--format",
        "csv",
    ]);
    ok(&c);
    assert_eq!(c.stdout, "n\r\n\"Bob, \"\"B\"\"\"\r\n");

    let tsv = run(&[
        "query",
        s(&db),
        &format!("{EX}SELECT ?n WHERE {{ :bob :name ?n }}"),
        "--format",
        "tsv",
    ]);
    ok(&tsv);
    assert_eq!(tsv.stdout, "?n\n\"Bob, \\\"B\\\"\"@en\n");

    for (f, want) in [
        ("table", "true\n"),
        ("csv", "true\n"),
        ("tsv", "true\n"),
        ("json", "{\"head\":{},\"boolean\":true}\n"),
    ] {
        let a = run(&[
            "query",
            s(&db),
            &format!("{EX}ASK {{ :alice :knows :bob }}"),
            "--format",
            f,
        ]);
        ok(&a);
        assert_eq!(a.stdout, want, "ASK as {f}");
    }

    let g = run(&[
        "query",
        s(&db),
        &format!("{EX}CONSTRUCT {{ ?s :knows ?o }} WHERE {{ ?s :knows ?o }}"),
    ]);
    ok(&g);
    assert_eq!(
        g.stdout,
        "<http://example.org/alice> <http://example.org/knows> <http://example.org/bob> .\n"
    );
}

#[test]
fn query_text_from_file_and_stdin() {
    let d = Dir::new();
    let db = people(&d);
    let rq = d.write(
        "q.rq",
        &format!("{EX}SELECT ?n WHERE {{ :alice :name ?n }}"),
    );
    let r = run(&["query", s(&db), "-f", s(&rq), "--format", "csv"]);
    ok(&r);
    assert_eq!(r.stdout, "n\r\nAlice\r\n");
    let r = run_stdin(
        &["query", s(&db), "-", "--format", "csv"],
        &format!("{EX}SELECT ?a WHERE {{ :alice :age ?a }}"),
    );
    ok(&r);
    assert_eq!(r.stdout, "a\r\n42\r\n");
    let r = run_stdin(
        &["query", s(&db), "--format", "csv", "--quiet"],
        "ASK { ?s ?p ?o }",
    );
    ok(&r);
    assert_eq!(r.stdout, "true\n");
    assert_eq!(r.stderr, "", "--quiet prints no summary");
    assert_eq!(run_stdin(&["query", s(&db)], "  ").code, 2);
}

#[test]
fn query_errors_and_budgets() {
    let d = Dir::new();
    let db = people(&d);
    let r = run(&["query", s(&db), "SELECT WHERE {"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.starts_with("error: "), "{}", r.stderr);
    let r = run(&[
        "query",
        s(&db),
        "SELECT * WHERE { ?s ?p ?o }",
        "--max-rows",
        "2",
    ]);
    assert_eq!(r.code, 1, "{}", r.stdout);
    ok(&run(&[
        "query",
        s(&db),
        "SELECT * WHERE { ?s ?p ?o }",
        "--max-rows",
        "1000",
        "--timeout",
        "5s",
    ]));
    let r = run(&["update", s(&db), "INSERT DATA { broken"]);
    assert_eq!(r.code, 1);
}

#[test]
fn standard_mode_makes_graph_direct() {
    let d = Dir::new();
    let db = d.path("g.glv");
    ok(&run(&[
        "update",
        s(&db),
        &format!("{EX}INSERT DATA {{ GRAPH :child {{ :a :b :c }} . :child meta:in :parent }}"),
        "--durability",
        "sync",
    ]));
    let q = format!("{EX}ASK {{ GRAPH :parent {{ :a :b :c }} }}");
    assert_eq!(run(&["query", s(&db), &q]).stdout, "true\n");
    assert_eq!(run(&["query", s(&db), &q, "--standard"]).stdout, "false\n");
}

#[test]
fn datalog_programs() {
    let d = Dir::new();
    let db = people(&d);
    let prog = "@prefix : <http://example.org/> .\n\
        reach(?x, ?y) :- :knows(?x, ?y) .\n\
        reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .\n\
        ?- reach(:alice, ?who) .\n";
    let dl = d.write("reach.dl", prog);
    let r = run(&["datalog", s(&db), s(&dl), "--format", "csv"]);
    ok(&r);
    let mut got: Vec<&str> = r.stdout.lines().skip(1).collect();
    got.sort();
    assert_eq!(got.len(), 3, "{}", r.stdout);
    assert!(got[0].starts_with("_:"), "Dan is a blank node: {got:?}");
    assert_eq!(
        &got[1..],
        ["http://example.org/bob", "http://example.org/carol"]
    );

    let r = run(&[
        "query",
        s(&db),
        "--datalog",
        "-f",
        s(&dl),
        "--max-rows",
        "1",
    ]);
    ok(&r);
    assert_eq!(
        r.stdout.lines().count(),
        3,
        "header, rule, one row: {}",
        r.stdout
    );

    let r = run_stdin(&["datalog", s(&db), "-", "--format", "json"], prog);
    ok(&r);
    assert!(r.stdout.contains("\"vars\":[\"who\"]"), "{}", r.stdout);

    let assert_prog = d.write(
        "assert.dl",
        "@prefix : <http://example.org/> .\n@assert :colleague(?x, ?y) :- :knows(?x, ?y) .\n",
    );
    assert_eq!(
        run(&["datalog", s(&db), s(&assert_prog)]).code,
        1,
        "@assert needs --update"
    );
    ok(&run(&["datalog", s(&db), s(&assert_prog), "--update"]));
    let r = run(&[
        "query",
        s(&db),
        &format!("{EX}ASK {{ :alice :colleague :bob }}"),
    ]);
    assert_eq!(r.stdout, "true\n");
}

// ---- import and export -----------------------------------------------------------

/// Sorted lines with blank-node labels replaced, for comparing datasets.
fn canon(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut out = String::new();
            let mut it = l.chars().peekable();
            while let Some(c) = it.next() {
                if c == '_' && it.peek() == Some(&':') {
                    it.next();
                    while it.peek().is_some_and(|c| c.is_alphanumeric()) {
                        it.next();
                    }
                    out.push_str("_:B");
                } else {
                    out.push(c);
                }
            }
            out
        })
        .collect();
    lines.sort();
    lines
}

const DATASET_TRIG: &str = r#"@prefix : <http://example.org/> .
@prefix meta: <https://graphlava.org/ns#> .
:alice :worksAt :acme {| :confidence 0.9 |} .
:alice :knows :bob ~ :r1 {| :source :hr |} .
:n :claims <<( :alice :knows :carol )>> .
_:x :p "blank" .
:episode1 {
  :alice :said "hello"@en .
  :bob :heard _:y .
  _:y :says "hi" .
}
:episode1 meta:in :memory .
"#;

#[test]
fn round_trip_every_format() {
    let d = Dir::new();
    let src = d.write("src.trig", DATASET_TRIG);
    let db = d.path("a.glv");
    let r = run(&["import", s(&db), s(&src)]);
    ok(&r);
    assert!(r.stderr.contains("imported 12 quads"), "{}", r.stderr);
    let reference = run(&["export", s(&db), "--format", "nq"]);
    ok(&reference);
    for fmt in ["nt", "nq", "ttl", "trig"] {
        let out = d.path(&format!("dump.{fmt}"));
        ok(&run(&["export", s(&db), "-o", s(&out)]));
        let db2 = d.path(&format!("b-{fmt}.glv"));
        ok(&run(&["import", s(&db2), s(&out)]));
        // Compare line-based dumps: N-Quads for the quad formats, N-Triples
        // (graphs merged) for the triple formats.
        let cmp = if fmt == "nq" || fmt == "trig" {
            "nq"
        } else {
            "nt"
        };
        let again = run(&["export", s(&db2), "--format", cmp]);
        ok(&again);
        let first = run(&["export", s(&db), "--format", cmp]);
        assert_eq!(
            canon(&first.stdout),
            canon(&again.stdout),
            "round trip through {fmt}"
        );
        // The annotations still hang on their statements.
        let q = format!(
            "{EX}SELECT ?c ?src WHERE {{ << :alice :worksAt :acme ~ ?r >> . ?r :confidence ?c . \
             << :alice :knows :bob ~ ?k >> . ?k :source ?src }}"
        );
        let a = run(&["query", s(&db2), &q, "--format", "csv"]);
        ok(&a);
        assert_eq!(a.stdout, "c,src\r\n0.9,http://example.org/hr\r\n", "{fmt}");
        let ch = run(&["check", s(&db2)]);
        ok(&ch);
    }
    // Quad formats keep the graphs and the nesting.
    let db2 = d.path("b-trig.glv");
    let q = format!("{EX}SELECT ?w WHERE {{ GRAPH :memory {{ :alice :said ?w }} }}");
    assert_eq!(
        run(&["query", s(&db2), &q, "--format", "csv"]).stdout,
        "w\r\nhello\r\n"
    );
    assert!(
        canon(&reference.stdout)
            .iter()
            .any(|l| l.ends_with("<http://example.org/episode1> ."))
    );
    // The IRI reifier survives as an alias.
    assert!(
        reference.stdout.contains(
            "<http://example.org/r1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies>"
        )
    );
    // Triple formats merge the graphs.
    let nt = run(&["export", s(&db), "--format", "nt"]);
    assert!(
        !nt.stdout.contains("<http://example.org/episode1> ."),
        "{}",
        nt.stdout
    );
}

#[test]
fn import_into_a_graph_and_export_one_graph() {
    let d = Dir::new();
    let db = d.path("g.glv");
    let nt = d.write(
        "a.nt",
        "<http://example.org/a> <http://example.org/p> \"1\" .\n",
    );
    ok(&run(&[
        "import",
        s(&db),
        s(&nt),
        "--graph",
        "http://example.org/g1",
    ]));
    let nt2 = d.write(
        "b.nt",
        "<http://example.org/b> <http://example.org/p> \"2\" .\n",
    );
    ok(&run(&["import", s(&db), s(&nt2)]));
    let r = run(&["export", s(&db), "--graph", "http://example.org/g1"]);
    ok(&r);
    assert_eq!(
        r.stdout,
        "<http://example.org/a> <http://example.org/p> \"1\" <http://example.org/g1> .\n"
    );
    // A scoped export keeps the nesting edges of the graphs in scope.
    ok(&run(&[
        "update",
        s(&db),
        &format!("{EX}INSERT DATA {{ :g1 meta:in :top . :other meta:in :elsewhere }}"),
    ]));
    let r = run(&["export", s(&db), "--graph", "http://example.org/top"]);
    ok(&r);
    assert_eq!(
        r.stdout,
        "<http://example.org/a> <http://example.org/p> \"1\" <http://example.org/g1> .\n\
         <http://example.org/g1> <https://graphlava.org/ns#in> <http://example.org/top> .\n"
    );
    let r = run(&["export", s(&db), "--graph", "http://example.org/none"]);
    assert_eq!(r.code, 1);
    // stdin needs a format
    assert_eq!(run_stdin(&["import", s(&db), "-"], "").code, 2);
    let r = run_stdin(
        &["import", s(&db), "-", "--format", "nt", "--quiet"],
        "<http://example.org/c> <http://example.org/p> <http://example.org/d> .\n",
    );
    ok(&r);
    assert_eq!(r.stderr, "");
    assert_eq!(run(&["import", s(&db), s(&d.write("x.txt", ""))]).code, 2);
}

#[test]
fn import_reports_parse_errors() {
    let d = Dir::new();
    let db = d.path("e.glv");
    let bad = d.write("bad.ttl", "@prefix : <http://example.org/> .\n:a :b .\n");
    let r = run(&["import", s(&db), s(&bad)]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("parse error"), "{}", r.stderr);
}

#[test]
fn import_in_small_batches_keeps_blank_nodes_and_reifiers() {
    let d = Dir::new();
    let db = d.path("b.glv");
    let ttl = d.write(
        "b.ttl",
        "@prefix : <http://example.org/> .\n\
         _:x :p :o1 .\n_:x :p :o2 .\n_:x :p :o3 .\n\
         _:x :q :o4 {| :w 1 |} .\n\
         :z :about <<( _:x :q :o4 )>> .\n",
    );
    ok(&run(&["import", s(&db), s(&ttl), "--batch", "1"]));
    let r = run(&[
        "query",
        s(&db),
        &format!("{EX}SELECT (COUNT(DISTINCT ?x) AS ?n) WHERE {{ ?x :p ?o }}"),
        "--format",
        "csv",
    ]);
    assert_eq!(r.stdout, "n\r\n1\r\n");
    let r = run(&[
        "query",
        s(&db),
        &format!(
            "{EX}SELECT ?w WHERE {{ ?x :p :o1 . << ?x :q :o4 ~ ?r >> . ?r :w ?w . :z :about ?r }}"
        ),
        "--format",
        "csv",
    ]);
    assert_eq!(r.stdout, "w\r\n1\r\n", "{}", r.stderr);
}

// ---- shell -----------------------------------------------------------------------

#[test]
fn shell_through_stdin() {
    let d = Dir::new();
    let db = d.path("sh.glv");
    let script = "\
.prefix : <http://example.org/>
.mode update
INSERT DATA {
  :alice :knows :bob ;
         :age 42 .
  :bob :knows :carol {| :since 2020 |}
};
.mode sparql
SELECT ?x ?y WHERE {
  ?x :knows ?y
}

.format csv
SELECT ?y WHERE { << ?a :knows ?b ~ ?s >> . ?s :since ?y };
.mode datalog
reach(?x, ?z) :- :knows(?x, ?z) .
reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
?- reach(:alice, ?who) .

.mode sparql
.format json
ASK { :alice :age 42 };
SELECT nonsense;
.bogus
.mode
.quit
SELECT ?never WHERE { ?never ?p ?o };
";
    let r = run_stdin(&["shell", s(&db), "--history", s(&d.path("hist"))], script);
    assert_eq!(r.code, 1, "a failed statement makes a piped shell exit 1");
    let out = &r.stdout;
    assert!(out.contains(":alice  :bob"), "{out}");
    assert!(out.contains(":bob    :carol"), "{out}");
    assert!(out.contains("y\r\n2020\r\n"), "{out}");
    assert!(
        out.contains("who\r\nhttp://example.org/bob\r\nhttp://example.org/carol\r\n"),
        "{out}"
    );
    assert!(out.contains("{\"head\":{},\"boolean\":true}"), "{out}");
    assert!(out.contains("sparql\n"), "{out}");
    assert!(!out.contains("never"), "stops at .quit");
    let err = &r.stderr;
    assert!(err.contains("inserted 4"), "{err}");
    assert!(err.contains("(2 rows, "), "{err}");
    assert!(
        err.contains("error at 1:"),
        "positions count from the typed statement: {err}"
    );
    assert!(err.contains("unknown command '.bogus'"), "{err}");

    let r = run_stdin(
        &["shell", s(&db)],
        ".prefix : <http://example.org/>\nASK { :alice :knows :bob };\n",
    );
    ok(&r);
    assert_eq!(r.stdout, "true\n");
}

// ---- admin -------------------------------------------------------------------------

#[test]
fn info_and_stats() {
    let d = Dir::new();
    let db = people(&d);
    ok(&run(&[
        "update",
        s(&db),
        &format!("{EX}INSERT DATA {{ GRAPH :ep {{ :alice :said \"hi\" }} . :ep meta:in :mem }}"),
    ]));
    let r = run(&["info", s(&db)]);
    ok(&r);
    assert!(
        r.stdout.contains("format version           1"),
        "{}",
        r.stdout
    );
    assert!(r.stdout.contains("features"), "{}", r.stdout);
    assert!(r.stdout.contains("size"), "{}", r.stdout);
    assert!(r.stdout.contains("pso absent"), "{}", r.stdout);

    let r = run(&["stats", s(&db)]);
    ok(&r);
    let out = &r.stdout;
    assert!(out.contains("statements  11"), "{out}");
    assert!(
        out.lines()
            .any(|l| l.starts_with("<http://example.org/knows>") && l.ends_with(" 3")),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with("<http://example.org/ep>")
                && l.ends_with("<http://example.org/mem>")),
        "{out}"
    );
    assert!(out.contains("hyperedge types  none"), "{out}");
}

#[test]
fn stats_lists_hyperedge_types() {
    use graphlava::{Database, Durability, HedgeTypeDecl, OpenOptions, RoleSpec, Term};
    let d = Dir::new();
    let path = d.path("h.glv");
    {
        let ex = |l: &str| Term::iri(format!("http://example.org/{l}"));
        let db = Database::open(&path, OpenOptions::default()).unwrap();
        let mut w = db.write(Durability::Ordered).unwrap();
        w.declare_hedge_type(
            ex("Meeting"),
            HedgeTypeDecl {
                roles: vec![RoleSpec::one(ex("agent")), RoleSpec::one(ex("target"))],
                attrs: vec![],
            },
        )
        .unwrap();
        for who in ["bob", "carol"] {
            w.create_hedge(
                ex("Meeting"),
                None,
                vec![
                    (ex("agent").into(), ex("alice").into()),
                    (ex("target").into(), ex(who).into()),
                ],
                vec![],
            )
            .unwrap();
        }
        w.commit().unwrap();
        db.close().unwrap();
    }
    let r = run(&["stats", s(&path)]);
    ok(&r);
    assert!(
        r.stdout
            .lines()
            .any(|l| l.starts_with("<http://example.org/Meeting>")
                && l.contains(" 2  ")
                && l.ends_with("<http://example.org/agent> <http://example.org/target>")),
        "{}",
        r.stdout
    );
    // Hyperedges export as their type and role triples.
    let e = run(&["export", s(&path), "--format", "nt"]);
    ok(&e);
    assert_eq!(
        e.stdout.lines().filter(|l| l.contains("/agent>")).count(),
        2,
        "{}",
        e.stdout
    );
}

#[test]
fn check_passes_then_fails_on_a_damaged_file() {
    let d = Dir::new();
    let db = people(&d);
    let r = run(&["check", s(&db)]);
    ok(&r);
    assert!(r.stdout.contains("ok"));
    // Remove one entry of the POS ordering behind graphlava's back.
    {
        let rdb = redb::Database::open(&db).unwrap();
        let w = rdb.begin_write().unwrap();
        {
            let def: redb::TableDefinition<&[u8; 24], &[u8; 8]> = redb::TableDefinition::new("pos");
            let mut t = w.open_table(def).unwrap();
            let first = {
                use redb::ReadableTable;
                let (k, _) = t.first().unwrap().unwrap();
                *k.value()
            };
            t.remove(&first).unwrap();
        }
        w.commit().unwrap();
    }
    let r = run(&["check", s(&db)]);
    assert_eq!(r.code, 1, "{}{}", r.stdout, r.stderr);
    assert!(r.stderr.contains("invariant violation"), "{}", r.stderr);
}

#[test]
fn sweep_and_build_commands() {
    let d = Dir::new();
    let db = people(&d);
    ok(&run(&[
        "update",
        s(&db),
        &format!("{EX}DELETE DATA {{ :alice :name \"Alice\" }}"),
    ]));
    let r = run(&["sweep", s(&db)]);
    ok(&r);
    assert!(r.stdout.starts_with("marked "), "{}", r.stdout);
    let r = run(&["build-ordering", s(&db), "pso"]);
    ok(&r);
    assert_eq!(r.stdout, "ordering pso ready\n");
    assert!(run(&["info", s(&db)]).stdout.contains("pso ready"));
    let r = run(&["build-index", s(&db), "http://example.org/age"]);
    ok(&r);
    assert_eq!(r.stdout, "value index on <http://example.org/age> ready\n");
    ok(&run(&["build-index", s(&db), "<http://example.org/name>"]));
    ok(&run(&["build-index", s(&db), "rdfs:label"]));
    assert_eq!(run(&["build-index", s(&db), "label"]).code, 2);
    assert!(run(&["info", s(&db)]).stdout.contains("value-index"));
    let q = format!("{EX}SELECT ?a WHERE {{ ?s :age ?a FILTER(?a > 40) }}");
    assert_eq!(
        run(&["query", s(&db), &q, "--format", "csv"]).stdout,
        "a\r\n42\r\n"
    );
    ok(&run(&["check", s(&db)]));
}
