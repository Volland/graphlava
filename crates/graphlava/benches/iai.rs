//! Instruction-count micro benchmarks (iai-callgrind): the pull-request gate
//! of adr 0020 ([[17-benchmarks#Micro benchmarks]]). The cases and their
//! bodies are shared with the criterion bench `micro.rs` via `common`.
//!
//! Needs Linux with valgrind and `iai-callgrind-runner` at the same version as
//! the `iai-callgrind` dev-dependency (`=0.16.1`):
//!
//! ```text
//! cargo install iai-callgrind-runner --version 0.16.1 --locked
//! cargo bench -p graphlava --bench iai -- --save-baseline=main
//! cargo bench -p graphlava --bench iai -- --baseline=main --callgrind-limits='ir=3%'
//! ```
//!
//! On other systems the bench compiles but `main` only prints a notice.
//!
//! Every case gets its fixture from a `setup` function, which iai-callgrind
//! runs outside the measured call; values are dropped after it returns. Each
//! case runs in its own process, so fixtures are leaked rather than shared.

#![cfg_attr(not(target_os = "linux"), allow(dead_code, unused_imports))]

mod common;

use std::hint::black_box;

use common::*;
use graphlava::dict::Dict;
use graphlava::id::Id;
use graphlava::query::exec::{Exec, Row};
use graphlava::query::ir::Node;
use graphlava::storage::overlay::Overlay;
use graphlava::{Database, ReadTxn, Reader, WriteTxn};
use iai_callgrind::{library_benchmark, library_benchmark_group};

// ---- fixtures (run outside the measurement) ----------------------------------

fn leak<T>(v: T) -> &'static T {
    Box::leak(Box::new(v))
}

fn warm_db(name: &str) -> (&'static Database, WarmIds) {
    let (db, ids) = warm(&bench_dir(name));
    (leak(db), ids)
}

fn warm_txn() -> (WriteTxn<'static>, WarmIds) {
    let (db, ids) = warm_db("iai-store");
    (txn(db), ids)
}

fn warm_read() -> (Dict, &'static ReadTxn<'static>) {
    let (db, _) = warm_db("iai-dict");
    let r = leak(db.read().expect("read"));
    (r.dict(), r)
}

fn warm_overlay() -> (Dict, Overlay<'static>) {
    let (d, r) = warm_read();
    (d, Overlay::new(r.kv()))
}

fn warm_known_id() -> (Dict, &'static ReadTxn<'static>, Id) {
    let (d, r) = warm_read();
    let id = dict_lookup(d, r, &known_iri()).expect("known IRI");
    (d, r, id)
}

fn triangle_fixture() -> (Exec<'static>, Vec<Node>, Vec<Row>) {
    let db = leak(triangles(&bench_dir("iai-lftj")));
    let r = leak(db.read().expect("read"));
    let (parts, input) = triangle_parts(r);
    (exec(r), parts, input)
}

fn cached_db() -> &'static Database {
    let (db, _) = warm_db("iai-plan-cache");
    db.sparql(CACHED_QUERY)
        .expect("first run fills the plan cache");
    db
}

fn warm_exec() -> Exec<'static> {
    let (db, _) = warm_db("iai-parse");
    exec(leak(db.read().expect("read")))
}

// ---- ID encode and decode ------------------------------------------------------

#[library_benchmark]
#[bench::integer(SAMPLES[0].1)]
#[bench::decimal(SAMPLES[1].1)]
#[bench::boolean(SAMPLES[2].1)]
#[bench::date_time(SAMPLES[3].1)]
#[bench::date(SAMPLES[4].1)]
#[bench::string(SAMPLES[5].1)]
#[bench::lang_string(SAMPLES[6].1)]
#[bench::double(SAMPLES[7].1)]
fn id_encode(s: Sample) -> Option<Id> {
    black_box(encode(s))
}

#[library_benchmark]
#[bench::integer(args = (SAMPLES[0].1), setup = encoded)]
#[bench::decimal(args = (SAMPLES[1].1), setup = encoded)]
#[bench::boolean(args = (SAMPLES[2].1), setup = encoded)]
#[bench::date_time(args = (SAMPLES[3].1), setup = encoded)]
#[bench::date(args = (SAMPLES[4].1), setup = encoded)]
#[bench::string(args = (SAMPLES[5].1), setup = encoded)]
#[bench::lang_string(args = (SAMPLES[6].1), setup = encoded)]
#[bench::double(args = (SAMPLES[7].1), setup = encoded)]
fn id_decode((s, id): (Sample, Id)) -> Decoded {
    black_box(decode(s, id))
}

#[library_benchmark]
#[bench::spo_pos_osp(SPO)]
fn composite_key(spo: [Id; 3]) -> ([Vec<u8>; 3], [Id; 3]) {
    black_box(composite_keys(spo))
}

library_benchmark_group!(
    name = encoding;
    benchmarks = id_encode, id_decode, composite_key
);

// ---- dictionary -------------------------------------------------------------------

#[library_benchmark]
#[bench::hit(args = (), setup = warm_overlay)]
fn dict_intern_hit((d, mut ov): (Dict, Overlay<'static>)) -> (Id, Overlay<'static>) {
    let id = dict_intern(d, &mut ov, &known_iri());
    (black_box(id), ov)
}

#[library_benchmark]
#[bench::miss(args = (), setup = warm_overlay)]
fn dict_intern_miss((d, mut ov): (Dict, Overlay<'static>)) -> (Id, Overlay<'static>) {
    let id = dict_intern(d, &mut ov, &fresh_iri());
    (black_box(id), ov)
}

#[library_benchmark]
#[bench::hit(args = (), setup = warm_read)]
fn dict_lookup_iri((d, r): (Dict, &'static ReadTxn<'static>)) -> Option<Id> {
    black_box(dict_lookup(d, r, &known_iri()))
}

#[library_benchmark]
#[bench::iri(args = (), setup = warm_known_id)]
fn dict_decode_id((d, r, id): (Dict, &'static ReadTxn<'static>, Id)) -> graphlava::Term {
    black_box(dict_decode(d, r, id))
}

library_benchmark_group!(
    name = dictionary;
    benchmarks = dict_intern_hit, dict_intern_miss, dict_lookup_iri, dict_decode_id
);

// ---- warm store writes ----------------------------------------------------------

#[library_benchmark]
#[bench::warm(args = (), setup = warm_txn)]
fn store_insert((mut w, _): (WriteTxn<'static>, WarmIds)) -> WriteTxn<'static> {
    black_box(op_insert(&mut w));
    w
}

#[library_benchmark]
#[bench::warm(args = (), setup = warm_txn)]
fn store_update((mut w, ids): (WriteTxn<'static>, WarmIds)) -> WriteTxn<'static> {
    op_update(&mut w, ids);
    w
}

#[library_benchmark]
#[bench::warm(args = (), setup = warm_txn)]
fn store_annotate((mut w, ids): (WriteTxn<'static>, WarmIds)) -> WriteTxn<'static> {
    black_box(op_annotate(&mut w, ids));
    w
}

#[library_benchmark]
#[bench::warm(args = (), setup = warm_txn)]
fn store_cascade_delete((mut w, ids): (WriteTxn<'static>, WarmIds)) -> WriteTxn<'static> {
    black_box(op_cascade_delete(&mut w, ids));
    w
}

#[library_benchmark]
#[bench::warm(args = (), setup = warm_txn)]
fn store_promote((mut w, _): (WriteTxn<'static>, WarmIds)) -> WriteTxn<'static> {
    black_box(op_promote(&mut w));
    w
}

library_benchmark_group!(
    name = store;
    benchmarks = store_insert, store_update, store_annotate, store_cascade_delete, store_promote
);

// ---- query -------------------------------------------------------------------------

#[library_benchmark]
#[bench::triangle_3x10k(args = (), setup = triangle_fixture)]
fn lftj_join((ex, parts, input): (Exec<'static>, Vec<Node>, Vec<Row>)) -> Vec<(usize, Row)> {
    black_box(lftj_triangle(&ex, &parts, &input))
}

#[library_benchmark]
#[bench::hit(args = (), setup = cached_db)]
fn plan_cache_hit(db: &'static Database) -> graphlava::QueryResult {
    black_box(db.sparql(CACHED_QUERY).expect("query"))
}

#[library_benchmark]
#[bench::ten_patterns(args = (), setup = warm_exec)]
fn sparql_parse_plan(ex: Exec<'static>) -> graphlava::sparql::Planned {
    black_box(parse_and_plan(&ex, TEN_PATTERNS))
}

library_benchmark_group!(
    name = query;
    benchmarks = lftj_join, plan_cache_hit, sparql_parse_plan
);

#[cfg(target_os = "linux")]
iai_callgrind::main!(
    library_benchmark_groups = encoding,
    dictionary,
    store,
    query
);

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "graphlava iai bench: skipped; iai-callgrind needs valgrind, which this bench \
         only runs on Linux. Use `--bench micro` for wall-clock numbers here."
    );
}
