//! Rust side of [[17-benchmarks#Binding overhead]]: times the same calls as
//! `bench/overhead.py` on the same file, so the Python overhead is the
//! difference. Prints one JSON object with per-workload latency samples
//! summarised as percentiles and a log-linear (HDR-style) histogram.
//!
//! cargo run --release -p graphlava-py --example overhead_baseline -- <file> <nodes> <ops>

// @tg: verifies:: [[openspec:python-binding#Python call overhead]] {level: benchmark}

use std::time::Instant;

use graphlava::{
    CoreOp, Database, Durability, Lang, OpenOptions, Reader, Term, TermRef, WriteRequest,
};

const EX: &str = "http://ex.org/";

/// The node sequence both sides use: a 64-bit LCG from seed 42.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

fn iri(s: &str) -> Term {
    Term::iri(format!("{EX}{s}"))
}

fn summary(name: &str, mut ns: Vec<u64>, per: u64) -> String {
    ns.sort_unstable();
    let q =
        |p: f64| ns[((ns.len() as f64 - 1.0) * p).round() as usize] as f64 / 1000.0 / per as f64;
    // log-linear buckets: 16 sub-buckets per power of two, in ns
    let mut hist = std::collections::BTreeMap::<u64, u64>::new();
    for v in &ns {
        let v = (*v).max(1);
        let e = 63 - v.leading_zeros() as u64;
        let sub = if e >= 4 { (v >> (e - 4)) & 15 } else { v & 15 };
        *hist.entry(e * 16 + sub).or_default() += 1;
    }
    let h: Vec<String> = hist.iter().map(|(k, c)| format!("[{k},{c}]")).collect();
    format!(
        "\"{name}\":{{\"n\":{},\"p50_us\":{:.3},\"p99_us\":{:.3},\"p999_us\":{:.3},\"hist_log16_ns\":[{}]}}",
        ns.len(),
        q(0.5),
        q(0.99),
        q(0.999),
        h.join(",")
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let nodes: u64 = args[2].parse().unwrap();
    let ops: usize = args[3].parse().unwrap();
    let db = Database::open(path, OpenOptions::default()).expect("open");
    let r = db.read().unwrap();
    let knows = r.lookup(&iri("knows")).unwrap().expect("knows");
    let mut rng = Lcg(42);
    let ids: Vec<_> = (0..ops)
        .map(|_| {
            r.lookup(&iri(&format!("n{}", rng.next(nodes))))
                .unwrap()
                .expect("node")
        })
        .collect();
    let mut out = Vec::new();

    let mut s = Vec::with_capacity(ops);
    for id in &ids {
        let t = Instant::now();
        std::hint::black_box(r.props(*id).unwrap());
        s.push(t.elapsed().as_nanos() as u64);
    }
    out.push(summary("props", s, 1));

    let mut s = Vec::with_capacity(ops);
    for id in &ids {
        let t = Instant::now();
        for m in r.out(*id, Some(knows)).unwrap() {
            std::hint::black_box(r.out(m.o, Some(knows)).unwrap());
        }
        s.push(t.elapsed().as_nanos() as u64);
    }
    out.push(summary("out_2hop", s, 1));
    drop(r);

    let q = "PREFIX : <http://ex.org/> SELECT ?s ?age WHERE { ?s :age ?age } LIMIT 10000";
    let mut s = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let (txn, rows) = db.query_ids(Lang::Sparql, q).unwrap();
        for row in &rows.rows {
            for id in row {
                std::hint::black_box(txn.decode_row_id(&rows, *id).unwrap());
            }
        }
        s.push(t.elapsed().as_nanos() as u64 / rows.rows.len().max(1) as u64);
    }
    out.push(summary("select_10k_per_row", s, 1));

    for (name, d) in [
        ("submit_1_ordered", Durability::Ordered),
        ("commit_sync", Durability::Sync),
        ("commit_relaxed", Durability::Relaxed),
    ] {
        let n = if d == Durability::Sync {
            ops.min(500)
        } else {
            ops.min(2000)
        };
        let mut s = Vec::with_capacity(n);
        for i in 0..n {
            let req = WriteRequest {
                ops: vec![CoreOp::Set {
                    s: iri(&format!("n{}", i as u64 % nodes)).into(),
                    p: iri("tick").into(),
                    o: TermRef::Term(Term::integer(i as i64)),
                }],
            };
            let t = Instant::now();
            db.submit(req, d).unwrap();
            s.push(t.elapsed().as_nanos() as u64);
        }
        out.push(summary(name, s, 1));
    }
    db.close().unwrap();
    println!("{{{}}}", out.join(","));
}
