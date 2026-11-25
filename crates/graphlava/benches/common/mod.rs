//! Fixtures and case bodies shared by the micro benchmarks
//! ([[17-benchmarks#Micro benchmarks]]): `micro.rs` measures them with
//! criterion (wall clock, local runs), `iai.rs` with iai-callgrind
//! (instruction counts, the pull-request gate of adr 0020). Both files call
//! the same functions here, so the two tiers measure the same work.
//!
//! Only public items of `graphlava` are used. Setup (opening the database,
//! loading data, opening the write transaction) is never part of a measured
//! call; every store case runs inside a fresh, uncommitted write transaction
//! that is dropped (aborted) after the measurement, so each iteration does the
//! same work on the same committed state.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use graphlava::dict::Dict;
use graphlava::id::{self, Id, Subtype};
use graphlava::query::exec::{Exec, Row};
use graphlava::query::ir::{GraphSpec, Node, Pt, TriplePat};
use graphlava::query::lftj;
use graphlava::storage::KvWrite;
use graphlava::tables::{self, Perm};
use graphlava::{Database, DeleteMode, Durability, OpenOptions, Reader, Term, WriteTxn};

pub const EX: &str = "http://ex.org/";

pub fn iri(local: &str) -> Term {
    Term::iri(format!("{EX}{local}"))
}

pub fn node(i: usize) -> Term {
    iri(&format!("n{i}"))
}

/// A fresh, empty directory for one fixture under Cargo's per-target
/// scratch directory (compile-time path, so it also works under
/// iai-callgrind, which clears the environment). Removed and recreated on
/// every run.
pub fn bench_dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("graphlava-micro")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("create bench dir");
    d
}

/// Default options with a write timeout long enough for slow runs under
/// Callgrind.
pub fn options() -> OpenOptions {
    OpenOptions {
        write_timeout: Duration::from_secs(3600),
        ..OpenOptions::default()
    }
}

pub fn open(dir: &Path) -> Database {
    Database::open(dir.join("bench.glv"), options()).expect("open database")
}

// ---- ID encode and decode per inline subtype ---------------------------------

/// One input value per inline subtype ([[02-id-encoding#Inline literals]]).
#[derive(Debug, Clone, Copy)]
pub enum Sample {
    Integer(i64),
    Decimal(i128),
    Boolean(bool),
    DateTime(i64, bool),
    Date(i64, bool),
    String(&'static str),
    LangString(&'static str, u16),
    Double(f64),
}

pub const SAMPLES: [(&str, Sample); 8] = [
    ("integer", Sample::Integer(-1_234_567_890)),
    ("decimal", Sample::Decimal(3_141_592_653)),
    ("boolean", Sample::Boolean(true)),
    ("dateTime", Sample::DateTime(1_732_406_400_000, true)),
    ("date", Sample::Date(1_732_406_400_000, false)),
    ("string", Sample::String("alice")),
    ("langString", Sample::LangString("chat", 7)),
    ("double", Sample::Double(1_234.567_8)),
];

/// Encodes a sample to its inline ID.
#[inline(never)]
pub fn encode(s: Sample) -> Option<Id> {
    match s {
        Sample::Integer(x) => id::enc_int(x),
        Sample::Decimal(x) => id::enc_decimal(x),
        Sample::Boolean(b) => Some(id::enc_bool(b)),
        Sample::DateTime(t, tz) => id::enc_instant(Subtype::DateTime, t, tz),
        Sample::Date(t, tz) => id::enc_instant(Subtype::Date, t, tz),
        Sample::String(s) => id::enc_string(s),
        Sample::LangString(s, l) => id::enc_lang_string(s, l),
        Sample::Double(x) => id::enc_double(x),
    }
}

/// A decoded inline value.
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded {
    Integer(i64),
    Decimal(i128),
    Boolean(bool),
    Instant(i64, bool),
    String(String),
    LangString(String, u16),
    Double(f64),
}

/// Decodes an inline ID of the sample's subtype.
#[inline(never)]
pub fn decode(s: Sample, id: Id) -> Decoded {
    match s {
        Sample::Integer(_) => Decoded::Integer(id::dec_int(id)),
        Sample::Decimal(_) => Decoded::Decimal(id::dec_decimal(id)),
        Sample::Boolean(_) => Decoded::Boolean(id::dec_bool(id)),
        Sample::DateTime(..) | Sample::Date(..) => {
            let (t, tz) = id::dec_instant(id);
            Decoded::Instant(t, tz)
        }
        Sample::String(_) => Decoded::String(id::dec_string(id)),
        Sample::LangString(..) => {
            let (s, l) = id::dec_lang_string(id);
            Decoded::LangString(s, l)
        }
        Sample::Double(_) => Decoded::Double(id::dec_double(id)),
    }
}

/// The sample and its encoded ID, for decode cases.
pub fn encoded(s: Sample) -> (Sample, Id) {
    (s, encode(s).expect("sample is inline"))
}

// ---- composite keys ---------------------------------------------------------------

/// A statement `(s, p, o)` of ordinary IDs.
pub const SPO: [Id; 3] = [
    Id(0x1000_0000_0012_3456),
    Id(0x1000_0000_0000_0042),
    Id(0xC000_0000_0000_1234),
];

/// Encodes the three base-ordering keys of a statement and decodes one back
/// ([[04-physical-layout#Key byte order]]).
#[inline(never)]
pub fn composite_keys(spo: [Id; 3]) -> ([Vec<u8>; 3], [Id; 3]) {
    let [s, p, o] = spo;
    let keys = Perm::BASE.map(|perm| tables::key(&perm.arrange(s, p, o)));
    let back = tables::unkey3(&keys[1]);
    (keys, Perm::Pos.spo(back).into())
}

// ---- warm store ----------------------------------------------------------------------

/// Number of nodes in the warm store; each has one `knows` statement and one
/// inline `age` property.
pub const WARM_N: usize = 1_000;

/// IDs the write cases act on.
#[derive(Debug, Clone, Copy)]
pub struct WarmIds {
    /// `n0 knows n1`, with two annotations, one of them annotated again.
    pub annotated: Id,
    /// `n1 knows n2`.
    pub to_update: Id,
    /// `n2 knows n3`.
    pub to_annotate: Id,
}

/// Opens and loads the warm store: `WARM_N` nodes, a `knows` ring, inline
/// integer ages, and a small annotation tree on the first statement. Also
/// warms the dictionary entries the cases use.
pub fn warm(dir: &Path) -> (Database, WarmIds) {
    let db = open(dir);
    let mut stmts = Vec::with_capacity(WARM_N);
    {
        let mut w = db.write(Durability::Sync).expect("write");
        for i in 0..WARM_N {
            let id = w
                .insert(node(i), iri("knows"), node((i + 1) % WARM_N))
                .expect("insert knows")
                .expect("statement id");
            stmts.push(id);
            w.insert(node(i), iri("age"), Term::integer(20 + (i % 50) as i64))
                .expect("insert age");
        }
        let a = stmts[0];
        let src = w
            .annotate(a, iri("source"), iri("doc1"))
            .expect("annotate")
            .expect("annotation id");
        w.annotate(a, iri("confidence"), Term::double(0.9))
            .expect("annotate");
        w.annotate(src, iri("since"), iri("doc0"))
            .expect("annotate");
        // Terms used by the cases below, so no case pays for a first intern.
        w.intern(iri("doc2")).expect("intern");
        w.commit().expect("commit");
    }
    let ids = WarmIds {
        annotated: stmts[0],
        to_update: stmts[1],
        to_annotate: stmts[2],
    };
    (db, ids)
}

/// A fresh write transaction on the warm store. Opening it starts the
/// engine batch, so the measured call does not.
pub fn txn(db: &Database) -> WriteTxn<'_> {
    db.write(Durability::Ordered).expect("write")
}

#[inline(never)]
pub fn op_insert(w: &mut WriteTxn<'_>) -> Option<Id> {
    w.insert(node(3), iri("knows"), node(700)).expect("insert")
}

#[inline(never)]
pub fn op_update(w: &mut WriteTxn<'_>, ids: WarmIds) {
    w.update(ids.to_update, node(500), None).expect("update")
}

#[inline(never)]
pub fn op_annotate(w: &mut WriteTxn<'_>, ids: WarmIds) -> Option<Id> {
    w.annotate(ids.to_annotate, iri("source"), iri("doc2"))
        .expect("annotate")
}

/// Deletes the statement with its annotation tree (four statements).
#[inline(never)]
pub fn op_cascade_delete(w: &mut WriteTxn<'_>, ids: WarmIds) -> Vec<Id> {
    let gone = w
        .delete(ids.annotated, DeleteMode::Cascade)
        .expect("delete");
    debug_assert!(gone.len() >= 3, "cascade removed {gone:?}");
    gone
}

/// Promotes the inline property `n4 age 24` to a statement
/// (rule `dm-promote-on-annotate`).
#[inline(never)]
pub fn op_promote(w: &mut WriteTxn<'_>) -> Option<Id> {
    let id = w
        .statement_id(node(4), iri("age"), Term::integer(24))
        .expect("promote");
    debug_assert!(id.is_some());
    id
}

// ---- dictionary --------------------------------------------------------------------

/// An IRI that is in the warm store's dictionary.
pub fn known_iri() -> String {
    format!("{EX}n123")
}

/// An IRI that is not, in a known namespace.
pub fn fresh_iri() -> String {
    format!("{EX}fresh-node")
}

#[inline(never)]
pub fn dict_intern(dict: Dict, kv: &mut dyn KvWrite, iri: &str) -> Id {
    dict.intern_iri(kv, iri).expect("intern")
}

#[inline(never)]
pub fn dict_lookup(dict: Dict, r: &dyn Reader, iri: &str) -> Option<Id> {
    dict.lookup_iri(r.kv(), iri).expect("lookup")
}

/// Reverse lookup: ID to term.
#[inline(never)]
pub fn dict_decode(dict: Dict, r: &dyn Reader, id: Id) -> Term {
    dict.decode(r.kv(), id).expect("decode")
}

// ---- Leapfrog Triejoin -------------------------------------------------------------

/// Edges per predicate in the triangle fixture: three sorted runs of 10^4 IDs.
pub const TRIANGLE_N: usize = 10_000;

/// Loads `a_i p1 b_i`, `b_i p2 c_i`, `c_i p3 a_i` for `i < TRIANGLE_N`:
/// 10^4 triangles over three predicates.
pub fn triangles(dir: &Path) -> Database {
    let db = open(dir);
    {
        let mut w = db.write(Durability::Sync).expect("write");
        for i in 0..TRIANGLE_N {
            let (a, b, c) = (
                iri(&format!("a{i}")),
                iri(&format!("b{i}")),
                iri(&format!("c{i}")),
            );
            w.insert(a.clone(), iri("p1"), b.clone()).expect("insert");
            w.insert(b, iri("p2"), c.clone()).expect("insert");
            w.insert(c, iri("p3"), a).expect("insert");
        }
        w.commit().expect("commit");
    }
    db
}

/// The triangle `?a p1 ?b . ?b p2 ?c . ?c p3 ?a` as LFTJ parts, with the
/// one unbound input row.
pub fn triangle_parts(r: &dyn Reader) -> (Vec<Node>, Vec<Row>) {
    let p = |local: &str| {
        r.lookup(&iri(local))
            .expect("lookup")
            .expect("predicate is interned")
    };
    let tp = |s: usize, pred: &str, o: usize| {
        Node::Triple(TriplePat {
            s: Pt::Var(s),
            p: Pt::Const(p(pred)),
            o: Pt::Var(o),
            id: None,
            graph: GraphSpec::Union,
        })
    };
    let parts = vec![tp(0, "p1", 1), tp(1, "p2", 2), tp(2, "p3", 0)];
    (parts, vec![vec![id::NULL; 3]])
}

/// Runs the triangle join with Leapfrog Triejoin; returns the result rows.
#[inline(never)]
pub fn lftj_triangle(ex: &Exec<'_>, parts: &[Node], input: &[Row]) -> Vec<(usize, Row)> {
    let rows = lftj::try_join(ex, parts, input)
        .expect("join")
        .expect("the triangle is cyclic, so LFTJ runs");
    debug_assert_eq!(rows.len(), TRIANGLE_N);
    rows
}

// ---- queries ------------------------------------------------------------------------

/// A two-pattern query on the warm store, for the plan-cache-hit case.
pub const CACHED_QUERY: &str = "PREFIX ex: <http://ex.org/>
SELECT ?b ?age WHERE { ex:n7 ex:knows ?b . ?b ex:age ?age }";

/// A ten-pattern query on the warm store, for parse and lowering.
pub const TEN_PATTERNS: &str = "PREFIX ex: <http://ex.org/>
SELECT ?a ?e WHERE {
  ?a ex:knows ?b . ?b ex:knows ?c . ?c ex:knows ?d . ?d ex:knows ?e .
  ?a ex:age ?x1 . ?b ex:age ?x2 . ?c ex:age ?x3 . ?d ex:age ?x4 . ?e ex:age ?x5 .
  ?e ex:knows ex:n9 .
}";

/// Parses the query and lowers it to a plan.
#[inline(never)]
pub fn parse_and_plan(ex: &Exec<'_>, text: &str) -> graphlava::sparql::Planned {
    let q = graphlava::sparql::parse_query(text, None).expect("parse");
    graphlava::sparql::plan(ex, &q, &graphlava::QueryOptions::default()).expect("plan")
}

/// An executor over a read snapshot, with statistics loaded as
/// `Database::sparql` does.
pub fn exec(r: &dyn Reader) -> Exec<'_> {
    let mut ex = Exec::new(r.kv(), r.dict());
    ex.stats = Some(graphlava::query::plan::Stats::load(r.kv()).expect("stats"));
    ex
}
