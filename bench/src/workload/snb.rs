//! SNB-style traversal workload ([[17-benchmarks#Traversal workload]]): an own
//! seeded social-graph generator shaped like LDBC SNB Interactive, the short
//! reads IS1–IS7 in SPARQL (and IS1, IS3, IS4, IS5, IS6 also in Datalog for
//! graphlava), and inserts (person, knows, post, comment) as SPARQL updates
//! from one writer while 1–16 readers run the short reads.
//!
//! Generator `snb-lite-v1` (scale factor SF): persons = 10 000·SF; knows degree
//! 1 + floor(40·u²) per person (LDBC RDF shape: `p knows k . k hasPerson q ;
//! creationDate d`, both directions); one forum per person (moderator); posts
//! 0–10 per person in their forum; comments 0–20 per person replying to a
//! random earlier message. Not the LDBC datagen: no correlations, no
//! places/tags/organisations beyond a city per person. LDBC's own SF1 has
//! ~10k persons and ~3M messages; this generator's message counts are ~10x lower.
//! Not measured: planner versus executor time share (no hook in the public API).

use super::{load_engine, Lang, Query};
use crate::adapter::{is_unsupported, Quad, Term};
use crate::run::Ctx;
use crate::stats::{Hists, RunSeries};
use anyhow::Result;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub const SNB_VERSION: &str = "snb-lite-v1";
const S: &str = "http://bench.graphlava.org/snb/";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

#[derive(Clone, Debug, clap::Args)]
pub struct SnbArgs {
    /// Scale factor (persons = 10 000 x SF).
    #[arg(long, default_value_t = 1.0)]
    pub sf: f64,
    /// Reader thread counts.
    #[arg(long, value_delimiter = ',', default_value = "1,4,16")]
    pub readers: Vec<usize>,
    /// Short reads per reader per run.
    #[arg(long, default_value_t = 2000)]
    pub reads: u64,
    /// Writer insert rate (updates/s, open loop); 0 disables the writer.
    #[arg(long, default_value_t = 50.0)]
    pub writer_rate: f64,
}

fn v(p: &str) -> Term {
    Term::Iri(format!("{S}{p}"))
}
fn ent(kind: &str, i: u64) -> Term {
    Term::Iri(format!("{S}{kind}/{i}"))
}

const FIRST: [&str; 8] = ["Ana", "Bo", "Chen", "Dara", "Eli", "Femi", "Gus", "Hana"];
const LAST: [&str; 8] = ["Ito", "Jones", "Kim", "Lopez", "Moss", "Nagy", "Ortiz", "Park"];

/// Generated graph plus its entity counts.
pub struct Snb {
    pub quads: Vec<Quad>,
    pub persons: u64,
    pub messages: u64,
    pub posts: u64,
}

pub fn generate(sf: f64, seed: u64) -> Snb {
    let mut rng = SmallRng::seed_from_u64(seed);
    let persons = ((10_000.0 * sf) as u64).max(10);
    let mut q = Vec::new();
    let mut add = |s: Term, p: Term, o: Term| q.push(Quad { s, p, o, g: None });
    let ty = Term::Iri(RDF_TYPE.into());
    let date = |d: i64| Term::typed(crate::suite::tasks::date_time(d), "dateTime");
    for i in 0..persons {
        let p = ent("person", i);
        add(p.clone(), ty.clone(), v("Person"));
        add(p.clone(), v("firstName"), Term::string(FIRST[rng.gen_range(0..8)]));
        add(p.clone(), v("lastName"), Term::string(LAST[rng.gen_range(0..8)]));
        add(p.clone(), v("gender"), Term::string(if rng.gen_bool(0.5) { "male" } else { "female" }));
        add(p.clone(), v("birthday"), Term::typed(format!("19{:02}-0{}-1{}", rng.gen_range(50..99), rng.gen_range(1..9), rng.gen_range(0..9)), "date"));
        add(p.clone(), v("creationDate"), date(rng.gen_range(0..30_000_000)));
        add(p.clone(), v("locationIP"), Term::string(format!("10.{}.{}.{}", rng.gen_range(0..255), rng.gen_range(0..255), rng.gen_range(0..255))));
        add(p.clone(), v("browserUsed"), Term::string(["Firefox", "Chrome", "Safari"][rng.gen_range(0..3)]));
        add(p.clone(), v("isLocatedIn"), ent("city", rng.gen_range(0..100)));
        let f = ent("forum", i);
        add(f.clone(), ty.clone(), v("Forum"));
        add(f.clone(), v("title"), Term::string(format!("Wall of person {i}")));
        add(f, v("hasModerator"), p);
    }
    let mut k = 0u64;
    for i in 0..persons {
        let u: f64 = rng.gen();
        let deg = 1 + (40.0 * u * u) as u64;
        for _ in 0..deg {
            let j = rng.gen_range(0..persons);
            if j == i {
                continue;
            }
            let d = date(rng.gen_range(30_000_000..60_000_000));
            for (a, b) in [(i, j), (j, i)] {
                let kn = ent("knows", k);
                k += 1;
                add(ent("person", a), v("knows"), kn.clone());
                add(kn.clone(), v("hasPerson"), ent("person", b));
                add(kn, v("creationDate"), d.clone());
            }
        }
    }
    let mut m = 0u64;
    let mut posts = 0u64;
    for i in 0..persons {
        for _ in 0..rng.gen_range(0..=10) {
            let msg = ent("message", m);
            add(msg.clone(), ty.clone(), v("Post"));
            add(ent("forum", i), v("containerOf"), msg.clone());
            add(msg.clone(), v("hasCreator"), ent("person", i));
            add(msg.clone(), v("content"), Term::string(format!("post {m} by {i}")));
            add(msg.clone(), v("length"), Term::int(rng.gen_range(10..200)));
            add(msg, v("creationDate"), date(60_000_000 + m as i64));
            m += 1;
            posts += 1;
        }
    }
    for i in 0..persons {
        for _ in 0..rng.gen_range(0..=20) {
            if m == 0 {
                break;
            }
            let target = rng.gen_range(0..m);
            let msg = ent("message", m);
            add(msg.clone(), ty.clone(), v("Comment"));
            add(msg.clone(), v("replyOf"), ent("message", target));
            add(msg.clone(), v("hasCreator"), ent("person", i));
            add(msg.clone(), v("content"), Term::string(format!("comment {m} by {i}")));
            add(msg.clone(), v("length"), Term::int(rng.gen_range(5..100)));
            add(msg, v("creationDate"), date(60_000_000 + m as i64));
            m += 1;
        }
    }
    Snb { quads: q, persons, messages: m, posts }
}

const PFX: &str = "PREFIX snb: <http://bench.graphlava.org/snb/>\n";
const DPFX: &str = "@prefix snb: <http://bench.graphlava.org/snb/> .\n@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n";

/// One short read with fresh parameters.
pub fn short_read(rng: &mut SmallRng, g: &Snb, datalog: bool) -> Query {
    let p = format!("<{S}person/{}>", rng.gen_range(0..g.persons));
    let m = format!("<{S}message/{}>", rng.gen_range(0..g.messages.max(1)));
    let which = rng.gen_range(1..=7);
    let dl = datalog && matches!(which, 1 | 3 | 4 | 5 | 6);
    let (lang, text) = if dl {
        let body = match which {
            1 => format!("q(?f, ?l, ?b, ?ip, ?br, ?c, ?g, ?d) :- triple({p}, snb:firstName, ?f), triple({p}, snb:lastName, ?l), triple({p}, snb:birthday, ?b), triple({p}, snb:locationIP, ?ip), triple({p}, snb:browserUsed, ?br), triple({p}, snb:isLocatedIn, ?c), triple({p}, snb:gender, ?g), triple({p}, snb:creationDate, ?d) .\n?- q(?f, ?l, ?b, ?ip, ?br, ?c, ?g, ?d) ."),
            3 => format!("q(?x, ?f, ?l, ?d) :- triple({p}, snb:knows, ?k), triple(?k, snb:hasPerson, ?x), triple(?k, snb:creationDate, ?d), triple(?x, snb:firstName, ?f), triple(?x, snb:lastName, ?l) .\n?- q(?x, ?f, ?l, ?d) ."),
            4 => format!("q(?d, ?c) :- triple({m}, snb:creationDate, ?d), triple({m}, snb:content, ?c) .\n?- q(?d, ?c) ."),
            5 => format!("q(?x, ?f, ?l) :- triple({m}, snb:hasCreator, ?x), triple(?x, snb:firstName, ?f), triple(?x, snb:lastName, ?l) .\n?- q(?x, ?f, ?l) ."),
            _ => format!("up(?x, ?x) :- triple(?x, rdf:type, snb:Post) .\nup(?x, ?y) :- triple(?x, snb:replyOf, ?z), up(?z, ?y) .\nq(?fo, ?t, ?mo, ?f, ?l) :- up({m}, ?post), triple(?fo, snb:containerOf, ?post), triple(?fo, snb:title, ?t), triple(?fo, snb:hasModerator, ?mo), triple(?mo, snb:firstName, ?f), triple(?mo, snb:lastName, ?l) .\n?- q(?fo, ?t, ?mo, ?f, ?l) ."),
        };
        (Lang::Datalog, format!("{DPFX}{body}"))
    } else {
        let body = match which {
            1 => format!("SELECT ?f ?l ?b ?ip ?br ?c ?g ?d WHERE {{ {p} snb:firstName ?f ; snb:lastName ?l ; snb:birthday ?b ; snb:locationIP ?ip ; snb:browserUsed ?br ; snb:isLocatedIn ?c ; snb:gender ?g ; snb:creationDate ?d }}"),
            2 => format!("SELECT ?msg ?content ?date ?post ?op WHERE {{ ?msg snb:hasCreator {p} ; snb:creationDate ?date ; snb:content ?content . OPTIONAL {{ ?msg snb:replyOf* ?post . ?post a snb:Post ; snb:hasCreator ?op }} }} ORDER BY DESC(?date) DESC(?msg) LIMIT 10"),
            3 => format!("SELECT ?x ?f ?l ?d WHERE {{ {p} snb:knows ?k . ?k snb:hasPerson ?x ; snb:creationDate ?d . ?x snb:firstName ?f ; snb:lastName ?l }} ORDER BY DESC(?d) ?x"),
            4 => format!("SELECT ?d ?c WHERE {{ {m} snb:creationDate ?d ; snb:content ?c }}"),
            5 => format!("SELECT ?x ?f ?l WHERE {{ {m} snb:hasCreator ?x . ?x snb:firstName ?f ; snb:lastName ?l }}"),
            6 => format!("SELECT ?fo ?t ?mo ?f ?l WHERE {{ {m} snb:replyOf* ?post . ?post a snb:Post . ?fo snb:containerOf ?post ; snb:title ?t ; snb:hasModerator ?mo . ?mo snb:firstName ?f ; snb:lastName ?l }}"),
            _ => format!("SELECT ?c ?content ?date ?a ?f ?l ?knows WHERE {{ ?c snb:replyOf {m} ; snb:content ?content ; snb:creationDate ?date ; snb:hasCreator ?a . ?a snb:firstName ?f ; snb:lastName ?l . {m} snb:hasCreator ?orig . BIND(EXISTS {{ ?orig snb:knows ?k . ?k snb:hasPerson ?a }} AS ?knows) }} ORDER BY DESC(?date) ?a"),
        };
        (Lang::Sparql, format!("{PFX}{body}"))
    };
    Query { template: format!("IS{which}{}", if dl { "_datalog" } else { "" }), lang, text }
}

/// One insert operation (IU-style) as a SPARQL update.
pub fn insert_op(rng: &mut SmallRng, g: &Snb, n: u64) -> (&'static str, String) {
    let p = rng.gen_range(0..g.persons);
    let d = crate::suite::tasks::date_time(90_000_000 + n as i64);
    match rng.gen_range(0..4) {
        0 => ("IU_person", format!("{PFX}INSERT DATA {{ <{S}person/new{n}> a snb:Person ; snb:firstName \"New\" ; snb:lastName \"Person{n}\" ; snb:creationDate \"{d}\"^^<http://www.w3.org/2001/XMLSchema#dateTime> }}")),
        1 => ("IU_knows", format!("{PFX}INSERT DATA {{ <{S}person/{p}> snb:knows <{S}knows/new{n}> . <{S}knows/new{n}> snb:hasPerson <{S}person/{}> ; snb:creationDate \"{d}\"^^<http://www.w3.org/2001/XMLSchema#dateTime> }}", rng.gen_range(0..g.persons))),
        2 => ("IU_post", format!("{PFX}INSERT DATA {{ <{S}message/new{n}> a snb:Post ; snb:hasCreator <{S}person/{p}> ; snb:content \"new post {n}\" ; snb:creationDate \"{d}\"^^<http://www.w3.org/2001/XMLSchema#dateTime> . <{S}forum/{p}> snb:containerOf <{S}message/new{n}> }}")),
        _ => ("IU_comment", format!("{PFX}INSERT DATA {{ <{S}message/new{n}> a snb:Comment ; snb:replyOf <{S}message/{}> ; snb:hasCreator <{S}person/{p}> ; snb:content \"new comment {n}\" ; snb:creationDate \"{d}\"^^<http://www.w3.org/2001/XMLSchema#dateTime> }}", rng.gen_range(0..g.messages.max(1)))),
    }
}

pub fn run(c: &Ctx, a: &SnbArgs) -> Result<()> {
    let g = generate(a.sf, c.c.seed);
    eprintln!("[snb] {} persons, {} messages ({} posts), {} quads", g.persons, g.messages, g.posts, g.quads.len());
    let mut out = Vec::new();
    for &e in &c.c.adapters {
        let (ad, d, load) = match load_engine(c, e, "snb", &g.quads) {
            Ok(x) => x,
            Err(x) => {
                out.push(json!({"adapter": e.name(), "skipped": format!("{x:#}")}));
                continue;
            }
        };
        let datalog = e == crate::adapter::Engine::Graphlava;
        let mut points = Vec::new();
        for &nr in &a.readers {
            let mut series = RunSeries::default();
            let mut runs = Vec::new();
            for run in 0..c.c.runs {
                let stop = AtomicBool::new(false);
                let errs = AtomicU64::new(0);
                let unsup = AtomicU64::new(0);
                let first_err = std::sync::Mutex::new(None::<String>);
                let t0 = Instant::now();
                let (rh, wh) = std::thread::scope(|sc| -> Result<(Hists, Hists)> {
                    let writer = (a.writer_rate > 0.0).then(|| {
                        let (ad, g, stop, errs) = (ad.as_ref(), &g, &stop, &errs);
                        sc.spawn(move || -> Result<Hists> {
                            let mut s = ad.session()?;
                            let mut rng = SmallRng::seed_from_u64(c.c.seed ^ 0xBEEF ^ run as u64);
                            let mut h = Hists::default();
                            let t0 = Instant::now();
                            let mut i = 0u64;
                            while !stop.load(Ordering::Relaxed) {
                                let t = t0 + Duration::from_secs_f64(i as f64 / a.writer_rate);
                                while Instant::now() < t && !stop.load(Ordering::Relaxed) {
                                    std::thread::sleep(Duration::from_micros(200));
                                }
                                let (cls, u) = insert_op(&mut rng, g, (run as u64) << 32 | i);
                                match s.sparql_update(&u, c.c.durability) {
                                    Ok(()) => h.rec(cls, (Instant::now() - t).as_nanos() as u64),
                                    Err(x) if is_unsupported(&x) => break,
                                    Err(_) => { errs.fetch_add(1, Ordering::Relaxed); }
                                }
                                i += 1;
                            }
                            Ok(h)
                        })
                    });
                    let readers: Vec<_> = (0..nr)
                        .map(|ri| {
                            let (ad, g, errs, unsup, first_err) = (ad.as_ref(), &g, &errs, &unsup, &first_err);
                            sc.spawn(move || -> Result<Hists> {
                                let mut s = ad.session()?;
                                let mut rng = SmallRng::seed_from_u64(c.c.seed + 1000 * run as u64 + ri as u64);
                                let mut h = Hists::default();
                                for _ in 0..a.reads {
                                    let dl = datalog && rng.gen_bool(0.5);
                                    let q = short_read(&mut rng, g, dl);
                                    let t = Instant::now();
                                    let r = match q.lang {
                                        Lang::Sparql => s.sparql(&q.text),
                                        Lang::Datalog => s.datalog(&q.text),
                                    };
                                    match r {
                                        Ok(_) => h.rec(&q.template, t.elapsed().as_nanos() as u64),
                                        Err(x) if is_unsupported(&x) => { unsup.fetch_add(1, Ordering::Relaxed); }
                                        Err(x) => {
                                            errs.fetch_add(1, Ordering::Relaxed);
                                            first_err.lock().expect("lock").get_or_insert(format!("{}: {x:#}", q.template));
                                        }
                                    }
                                }
                                Ok(h)
                            })
                        })
                        .collect();
                    let mut rh = Hists::default();
                    for r in readers {
                        rh.merge(&r.join().expect("reader")?);
                    }
                    stop.store(true, Ordering::Relaxed);
                    let wh = match writer {
                        Some(w) => w.join().expect("writer")?,
                        None => Hists::default(),
                    };
                    Ok((rh, wh))
                })?;
                let secs = t0.elapsed().as_secs_f64();
                series.push_hists(&rh);
                series.push_hists(&wh);
                series.push("read_ops_per_s", rh.all().len() as f64 / secs);
                runs.push(json!({"run": run, "readers": nr, "wall_s": secs, "reads": rh.to_json(), "writes": wh.to_json(),
                    "errors": errs.load(Ordering::Relaxed), "unsupported": unsup.load(Ordering::Relaxed),
                    "first_error": first_err.into_inner().expect("lock")}));
            }
            points.push(json!({"readers": nr, "summary": series.to_json(), "runs": runs}));
        }
        out.push(json!({"adapter": e.name(), "describe": ad.describe(), "load": load, "points": points,
            "planner_executor_share": null}));
        drop(ad);
        c.cleanup(&d);
    }
    c.save("snb-traversal", json!({"generator": SNB_VERSION, "sf": a.sf, "persons": g.persons, "messages": g.messages,
        "readers": a.readers, "reads": a.reads, "writer_rate": a.writer_rate}), json!({"adapters": out}))
}
