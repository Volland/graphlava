//! Wall-clock micro benchmarks (criterion) for local runs
//! ([[17-benchmarks#Micro benchmarks]]). The pull-request gate counts
//! instructions for the same cases in `iai.rs`; see adr 0020.
//!
//! ```text
//! cargo bench -p graphlava --bench micro             # full run
//! cargo bench -p graphlava --bench micro -- --quick  # a few seconds per case
//! cargo bench -p graphlava --bench micro -- dict/    # one group
//! ```

mod common;

use std::hint::black_box;
use std::time::Duration;

use common::*;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use graphlava::Reader;
use graphlava::storage::overlay::Overlay;

fn ids(c: &mut Criterion) {
    let mut g = c.benchmark_group("id");
    for (name, s) in SAMPLES {
        g.bench_function(format!("encode/{name}"), |b| {
            b.iter(|| encode(black_box(s)))
        });
        let (s, id) = encoded(s);
        g.bench_function(format!("decode/{name}"), |b| {
            b.iter(|| decode(black_box(s), black_box(id)))
        });
    }
    g.finish();
}

fn keys(c: &mut Criterion) {
    c.bench_function("key/composite_spo_pos_osp", |b| {
        b.iter(|| composite_keys(black_box(SPO)))
    });
}

fn dict(c: &mut Criterion) {
    let (db, _) = warm(&bench_dir("crit-dict"));
    let r = db.read().expect("read");
    let d = r.dict();
    let (known, fresh) = (known_iri(), fresh_iri());
    let known_id = dict_lookup(d, &r, &known).expect("known IRI");
    let mut g = c.benchmark_group("dict");
    g.bench_function("intern_hit", |b| {
        let mut ov = Overlay::new(r.kv());
        b.iter(|| dict_intern(d, &mut ov, black_box(&known)))
    });
    g.bench_function("intern_miss", |b| {
        b.iter_batched(
            || Overlay::new(r.kv()),
            |mut ov| {
                dict_intern(d, &mut ov, black_box(&fresh));
                ov
            },
            BatchSize::SmallInput,
        )
    });
    g.bench_function("lookup", |b| {
        b.iter(|| dict_lookup(d, &r, black_box(&known)))
    });
    g.bench_function("decode", |b| {
        b.iter(|| dict_decode(d, &r, black_box(known_id)))
    });
    g.finish();
}

fn store(c: &mut Criterion) {
    let (db, w) = warm(&bench_dir("crit-store"));
    let mut g = c.benchmark_group("store");
    // One write transaction per iteration (the store is single-writer), set
    // up and dropped (aborted) outside the measurement.
    g.bench_function("insert", |b| {
        b.iter_batched(
            || txn(&db),
            |mut t| {
                op_insert(&mut t);
                t
            },
            BatchSize::PerIteration,
        )
    });
    g.bench_function("update", |b| {
        b.iter_batched(
            || txn(&db),
            |mut t| {
                op_update(&mut t, w);
                t
            },
            BatchSize::PerIteration,
        )
    });
    g.bench_function("annotate", |b| {
        b.iter_batched(
            || txn(&db),
            |mut t| {
                op_annotate(&mut t, w);
                t
            },
            BatchSize::PerIteration,
        )
    });
    g.bench_function("cascade_delete", |b| {
        b.iter_batched(
            || txn(&db),
            |mut t| {
                op_cascade_delete(&mut t, w);
                t
            },
            BatchSize::PerIteration,
        )
    });
    g.bench_function("promote", |b| {
        b.iter_batched(
            || txn(&db),
            |mut t| {
                op_promote(&mut t);
                t
            },
            BatchSize::PerIteration,
        )
    });
    g.finish();
}

fn query(c: &mut Criterion) {
    let db = triangles(&bench_dir("crit-lftj"));
    {
        let r = db.read().expect("read");
        let ex = exec(&r);
        let (parts, input) = triangle_parts(&r);
        c.bench_function("query/lftj_triangle_3x10k", |b| {
            b.iter(|| lftj_triangle(&ex, black_box(&parts), &input))
        });
    }
    drop(db);

    let (db, _) = warm(&bench_dir("crit-query"));
    db.sparql(CACHED_QUERY)
        .expect("first run fills the plan cache");
    c.bench_function("query/plan_cache_hit", |b| {
        b.iter(|| db.sparql(black_box(CACHED_QUERY)).expect("query"))
    });
    let r = db.read().expect("read");
    let ex = exec(&r);
    c.bench_function("query/sparql_parse_plan_10_patterns", |b| {
        b.iter(|| parse_and_plan(&ex, black_box(TEN_PATTERNS)))
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2));
    targets = ids, keys, dict, store, query
}
criterion_main!(benches);
