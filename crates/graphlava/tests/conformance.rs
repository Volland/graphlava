//! Rust runner of the binding conformance corpus `bindings/conformance/*.jsonl`
//! ([[adr:0042]]). It validates every case against the schema rules first,
//! then executes the cases meant for every host (`*`) or for `rust`, so a
//! corpus bug fails here and is never reported as a binding bug.

// @tg: verifies:: [[openspec:python-binding#Python API parity]] {level: conformance}
// @tg: verifies:: [[openspec:python-binding#Python term mapping]] {level: conformance}

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use graphlava::{
    CoreOp, Database, DeleteMode, Durability, Error, HedgeTypeDecl, Id, Literal, OpResult,
    OpenOptions, PredicateDecl, QueryResult, Reader, RoleSpec, Term, TermRef, WriteRequest,
};
use serde_json::Value;

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
const EX: &str = "http://ex.org/";

const HOSTS: &[&str] = &["*", "rust", "python", "node", "swift", "kotlin"];
const KINDS: &[&str] = &["roundtrip", "reject", "query", "ops"];
const ERRORS: &[&str] = &[
    "Io",
    "Corrupted",
    "Full",
    "FormatMismatch",
    "Locked",
    "Conflict",
    "NotFound",
    "ReferencedByStatement",
    "FillsRole",
    "GraphCycle",
    "GraphDepthExceeded",
    "HyperedgeShapeMismatch",
    "CascadeLimitExceeded",
    "WriteTimeout",
    "ReadOnly",
    "Invalid",
];
const OPS: &[&str] = &[
    "insert",
    "set",
    "update",
    "annotate",
    "delete",
    "remove",
    "statement_id",
    "insert_unasserted",
    "declare_predicate",
    "create_graph",
    "add_member",
    "remove_member",
    "nest",
    "unnest",
    "drop_graph",
    "declare_hedge_type",
    "create_hedge",
    "update_role",
    "reopen",
];
const NATIVE_TYPES: &[&str] = &[
    "int",
    "float",
    "bool",
    "str",
    "decimal",
    "datetime",
    "naive_datetime",
    "date",
];

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bindings/conformance")
}

fn load() -> Vec<(String, Value)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(corpus_dir())
        .expect("bindings/conformance exists")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        for (n, line) in std::fs::read_to_string(&f)
            .expect("read corpus")
            .lines()
            .enumerate()
        {
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{name}:{}: invalid JSON: {e}", n + 1));
            out.push((format!("{name}:{}", n + 1), v));
        }
    }
    out
}

// ---- validation -------------------------------------------------------------------

fn check_term(t: &Value, at: &str) {
    let o = t
        .as_object()
        .unwrap_or_else(|| panic!("{at}: term is not an object"));
    let keys: HashSet<&str> = o.keys().map(String::as_str).collect();
    let ok = if keys.contains("iri") {
        keys.len() == 1 && o["iri"].is_string()
    } else if keys.contains("bnode") {
        keys.len() == 1 && o["bnode"].is_string()
    } else if keys.contains("ref") {
        keys.len() == 1 && o["ref"].is_string()
    } else if keys.contains("literal") {
        o["literal"].is_string()
            && keys
                .iter()
                .all(|k| matches!(*k, "literal" | "datatype" | "lang"))
    } else if keys.contains("triple") {
        let a = o["triple"].as_array();
        if let Some(a) = a {
            a.iter().for_each(|x| check_term(x, at));
        }
        keys.len() == 1 && a.is_some_and(|a| a.len() == 3)
    } else {
        false
    };
    assert!(ok, "{at}: malformed term {t}");
}

fn check_value(v: &Value, at: &str) {
    let o = v.as_object().unwrap_or_else(|| panic!("{at}: value"));
    assert!(!o.is_empty(), "{at}: empty value");
    for k in o.keys() {
        assert!(
            matches!(k.as_str(), "term" | "native"),
            "{at}: value key {k}"
        );
    }
    if let Some(t) = o.get("term") {
        check_term(t, at);
    }
    if let Some(n) = o.get("native") {
        let ty = n["type"]
            .as_str()
            .unwrap_or_else(|| panic!("{at}: native.type"));
        assert!(n.get("value").is_some(), "{at}: native.value");
        // Host-specific native types are prefixed by their host name or listed.
        assert!(
            NATIVE_TYPES.contains(&ty) || !ty.is_empty(),
            "{at}: native type {ty}"
        );
    }
}

fn check_error(e: &Value, at: &str) {
    let e = e
        .as_str()
        .unwrap_or_else(|| panic!("{at}: error is a string"));
    assert!(ERRORS.contains(&e), "{at}: unknown error {e}");
}

fn check_op(op: &Value, at: &str) {
    let name = op["op"].as_str().unwrap_or_else(|| panic!("{at}: op name"));
    assert!(OPS.contains(&name), "{at}: unknown op {name}");
    for k in [
        "s", "p", "o", "expected", "graph", "child", "parent", "type", "role", "filler",
    ] {
        if let Some(t) = op.get(k) {
            check_term(t, at);
        }
    }
    if let Some(e) = op.get("error") {
        check_error(e, at);
    }
    if let Some(m) = op.get("mode") {
        assert!(
            matches!(m.as_str(), Some("cascade" | "keep_annotations" | "detach")),
            "{at}: mode {m}"
        );
    }
}

fn validate(cases: &[(String, Value)]) {
    let mut ids = HashSet::new();
    for (at, c) in cases {
        let id = c["id"].as_str().unwrap_or_else(|| panic!("{at}: id"));
        assert!(
            !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-.+".contains(&b)),
            "{at}: id {id:?} is not kebab-case"
        );
        assert!(ids.insert(id.to_string()), "{at}: duplicate id {id}");
        let kind = c["kind"].as_str().unwrap_or_else(|| panic!("{at}: kind"));
        assert!(KINDS.contains(&kind), "{at}: kind {kind}");
        if let Some(h) = c.get("hosts") {
            for h in h.as_array().unwrap_or_else(|| panic!("{at}: hosts")) {
                assert!(
                    h.as_str().is_some_and(|h| HOSTS.contains(&h)),
                    "{at}: host {h}"
                );
            }
        }
        match kind {
            "roundtrip" => {
                check_value(&c["value"], at);
                assert!(
                    c["value"].get("term").is_some(),
                    "{at}: roundtrip needs value.term"
                );
                if let Some(e) = c.get("expect") {
                    check_value(e, at);
                }
            }
            "reject" => {
                check_value(&c["value"], at);
                check_error(&c["error"], at);
            }
            "query" => {
                assert!(
                    matches!(c["lang"].as_str(), Some("sparql" | "datalog")),
                    "{at}: lang"
                );
                assert!(c["text"].is_string(), "{at}: text");
                let e = &c["expect"];
                assert!(
                    e.get("rows").is_some()
                        || e.get("boolean").is_some()
                        || e.get("count").is_some(),
                    "{at}: expect needs rows, boolean or count"
                );
                if let Some(rows) = e.get("rows") {
                    let n = e["vars"].as_array().expect("vars").len();
                    for r in rows.as_array().expect("rows") {
                        let r = r.as_array().expect("row");
                        assert_eq!(r.len(), n, "{at}: row width");
                        r.iter()
                            .filter(|t| !t.is_null())
                            .for_each(|t| check_term(t, at));
                    }
                }
                for op in c
                    .get("setup")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    check_op(op, at);
                }
            }
            "ops" => {
                for op in c["ops"].as_array().expect("ops") {
                    check_op(op, at);
                }
            }
            _ => unreachable!(),
        }
    }
}

// ---- execution --------------------------------------------------------------------

struct Run {
    _dir: tempfile::TempDir,
    path: PathBuf,
    db: Option<Database>,
    names: HashMap<String, Id>,
}

impl Run {
    fn new() -> Run {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("case.glv");
        let db = Database::open(&path, OpenOptions::default()).expect("open");
        Run {
            _dir: dir,
            path,
            db: Some(db),
            names: HashMap::new(),
        }
    }
    fn db(&self) -> &Database {
        self.db.as_ref().expect("open")
    }
    fn reopen(&mut self) {
        self.db.take().expect("open").close().expect("close");
        self.db = Some(Database::open(&self.path, OpenOptions::default()).expect("reopen"));
    }
    fn term(&self, t: &Value) -> Term {
        if let Some(i) = t.get("iri") {
            return Term::iri(i.as_str().unwrap());
        }
        if let Some(b) = t.get("bnode") {
            return Term::Blank(b.as_str().unwrap().to_string());
        }
        if let Some(l) = t.get("literal") {
            let lex = l.as_str().unwrap();
            if let Some(tag) = t.get("lang") {
                return Term::lit(Literal::lang(lex, tag.as_str().unwrap()));
            }
            let dt = t
                .get("datatype")
                .and_then(Value::as_str)
                .map_or_else(|| format!("{XSD}string"), str::to_string);
            return Term::lit(Literal::typed(lex, dt));
        }
        if let Some(a) = t.get("triple") {
            return Term::triple(self.term(&a[0]), self.term(&a[1]), self.term(&a[2]));
        }
        panic!("cannot build a term from {t}");
    }
    fn tref(&self, t: &Value) -> TermRef {
        match t.get("ref") {
            Some(r) => TermRef::Id(self.names[r.as_str().unwrap()]),
            None => TermRef::Term(self.term(t)),
        }
    }
    fn id(&self, op: &Value) -> Id {
        self.names[op["id"].as_str().expect("op id")]
    }
}

fn error_name(e: &Error) -> &'static str {
    match e {
        Error::Io(_) => "Io",
        Error::Corrupted(_) => "Corrupted",
        Error::Full => "Full",
        Error::FormatMismatch(_) => "FormatMismatch",
        Error::Locked => "Locked",
        Error::Conflict(_) => "Conflict",
        Error::NotFound(_) => "NotFound",
        Error::ReferencedByStatement { .. } => "ReferencedByStatement",
        Error::FillsRole { .. } => "FillsRole",
        Error::GraphCycle { .. } => "GraphCycle",
        Error::GraphDepthExceeded { .. } => "GraphDepthExceeded",
        Error::HyperedgeShapeMismatch(_) => "HyperedgeShapeMismatch",
        Error::CascadeLimitExceeded { .. } => "CascadeLimitExceeded",
        Error::WriteTimeout => "WriteTimeout",
        Error::ReadOnly => "ReadOnly",
        Error::Invalid(_) => "Invalid",
        _ => "Unknown",
    }
}

fn mode(op: &Value) -> DeleteMode {
    match op.get("mode").and_then(Value::as_str) {
        Some("detach") => DeleteMode::Detach,
        Some("keep_annotations") => DeleteMode::KeepAnnotations,
        _ => DeleteMode::Cascade,
    }
}

fn build_op(run: &Run, op: &Value) -> CoreOp {
    let t = |k: &str| run.tref(&op[k]);
    let opt = |k: &str| op.get(k).map(|v| run.tref(v));
    match op["op"].as_str().unwrap() {
        "insert" => CoreOp::Insert {
            s: t("s"),
            p: t("p"),
            o: t("o"),
            graph: opt("graph"),
        },
        "set" => CoreOp::Set {
            s: t("s"),
            p: t("p"),
            o: t("o"),
        },
        "update" => CoreOp::Update {
            id: run.id(op),
            o: t("o"),
            expected: opt("expected"),
        },
        "annotate" => CoreOp::Annotate {
            id: run.id(op),
            p: t("p"),
            o: t("o"),
        },
        "delete" => CoreOp::Delete {
            id: run.id(op),
            mode: mode(op),
        },
        "remove" => CoreOp::Remove {
            s: t("s"),
            p: t("p"),
            o: t("o"),
            mode: mode(op),
            graph: opt("graph"),
        },
        "statement_id" => CoreOp::StatementId {
            s: t("s"),
            p: t("p"),
            o: t("o"),
        },
        "insert_unasserted" => CoreOp::InsertUnasserted {
            s: t("s"),
            p: t("p"),
            o: t("o"),
        },
        "declare_predicate" => CoreOp::DeclarePredicate {
            p: t("p"),
            decl: PredicateDecl {
                multi: op.get("multi").and_then(Value::as_bool).unwrap_or(false),
                functional: op
                    .get("functional")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        },
        "create_graph" => CoreOp::CreateGraph { name: t("graph") },
        "add_member" => CoreOp::AddMember {
            item: run.id(op),
            graph: t("graph"),
        },
        "remove_member" => CoreOp::RemoveMember {
            item: run.id(op),
            graph: t("graph"),
        },
        "nest" => CoreOp::Nest {
            child: t("child"),
            parent: t("parent"),
        },
        "unnest" => CoreOp::Unnest {
            child: t("child"),
            parent: t("parent"),
        },
        "drop_graph" => CoreOp::DropGraph {
            graph: t("graph"),
            recursive: op
                .get("recursive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "declare_hedge_type" => CoreOp::DeclareHedgeType {
            ty: t("type"),
            decl: HedgeTypeDecl {
                roles: op["roles"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| RoleSpec {
                        role: run.tref(&r["role"]),
                        many: r.get("many").and_then(Value::as_bool).unwrap_or(false),
                        required: r.get("required").and_then(Value::as_bool).unwrap_or(true),
                        kind: None,
                    })
                    .collect(),
                attrs: op
                    .get("attrs")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|x| run.tref(x)).collect())
                    .unwrap_or_default(),
            },
        },
        "create_hedge" => {
            let pairs = |k: &str| -> Vec<(TermRef, TermRef)> {
                op.get(k)
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|p| (run.tref(&p[0]), run.tref(&p[1])))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            CoreOp::CreateHedge {
                ty: t("type"),
                graph: opt("graph"),
                roles: pairs("roles"),
                attrs: pairs("attrs"),
            }
        }
        "update_role" => CoreOp::UpdateRole {
            hedge: run.id(op),
            role: t("role"),
            filler: t("filler"),
        },
        other => panic!("op {other} has no CoreOp"),
    }
}

fn run_ops(run: &mut Run, ops: &[Value], at: &str) {
    for (i, op) in ops.iter().enumerate() {
        if op["op"] == "reopen" {
            run.reopen();
            continue;
        }
        let core = build_op(run, op);
        let r = run
            .db()
            .submit(WriteRequest { ops: vec![core] }, Durability::Ordered);
        match (op.get("error").and_then(Value::as_str), r) {
            (Some(want), Err(e)) => assert_eq!(error_name(&e), want, "{at} op {i}: {e}"),
            (Some(want), Ok(t)) => panic!("{at} op {i}: expected {want}, got {:?}", t.results),
            (None, Err(e)) => panic!("{at} op {i}: unexpected error {e}"),
            (None, Ok(t)) => {
                let res = &t.results[0];
                if let Some(name) = op.get("as").and_then(Value::as_str) {
                    let OpResult::Id(Some(id)) = res else {
                        panic!("{at} op {i}: `as` needs an ID result, got {res:?}")
                    };
                    run.names.insert(name.to_string(), *id);
                }
                if let Some(want) = op.get("result") {
                    let got = match res {
                        OpResult::Changed(b) | OpResult::Removed(b) => Value::Bool(*b),
                        OpResult::Deleted(v) => Value::from(v.len()),
                        other => panic!("{at} op {i}: result check on {other:?}"),
                    };
                    assert_eq!(&got, want, "{at} op {i}: result");
                }
            }
        }
    }
}

/// The canonical JSON form of a term, for comparison with the corpus.
fn canon_json(t: &Term) -> Value {
    match t {
        Term::Iri(i) => serde_json::json!({ "iri": i }),
        Term::Blank(b) => serde_json::json!({ "bnode": b }),
        Term::Literal(l) => match &l.lang {
            Some(tag) => serde_json::json!({ "literal": l.lexical, "lang": tag }),
            None if l.datatype == format!("{XSD}string") => {
                serde_json::json!({ "literal": l.lexical })
            }
            None => serde_json::json!({ "literal": l.lexical, "datatype": l.datatype }),
        },
        Term::Triple(tt) => serde_json::json!({
            "triple": [canon_json(&tt.subject), canon_json(&tt.predicate), canon_json(&tt.object)]
        }),
        _ => panic!("unsupported term {t:?}"),
    }
}

fn norm_json(run: &Run, t: &Value) -> Value {
    if t.is_null() {
        return Value::Null;
    }
    let mut t = canon_json(&run.term(t));
    if let Some(o) = t.as_object_mut() {
        if o.get("datatype").and_then(Value::as_str) == Some(LANG_STRING) {
            o.remove("datatype");
        }
    }
    t
}

fn exec_roundtrip(c: &Value, at: &str) {
    let mut run = Run::new();
    let value = &c["value"]["term"];
    let s = Term::iri(format!("{EX}s"));
    let p = Term::iri(format!("{EX}p"));
    run.db()
        .submit(
            WriteRequest {
                ops: vec![CoreOp::Insert {
                    s: s.clone().into(),
                    p: p.clone().into(),
                    o: run.tref(value),
                    graph: None,
                }],
            },
            Durability::Ordered,
        )
        .unwrap_or_else(|e| panic!("{at}: insert: {e}"));
    if c.get("reopen").and_then(Value::as_bool) == Some(true) {
        run.reopen();
    }
    let r = run.db().read().unwrap();
    let sid = r.lookup(&s).unwrap().expect("subject interned");
    let pid = r.lookup(&p).unwrap().expect("predicate interned");
    let ms = r.out(sid, Some(pid)).unwrap();
    assert_eq!(ms.len(), 1, "{at}: one statement");
    let o = ms[0].o;
    let got = canon_json(&r.decode(o).unwrap());
    let want = c.get("expect").and_then(|e| e.get("term")).unwrap_or(value);
    assert_eq!(got, norm_json(&run, want), "{at}: decoded term");
    if let Some(inline) = c.get("inline").and_then(Value::as_bool) {
        assert_eq!(o.is_inline(), inline, "{at}: inline flag of {o:?}");
    }
    // Lookup of the decoded term finds the same ID again, except for owned
    // literals, which have no shared dictionary entry ([[adr:0023]]).
    if !o.is_owned_literal() {
        assert_eq!(
            r.lookup(&r.decode(o).unwrap()).unwrap(),
            Some(o),
            "{at}: lookup"
        );
    }
}

fn sorted(mut rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    rows.sort_by_key(|r| serde_json::to_string(r).unwrap());
    rows
}

fn exec_query(c: &Value, at: &str) {
    let mut run = Run::new();
    if let Some(setup) = c.get("setup").and_then(Value::as_array) {
        run_ops(&mut run, setup, at);
    }
    let text = c["text"].as_str().unwrap();
    let res = match c["lang"].as_str().unwrap() {
        "sparql" => run.db().sparql(text),
        _ => run.db().datalog(text),
    }
    .unwrap_or_else(|e| panic!("{at}: query failed: {e}"));
    let e = &c["expect"];
    if let Some(b) = e.get("boolean") {
        assert_eq!(res, QueryResult::Boolean(b.as_bool().unwrap()), "{at}");
        return;
    }
    let QueryResult::Solutions { vars, rows } = res else {
        panic!("{at}: expected solutions, got {res:?}")
    };
    if let Some(n) = e.get("count") {
        assert_eq!(rows.len() as u64, n.as_u64().unwrap(), "{at}: row count");
        return;
    }
    let want_vars: Vec<String> = e["vars"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(vars, want_vars, "{at}: vars");
    let got: Vec<Vec<Value>> = rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|t| t.as_ref().map_or(Value::Null, canon_json))
                .collect()
        })
        .collect();
    let want: Vec<Vec<Value>> = e["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r.as_array()
                .unwrap()
                .iter()
                .map(|t| norm_json(&run, t))
                .collect()
        })
        .collect();
    if e.get("ordered").and_then(Value::as_bool) == Some(true) {
        assert_eq!(got, want, "{at}: rows");
    } else {
        assert_eq!(sorted(got), sorted(want), "{at}: rows");
    }
}

fn runs_in_rust(c: &Value) -> bool {
    c.get("hosts")
        .and_then(Value::as_array)
        .is_none_or(|h| h.iter().any(|h| h == "*" || h == "rust"))
}

#[test]
fn corpus_is_valid_and_large_enough() {
    let cases = load();
    validate(&cases);
    assert!(
        cases.len() >= 200,
        "{} cases; the corpus needs at least 200",
        cases.len()
    );
}

#[test]
fn corpus_passes_against_graphlava() {
    let cases = load();
    validate(&cases);
    let mine: Vec<&(String, Value)> = cases.iter().filter(|(_, c)| runs_in_rust(c)).collect();
    let ran = mine.len();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let failures: Vec<String> = std::thread::scope(|sc| {
        let handles: Vec<_> = mine
            .chunks(ran.div_ceil(threads).max(1))
            .map(|chunk| {
                sc.spawn(move || {
                    let mut failures = Vec::new();
                    for (at, c) in chunk {
                        let at = format!("{at} ({})", c["id"].as_str().unwrap());
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            match c["kind"].as_str().unwrap() {
                                "roundtrip" => exec_roundtrip(c, &at),
                                "query" => exec_query(c, &at),
                                "ops" => {
                                    let mut run = Run::new();
                                    run_ops(&mut run, c["ops"].as_array().unwrap(), &at);
                                    let inv = run.db().read().unwrap().check_invariants().unwrap();
                                    assert_eq!(inv, None, "{at}: invariants");
                                }
                                _ => {} // reject: host-level rule, validated only
                            }
                        }));
                        if let Err(p) = r {
                            failures.push(
                                p.downcast_ref::<String>()
                                    .cloned()
                                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                                    .unwrap_or_default(),
                            );
                        }
                    }
                    failures
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("runner thread"))
            .collect()
    });
    assert!(
        failures.is_empty(),
        "{} of {ran} cases failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
