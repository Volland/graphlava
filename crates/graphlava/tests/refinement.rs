//! Refinement replay ([[19-verification#Refinement bridge]], L1, [[adr:0019]]).
//!
//! `lake exe oracle <seed> <n>` runs the executable Lean model
//! `MetaCore.Store` on a seeded random operation sequence and prints one JSON
//! line per operation with the model's result. This test replays every
//! operation through the public API on a real file and checks that the Rust
//! store returns the same result, operation by operation, and that the final
//! states agree through the abstraction function α:
//!
//! - model `node n`  ↔ IRI `http://oracle/n{n}`, model predicate `p` ↔ IRI
//!   `http://oracle/p{p}` (predicate 7 is declared `multi`), model `lit n` ↔
//!   the inline integer `n`;
//! - model statement IDs ↔ Rust statement IDs by allocation order (a
//!   bijection built during the replay); every model entry is a statement with
//!   an ID, so literal-valued inserts are promoted on the Rust side
//!   (rule `dm-promote-on-annotate`: asking for the ID promotes).
//!
//! - model graph `0` ↔ the default graph, graph `n` ↔ the graph named
//!   `http://oracle/g{n}`; hyperedge type `t` ↔ `http://oracle/T{t}` with
//!   roles `http://oracle/r{r}` (cardinality one), declared from the schema
//!   line; model hyperedge IDs share the bijection with statement IDs.
//!
//! Seeds 1..64 × 2,000 operations by default (`GRAPHLAVA_REFINEMENT_SEEDS`,
//! `GRAPHLAVA_REFINEMENT_OPS` override). Needs `lake` on `PATH`; without it the
//! test is skipped unless `GRAPHLAVA_REQUIRE_REFINEMENT=1`.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

use graphlava::id::{self, DEFAULT_GRAPH, Kind};
use graphlava::{
    CoreOp, Database, DeleteMode, Durability, Error, HedgeTypeDecl, Id, OpResult, OpenOptions,
    PredicateDecl, Reader, RoleSpec, Term, TermRef, WriteRequest,
};
use serde_json::Value;

const NS: &str = "http://oracle/";

fn oracle(seed: u64, n: u64) -> Option<Vec<Value>> {
    let formal = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../formal");
    let out = Command::new("lake")
        .args(["exe", "oracle", &seed.to_string(), &n.to_string()])
        .current_dir(&formal)
        .output()
        .ok()?;
    if !out.status.success() {
        panic!("oracle failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Some(
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).expect("JSON line"))
            .collect(),
    )
}

struct Replay {
    db: Database,
    /// α on statement IDs: model → Rust and back.
    to_rust: HashMap<u64, Id>,
    to_model: HashMap<Id, u64>,
}

fn graph_ref(g: &Value) -> TermRef {
    match g.as_u64().unwrap() {
        0 => TermRef::Id(DEFAULT_GRAPH),
        n => TermRef::Term(Term::iri(format!("{NS}g{n}"))),
    }
}

fn blocked(e: &Error) -> bool {
    matches!(
        e,
        Error::ReferencedByStatement { .. } | Error::FillsRole { .. }
    )
}

impl Replay {
    fn removed(&self, gone: &[Id]) -> BTreeSet<u64> {
        gone.iter().map(|g| self.to_model[g]).collect()
    }

    fn want(e: &Value) -> BTreeSet<u64> {
        e["removed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect()
    }

    /// Records a fresh model ID ↔ Rust ID pair, or checks an existing one.
    fn bind(&mut self, expect: u64, id: Id, ctx: &str) {
        match self.to_rust.get(&expect) {
            Some(r) => assert_eq!(*r, id, "{ctx}: existing item"),
            None => {
                assert!(
                    !self.to_model.contains_key(&id),
                    "{ctx}: Rust returned an existing ID for a fresh model ID"
                );
                self.to_rust.insert(expect, id);
                self.to_model.insert(id, expect);
            }
        }
    }

    fn term(&self, v: &Value) -> Result<TermRef, ()> {
        if let Some(n) = v.get("node") {
            return Ok(TermRef::Term(Term::iri(format!(
                "{NS}n{}",
                n.as_u64().unwrap()
            ))));
        }
        if let Some(n) = v.get("lit") {
            return Ok(TermRef::Term(Term::integer(n.as_i64().unwrap())));
        }
        let i = v["stmt"].as_u64().unwrap();
        // The model only names live statements in terms.
        self.to_rust.get(&i).map(|id| TermRef::Id(*id)).ok_or(())
    }

    fn rust_id(&self, model: u64) -> Id {
        // An ID the model never allocated maps to an ID Rust never allocates.
        *self.to_rust.get(&model).unwrap_or(&Id::chunked(
            Kind::Statement,
            id::N_CHUNKS - 1,
            model % id::CHUNK,
        ))
    }

    fn submit(&self, ops: Vec<CoreOp>) -> Result<Vec<OpResult>, Error> {
        self.db
            .submit(WriteRequest { ops }, Durability::Relaxed)
            .map(|t| t.results)
    }

    fn step(&mut self, line: &Value, at: usize) {
        let ctx = || format!("op {at}: {line}");
        match line["op"].as_str().unwrap() {
            "insert" => {
                let t = &line["triple"];
                let (s, o) = (self.term(&t["s"]).unwrap(), self.term(&t["o"]).unwrap());
                let p = TermRef::Term(Term::iri(format!("{NS}p{}", t["p"].as_u64().unwrap())));
                // An unpromoted inline property has no ID: asking for it promotes.
                let res = self
                    .submit(vec![CoreOp::Insert {
                        s: s.clone(),
                        p: p.clone(),
                        o: o.clone(),
                        graph: Some(graph_ref(&line["graph"])),
                    }])
                    .unwrap_or_else(|e| panic!("{}: insert failed: {e}", ctx()));
                let id = match res[0] {
                    OpResult::Id(Some(id)) => id,
                    _ => match self
                        .submit(vec![CoreOp::StatementId { s, p, o }])
                        .unwrap_or_else(|e| panic!("{}: promote failed: {e}", ctx()))[0]
                    {
                        OpResult::Id(Some(id)) => id,
                        ref r => panic!("{}: no statement ID: {r:?}", ctx()),
                    },
                };
                let expect = line["expect"]["id"].as_u64().unwrap();
                self.bind(expect, id, &ctx());
            }
            "hedge" => {
                let ty = Term::iri(format!("{NS}T{}", line["type"].as_u64().unwrap()));
                let mut roles = Vec::new();
                for r in line["roles"].as_array().unwrap() {
                    let role = TermRef::Term(Term::iri(format!("{NS}r{}", r[0].as_u64().unwrap())));
                    roles.push((role, self.term(&r[1]).unwrap()));
                }
                let res = self.submit(vec![CoreOp::CreateHedge {
                    ty: ty.into(),
                    graph: Some(graph_ref(&line["graph"])),
                    roles,
                    attrs: Vec::new(),
                }]);
                match (res, line["expect"]["id"].as_u64()) {
                    (Ok(r), Some(expect)) => {
                        let OpResult::Id(Some(id)) = r[0] else {
                            panic!("{}: {r:?}", ctx())
                        };
                        assert_eq!(id.kind(), Kind::Hyperedge, "{}", ctx());
                        self.bind(expect, id, &ctx());
                    }
                    (Err(Error::HyperedgeShapeMismatch(_)), None) => {}
                    (other, _) => panic!("{}: got {other:?}", ctx()),
                }
            }
            "add_member" => {
                let item = self.rust_id(line["id"].as_u64().unwrap());
                if item.kind() == Kind::Hyperedge {
                    // The model's membership operations cover statements only
                    // (it answers `ok:false`); hyperedge memberships are
                    // covered by the e2e tests.
                    assert_eq!(line["expect"]["ok"], Value::Bool(false), "{}", ctx());
                    return;
                }
                let res = self.submit(vec![CoreOp::AddMember {
                    item,
                    graph: graph_ref(&line["graph"]),
                }]);
                let expect = line["expect"]["ok"].as_bool().unwrap();
                match (&res, expect) {
                    (Ok(_), true) | (Err(Error::NotFound(_)), false) => {}
                    _ => panic!("{}: got {res:?}", ctx()),
                }
            }
            "remove_member" => {
                let item = self.rust_id(line["id"].as_u64().unwrap());
                if item.kind() == Kind::Hyperedge {
                    // The model's membership operations cover statements only
                    // (it answers `ok:false`); hyperedge memberships are
                    // covered by the e2e tests.
                    assert_eq!(line["expect"]["ok"], Value::Bool(false), "{}", ctx());
                    return;
                }
                let res = self.submit(vec![CoreOp::RemoveMember {
                    item,
                    graph: graph_ref(&line["graph"]),
                }]);
                let e = &line["expect"];
                match (res, e["ok"].as_bool().unwrap()) {
                    (Ok(r), true) => {
                        let OpResult::Deleted(gone) = &r[0] else {
                            panic!("{}: {r:?}", ctx())
                        };
                        assert_eq!(self.removed(gone), Self::want(e), "{}: removed set", ctx());
                    }
                    (Err(ref x), false) if blocked(x) && e["blocked"] == Value::Bool(true) => {}
                    (Err(Error::NotFound(_)) | Err(Error::Invalid(_)), false)
                        if e.get("blocked").is_none() => {}
                    (other, _) => panic!("{}: got {other:?}", ctx()),
                }
            }
            "nest" | "unnest" => {
                let child = graph_ref(&line["child"]);
                let parent = graph_ref(&line["parent"]);
                let op = if line["op"] == "nest" {
                    CoreOp::Nest { child, parent }
                } else {
                    CoreOp::Unnest { child, parent }
                };
                let res = self.submit(vec![op]);
                let ok = match (&res, line["op"] == "nest") {
                    (Ok(_), true) => true,
                    (Ok(r), false) => r[0] == OpResult::Changed(true),
                    (Err(Error::GraphCycle { .. }), true) => false,
                    (Err(Error::Invalid(_)), false) => false,
                    _ => panic!("{}: got {res:?}", ctx()),
                };
                assert_eq!(ok, line["expect"]["ok"].as_bool().unwrap(), "{}", ctx());
            }
            "scope" => {
                let r = self.db.read().unwrap();
                let g = match line["graph"].as_u64().unwrap() {
                    0 => Some(DEFAULT_GRAPH),
                    n => r.graph(&Term::iri(format!("{NS}g{n}"))).unwrap(),
                };
                let got: Vec<u64> = match g {
                    None => Vec::new(),
                    Some(g) => {
                        let mut v: Vec<u64> = r
                            .scope_statements(g, line["direct"].as_bool().unwrap())
                            .unwrap()
                            .iter()
                            .map(|id| self.to_model[id])
                            .collect();
                        v.sort();
                        v
                    }
                };
                let want: Vec<u64> = line["expect"]["ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap())
                    .collect();
                assert_eq!(got, want, "{}: scope", ctx());
            }
            "update" => {
                let id = self.rust_id(line["id"].as_u64().unwrap());
                let o = self.term(&line["o"]).unwrap();
                let ok = self.submit(vec![CoreOp::Update {
                    id,
                    o,
                    expected: None,
                }]);
                let expect = line["expect"]["ok"].as_bool().unwrap();
                match (&ok, expect) {
                    (Ok(_), true) | (Err(Error::Conflict(_)) | Err(Error::NotFound(_)), false) => {}
                    _ => panic!("{}: got {ok:?}", ctx()),
                }
            }
            "delete" => {
                let model_id = line["id"].as_u64().unwrap();
                let id = self.rust_id(model_id);
                let mode = if line["detach"].as_bool().unwrap() {
                    DeleteMode::Detach
                } else {
                    DeleteMode::Cascade
                };
                let res = self.submit(vec![CoreOp::Delete { id, mode }]);
                let e = &line["expect"];
                match (res, e["ok"].as_bool().unwrap()) {
                    (Ok(r), true) => {
                        let OpResult::Deleted(gone) = &r[0] else {
                            panic!("{}: {r:?}", ctx())
                        };
                        assert_eq!(self.removed(gone), Self::want(e), "{}: removed set", ctx());
                    }
                    (Err(ref x), false) if blocked(x) && e["blocked"] == Value::Bool(true) => {}
                    (Err(Error::NotFound(_)), false) if e.get("blocked").is_none() => {}
                    (other, _) => panic!("{}: got {other:?}", ctx()),
                }
            }
            other => panic!("unknown op {other}"),
        }
    }

    /// α(final Rust state) == final model state.
    fn check_final(&self, line: &Value, seed: u64) {
        let r = self.db.read().unwrap();
        assert_eq!(
            r.check_invariants().unwrap(),
            None,
            "seed {seed}: invariants"
        );
        let term_json = |id: Id| -> Value {
            if id.kind() == Kind::Statement {
                return serde_json::json!({ "stmt": self.to_model[&id] });
            }
            if id.is_inline() {
                return serde_json::json!({ "lit": id::dec_int(id) });
            }
            let Term::Iri(i) = r.decode(id).unwrap() else {
                panic!("unexpected term {id}")
            };
            serde_json::json!({ "node": i.strip_prefix(&format!("{NS}n")).unwrap().parse::<u64>().unwrap() })
        };
        let mut got: Vec<(u64, Value)> = r
            .find(None, None, None)
            .unwrap()
            .into_iter()
            // virtual membership, role and type triples are not model statements
            .filter(
                |m| matches!(r.decode(m.p), Ok(Term::Iri(p)) if p.starts_with(&format!("{NS}p"))),
            )
            .map(|m| {
                let id = m.id.expect("every replayed statement has an ID");
                let Term::Iri(p) = r.decode(m.p).unwrap() else {
                    panic!()
                };
                let p: u64 = p.strip_prefix(&format!("{NS}p")).unwrap().parse().unwrap();
                (
                    self.to_model[&id],
                    serde_json::json!({ "s": term_json(m.s), "p": p, "o": term_json(m.o) }),
                )
            })
            .collect();
        got.sort_by_key(|(i, _)| *i);
        let mut want: Vec<(u64, Value)> = line["final"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e[0].as_u64().unwrap(), e[1].clone()))
            .collect();
        want.sort_by_key(|(i, _)| *i);
        assert_eq!(got, want, "seed {seed}: final state differs");
        // memberships of every live statement
        let graph_no = |g: Id| -> u64 {
            if g == DEFAULT_GRAPH {
                return 0;
            }
            let Term::Iri(n) = r.decode(g).unwrap() else {
                panic!("graph {g}")
            };
            n.strip_prefix(&format!("{NS}g")).unwrap().parse().unwrap()
        };
        for e in line["members"].as_array().unwrap() {
            let id = self.to_rust[&e[0].as_u64().unwrap()];
            let mut gs: Vec<u64> = r.graphs_of(id).unwrap().into_iter().map(graph_no).collect();
            gs.sort();
            let want: Vec<u64> = e[1]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect();
            assert_eq!(gs, want, "seed {seed}: memberships of {}", e[0]);
        }
        // live hyperedges, their types and roles
        let mut hedges = 0;
        for e in line["hedges"].as_array().unwrap() {
            let id = self.to_rust[&e[0].as_u64().unwrap()];
            let h = r
                .hedge(id)
                .unwrap()
                .unwrap_or_else(|| panic!("seed {seed}: hyperedge {} is gone", e[0]));
            let Term::Iri(ty) = r.decode(h.ty).unwrap() else {
                panic!()
            };
            assert_eq!(
                ty,
                format!("{NS}T{}", e[1]["type"]),
                "seed {seed}: type of {}",
                e[0]
            );
            let mut got: Vec<(String, Value)> = h
                .roles
                .iter()
                .map(|(role, n)| {
                    let Term::Iri(role) = r.decode(*role).unwrap() else {
                        panic!()
                    };
                    (role, term_json(*n))
                })
                .collect();
            got.sort_by(|a, b| a.0.cmp(&b.0));
            let mut want: Vec<(String, Value)> = e[1]["roles"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| (format!("{NS}r{}", x[0]), x[1].clone()))
                .collect();
            want.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(got, want, "seed {seed}: roles of {}", e[0]);
            hedges += 1;
        }
        let live_hedges = self
            .to_rust
            .values()
            .filter(|id| id.kind() == Kind::Hyperedge && r.hedge(**id).unwrap().is_some())
            .count();
        assert_eq!(live_hedges, hedges, "seed {seed}: live hyperedges");
        // `next` is the number of statement IDs ever allocated, plus one.
        assert_eq!(
            self.to_rust.len() as u64 + 1,
            line["next"].as_u64().unwrap(),
            "seed {seed}: allocator"
        );
    }
}

fn env_u64(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

// @tg: verifies:: [[21-e2e-tests#Model trace replay]] {level: refinement}
#[test]
fn refinement_replay() {
    let seeds = env_u64("GRAPHLAVA_REFINEMENT_SEEDS", 64);
    let ops = env_u64("GRAPHLAVA_REFINEMENT_OPS", 2000);
    if oracle(1, 1).is_none() {
        assert!(
            std::env::var("GRAPHLAVA_REQUIRE_REFINEMENT").is_err(),
            "`lake` not found but GRAPHLAVA_REQUIRE_REFINEMENT is set"
        );
        eprintln!("refinement_replay skipped: `lake` is not on PATH");
        return;
    }
    let threads: Vec<_> = (1..=seeds)
        .collect::<Vec<_>>()
        .chunks(seeds.div_ceil(8).max(1) as usize)
        .map(|c| c.to_vec())
        .map(|chunk| {
            std::thread::spawn(move || {
                for seed in chunk {
                    let trace = oracle(seed, ops).unwrap();
                    let dir = tempfile::tempdir().unwrap();
                    let db = Database::open(dir.path().join("replay.glv"), OpenOptions::default())
                        .unwrap();
                    let multi = trace[0]["schema"]["multi"].as_array().unwrap().clone();
                    for p in multi {
                        let p = TermRef::Term(Term::iri(format!("{NS}p{}", p.as_u64().unwrap())));
                        db.submit(
                            WriteRequest {
                                ops: vec![CoreOp::DeclarePredicate {
                                    p,
                                    decl: PredicateDecl {
                                        multi: true,
                                        functional: false,
                                    },
                                }],
                            },
                            Durability::Relaxed,
                        )
                        .unwrap();
                    }
                    for t in trace[0]["schema"]["htypes"].as_array().unwrap() {
                        let required: Vec<u64> = t["required"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_u64().unwrap())
                            .collect();
                        let roles = t["roles"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|r| {
                                let r = r.as_u64().unwrap();
                                let role = Term::iri(format!("{NS}r{r}"));
                                if required.contains(&r) {
                                    RoleSpec::one(role)
                                } else {
                                    RoleSpec::optional(role)
                                }
                            })
                            .collect();
                        db.submit(
                            WriteRequest {
                                ops: vec![CoreOp::DeclareHedgeType {
                                    ty: Term::iri(format!("{NS}T{}", t["id"])).into(),
                                    decl: HedgeTypeDecl {
                                        roles,
                                        attrs: Vec::new(),
                                    },
                                }],
                            },
                            Durability::Relaxed,
                        )
                        .unwrap();
                    }
                    let mut rp = Replay {
                        db,
                        to_rust: HashMap::new(),
                        to_model: HashMap::new(),
                    };
                    let path = dir.path().join("replay.glv");
                    for (i, line) in trace[1..trace.len() - 1].iter().enumerate() {
                        rp.step(line, i);
                        // Scenario "Model trace replay": close and reopen mid-trace.
                        if i == 1000 {
                            let db = std::mem::replace(
                                &mut rp.db,
                                Database::open(
                                    dir.path().join("scratch.glv"),
                                    OpenOptions::default(),
                                )
                                .unwrap(),
                            );
                            db.close().unwrap();
                            let scratch = std::mem::replace(
                                &mut rp.db,
                                Database::open(&path, OpenOptions::default()).unwrap(),
                            );
                            scratch.close().unwrap();
                        }
                    }
                    rp.check_final(trace.last().unwrap(), seed);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().expect("replay thread");
    }
}
