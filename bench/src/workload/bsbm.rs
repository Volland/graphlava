//! BSBM 3.1 (Bizer, Schultz 2009) explore and explore-and-update mixes.
//!
//! Data: the official Java generator (`bench/scripts/fetch-bsbm.sh` writes
//! `bench/data/bsbm/pc<N>/dataset.nt` and, with `-ud`, `dataset_update.nt`).
//! The official test driver talks to a SPARQL endpoint; the engines here are
//! embedded, so the harness drives the mix itself: the explore query
//! templates below are instantiated with parameters drawn (seeded) from the
//! loaded data, in the BSBM explore mix order with Q6 removed (as in 3.1).
//! The update mix inserts the next chunk of `dataset_update.nt` (INSERT DATA)
//! and deletes one offer (DELETE WHERE) before every explore mix.
//! `%currentDate%` is fixed at 2008-06-20 (the generator's default date range).

use super::{files_in, load_engine, read_rdf};
use crate::adapter::{is_unsupported, Quad, Term};
use crate::run::{durability_matches, Ctx};
use crate::stats::{Hists, RunSeries};
use anyhow::{bail, Result};
use rand::rngs::SmallRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Instant;

#[derive(Clone, Debug, clap::Args)]
pub struct BsbmArgs {
    /// Product count of the dataset (pc<N> directory; 28 480 products ~ 10M triples).
    #[arg(long, default_value_t = 28480)]
    pub products: u64,
    /// Query mixes per run (after `--warmup-mixes`).
    #[arg(long, default_value_t = 100)]
    pub mixes: usize,
    #[arg(long, default_value_t = 10)]
    pub warmup_mixes: usize,
    /// Run the explore-and-update mix as well.
    #[arg(long, default_value_t = true)]
    pub update: bool,
}

const V: &str = "http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/";
const PFX: &str = "PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
PREFIX dc: <http://purl.org/dc/elements/1.1/>
PREFIX rev: <http://purl.org/stuff/rev#>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
";

const CURRENT_DATE: &str = "\"2008-06-20T00:00:00\"^^xsd:dateTime";

/// Explore mix order (BSBM 3.1, Q6 removed).
pub const EXPLORE_MIX: [u8; 21] = [1, 2, 2, 3, 2, 2, 4, 2, 2, 5, 2, 2, 7, 7, 8, 9, 9, 10, 10, 11, 12];

fn template(q: u8) -> &'static str {
    match q {
        1 => "SELECT DISTINCT ?product ?label WHERE { ?product rdfs:label ?label . ?product a %ProductType% . ?product bsbm:productFeature %ProductFeature1% . ?product bsbm:productFeature %ProductFeature2% . ?product bsbm:productPropertyNumeric1 ?value1 . FILTER (?value1 > %x%) } ORDER BY ?label LIMIT 10",
        2 => "SELECT ?label ?comment ?producer ?productFeature ?propertyTextual1 ?propertyTextual2 ?propertyTextual3 ?propertyNumeric1 ?propertyNumeric2 ?propertyTextual4 ?propertyTextual5 ?propertyNumeric4 WHERE { %ProductXYZ% rdfs:label ?label . %ProductXYZ% rdfs:comment ?comment . %ProductXYZ% bsbm:producer ?p . ?p rdfs:label ?producer . %ProductXYZ% dc:publisher ?p . %ProductXYZ% bsbm:productFeature ?f . ?f rdfs:label ?productFeature . %ProductXYZ% bsbm:productPropertyTextual1 ?propertyTextual1 . %ProductXYZ% bsbm:productPropertyTextual2 ?propertyTextual2 . %ProductXYZ% bsbm:productPropertyTextual3 ?propertyTextual3 . %ProductXYZ% bsbm:productPropertyNumeric1 ?propertyNumeric1 . %ProductXYZ% bsbm:productPropertyNumeric2 ?propertyNumeric2 . OPTIONAL { %ProductXYZ% bsbm:productPropertyTextual4 ?propertyTextual4 } OPTIONAL { %ProductXYZ% bsbm:productPropertyTextual5 ?propertyTextual5 } OPTIONAL { %ProductXYZ% bsbm:productPropertyNumeric4 ?propertyNumeric4 } }",
        3 => "SELECT ?product ?label WHERE { ?product rdfs:label ?label . ?product a %ProductType% . ?product bsbm:productFeature %ProductFeature1% . ?product bsbm:productPropertyNumeric1 ?p1 . FILTER (?p1 > %x%) ?product bsbm:productPropertyNumeric3 ?p3 . FILTER (?p3 < %y%) OPTIONAL { ?product bsbm:productFeature %ProductFeature2% . ?product rdfs:label ?testVar } FILTER (!bound(?testVar)) } ORDER BY ?label LIMIT 10",
        4 => "SELECT DISTINCT ?product ?label ?propertyTextual WHERE { { ?product rdfs:label ?label . ?product rdf:type %ProductType% . ?product bsbm:productFeature %ProductFeature1% . ?product bsbm:productFeature %ProductFeature2% . ?product bsbm:productPropertyTextual1 ?propertyTextual . ?product bsbm:productPropertyNumeric1 ?p1 . FILTER (?p1 > %x%) } UNION { ?product rdfs:label ?label . ?product rdf:type %ProductType% . ?product bsbm:productFeature %ProductFeature1% . ?product bsbm:productFeature %ProductFeature3% . ?product bsbm:productPropertyTextual1 ?propertyTextual . ?product bsbm:productPropertyNumeric2 ?p2 . FILTER (?p2 > %y%) } } ORDER BY ?label OFFSET 5 LIMIT 10",
        5 => "SELECT DISTINCT ?product ?productLabel WHERE { ?product rdfs:label ?productLabel . FILTER (%ProductXYZ% != ?product) %ProductXYZ% bsbm:productFeature ?prodFeature . ?product bsbm:productFeature ?prodFeature . %ProductXYZ% bsbm:productPropertyNumeric1 ?origProperty1 . ?product bsbm:productPropertyNumeric1 ?simProperty1 . FILTER (?simProperty1 < (?origProperty1 + 120) && ?simProperty1 > (?origProperty1 - 120)) %ProductXYZ% bsbm:productPropertyNumeric2 ?origProperty2 . ?product bsbm:productPropertyNumeric2 ?simProperty2 . FILTER (?simProperty2 < (?origProperty2 + 170) && ?simProperty2 > (?origProperty2 - 170)) } ORDER BY ?productLabel LIMIT 5",
        7 => "SELECT ?productLabel ?offer ?price ?vendor ?vendorTitle ?review ?revTitle ?reviewer ?revName ?rating1 ?rating2 WHERE { %ProductXYZ% rdfs:label ?productLabel . OPTIONAL { ?offer bsbm:product %ProductXYZ% . ?offer bsbm:price ?price . ?offer bsbm:vendor ?vendor . ?vendor rdfs:label ?vendorTitle . ?vendor bsbm:country <http://downlode.org/rdf/iso-3166/countries#DE> . ?offer dc:publisher ?vendor . ?offer bsbm:validTo ?date . FILTER (?date > %currentDate%) } OPTIONAL { ?review bsbm:reviewFor %ProductXYZ% . ?review rev:reviewer ?reviewer . ?reviewer foaf:name ?revName . ?review dc:title ?revTitle . OPTIONAL { ?review bsbm:rating1 ?rating1 . } OPTIONAL { ?review bsbm:rating2 ?rating2 . } } }",
        8 => "SELECT ?title ?text ?reviewDate ?reviewer ?reviewerName ?rating1 ?rating2 ?rating3 ?rating4 WHERE { ?review bsbm:reviewFor %ProductXYZ% . ?review dc:title ?title . ?review rev:text ?text . FILTER langMatches(lang(?text), \"EN\") ?review bsbm:reviewDate ?reviewDate . ?review rev:reviewer ?reviewer . ?reviewer foaf:name ?reviewerName . OPTIONAL { ?review bsbm:rating1 ?rating1 . } OPTIONAL { ?review bsbm:rating2 ?rating2 . } OPTIONAL { ?review bsbm:rating3 ?rating3 . } OPTIONAL { ?review bsbm:rating4 ?rating4 . } } ORDER BY DESC(?reviewDate) LIMIT 20",
        9 => "DESCRIBE ?x WHERE { %ReviewXYZ% rev:reviewer ?x }",
        10 => "SELECT DISTINCT ?offer ?price WHERE { ?offer bsbm:product %ProductXYZ% . ?offer bsbm:vendor ?vendor . ?offer dc:publisher ?vendor . ?vendor bsbm:country <http://downlode.org/rdf/iso-3166/countries#US> . ?offer bsbm:deliveryDays ?deliveryDays . FILTER (?deliveryDays <= 3) ?offer bsbm:price ?price . ?offer bsbm:validTo ?date . FILTER (?date > %currentDate%) } ORDER BY xsd:double(str(?price)) LIMIT 10",
        11 => "SELECT ?property ?hasValue ?isValueOf WHERE { { %OfferXYZ% ?property ?hasValue } UNION { ?isValueOf ?property %OfferXYZ% } }",
        _ => "CONSTRUCT { %OfferXYZ% bsbm:product ?productURI . ?productURI rdfs:label ?productlabel . %OfferXYZ% bsbm:vendor ?vendorURI . ?vendorURI rdfs:label ?vendorname . ?vendorURI foaf:homepage ?vendorhomepage . %OfferXYZ% bsbm:price ?price . %OfferXYZ% bsbm:deliveryDays ?deliveryDays . %OfferXYZ% bsbm:validTo ?validTo } WHERE { %OfferXYZ% bsbm:product ?productURI . ?productURI rdfs:label ?productlabel . %OfferXYZ% bsbm:vendor ?vendorURI . ?vendorURI rdfs:label ?vendorname . ?vendorURI foaf:homepage ?vendorhomepage . %OfferXYZ% bsbm:offerWebpage ?offerURL . %OfferXYZ% bsbm:price ?price . %OfferXYZ% bsbm:deliveryDays ?deliveryDays . %OfferXYZ% bsbm:validTo ?validTo }",
    }
}

/// Parameter pools drawn from the data.
#[derive(Debug, Default)]
pub struct Pools {
    products: Vec<String>,
    types: Vec<String>,
    features: Vec<String>,
    offers: Vec<String>,
    reviews: Vec<String>,
}

pub fn pools(quads: &[Quad]) -> Pools {
    let mut p = Pools::default();
    let (pf, ty, prod, rf) = (format!("{V}productFeature"), "http://www.w3.org/1999/02/22-rdf-syntax-ns#type", format!("{V}product"), format!("{V}reviewFor"));
    let mut prods = std::collections::BTreeSet::new();
    for q in quads {
        let (Term::Iri(s), Term::Iri(pr)) = (&q.s, &q.p) else { continue };
        if *pr == pf {
            prods.insert(s.clone());
            if let Term::Iri(o) = &q.o {
                p.features.push(o.clone());
            }
        } else if *pr == prod {
            p.offers.push(s.clone());
        } else if *pr == rf {
            p.reviews.push(s.clone());
        } else if pr == ty {
            if let Term::Iri(o) = &q.o {
                if o.contains("/instances/ProductType") {
                    p.types.push(o.clone());
                }
            }
        }
    }
    p.products = prods.into_iter().collect();
    for v in [&mut p.features, &mut p.types, &mut p.offers, &mut p.reviews] {
        v.sort();
        v.dedup();
    }
    p
}

fn pick(rng: &mut SmallRng, v: &[String]) -> String {
    format!("<{}>", v.choose(rng).map_or("urn:none", |s| s.as_str()))
}

pub fn instantiate(q: u8, p: &Pools, rng: &mut SmallRng) -> String {
    let t = template(q)
        .replace("%ProductType%", &pick(rng, &p.types))
        .replace("%ProductFeature1%", &pick(rng, &p.features))
        .replace("%ProductFeature2%", &pick(rng, &p.features))
        .replace("%ProductFeature3%", &pick(rng, &p.features))
        .replace("%ProductXYZ%", &pick(rng, &p.products))
        .replace("%OfferXYZ%", &pick(rng, &p.offers))
        .replace("%ReviewXYZ%", &pick(rng, &p.reviews))
        .replace("%x%", &rng.gen_range(1..500).to_string())
        .replace("%y%", &rng.gen_range(1..500).to_string())
        .replace("%currentDate%", CURRENT_DATE);
    format!("{PFX}{t}")
}

pub fn run(c: &Ctx, a: &BsbmArgs) -> Result<()> {
    let dir = crate::run::bench_dir().join("data").join("bsbm").join(format!("pc{}", a.products));
    let main = dir.join("dataset.nt");
    if !main.exists() {
        bail!("{} not found: run bench/scripts/fetch-bsbm.sh {}", main.display(), a.products);
    }
    let quads = read_rdf(&[main], None)?;
    let upd = dir.join("dataset_update.nt");
    let updates: Vec<String> = if a.update && upd.exists() {
        // Chunks of the update stream as INSERT DATA bodies.
        let text = std::fs::read_to_string(&upd)?;
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        lines.chunks(lines.len().div_ceil(a.mixes.max(1) * 2).max(1)).map(|c| c.join("\n")).collect()
    } else {
        Vec::new()
    };
    let _ = files_in(&dir, &["nt"])?;
    let pools = pools(&quads);
    eprintln!("[bsbm] {} quads, {} products, {} offers", quads.len(), pools.products.len(), pools.offers.len());
    let mut out = Vec::new();
    for &e in &c.c.adapters {
        let (ad, d, load) = match load_engine(c, e, &format!("bsbm{}", a.products), &quads) {
            Ok(x) => x,
            Err(x) => {
                out.push(json!({"adapter": e.name(), "skipped": format!("{x:#}")}));
                continue;
            }
        };
        let mut mixes_out = BTreeMap::new();
        for mode in if a.update { vec!["explore", "explore-and-update"] } else { vec!["explore"] } {
            let mut series = RunSeries::default();
            let mut runs = Vec::new();
            let mut err: Option<String> = None;
            let mut upd_i = 0;
            for run in 0..c.c.runs {
                let mut s = ad.session()?;
                let mut rng = SmallRng::seed_from_u64(c.c.seed + run as u64);
                let mut h = Hists::default();
                let mut t0 = Instant::now();
                let mut timed = 0;
                for m in 0..(a.warmup_mixes + a.mixes) {
                    let rec = m >= a.warmup_mixes;
                    if rec && timed == 0 {
                        timed = 1;
                        t0 = Instant::now();
                    }
                    if mode == "explore-and-update" {
                        if let Some(body) = updates.get(upd_i % updates.len().max(1)) {
                            upd_i += 1;
                            let t = Instant::now();
                            let r = s.sparql_update(&format!("INSERT DATA {{ {body} }}"), c.c.durability);
                            if rec { h.rec("update_insert", t.elapsed().as_nanos() as u64); }
                            if let Err(x) = r { err.get_or_insert(format!("insert: {x:#}")); }
                        }
                        let off = pick(&mut rng, &pools.offers);
                        let t = Instant::now();
                        let r = s.sparql_update(&format!("DELETE WHERE {{ {off} ?p ?o }}"), c.c.durability);
                        if rec { h.rec("update_delete", t.elapsed().as_nanos() as u64); }
                        if let Err(x) = r { err.get_or_insert(format!("delete: {x:#}")); }
                    }
                    for &q in &EXPLORE_MIX {
                        let text = instantiate(q, &pools, &mut rng);
                        let t = Instant::now();
                        let r = s.sparql(&text);
                        if rec { h.rec(&format!("Q{q}"), t.elapsed().as_nanos() as u64); }
                        if let Err(x) = r {
                            err.get_or_insert(if is_unsupported(&x) { "unsupported".into() } else { format!("Q{q}: {x:#}") });
                        }
                    }
                    if err.as_deref() == Some("unsupported") { break; }
                }
                let _ = timed;
                let secs = t0.elapsed().as_secs_f64();
                series.push_hists(&h);
                series.push("qmph", a.mixes as f64 / (secs / 3600.0));
                runs.push(json!({"run": run, "latency": h.to_json(), "wall_s": secs}));
                if err.as_deref() == Some("unsupported") { break; }
            }
            mixes_out.insert(mode, json!({"summary": series.to_json(), "first_error": err, "runs": runs}));
        }
        out.push(json!({"adapter": e.name(), "describe": ad.describe(), "load": load,
            "durability_matches": durability_matches(e, c.c.durability), "mixes": mixes_out}));
        drop(ad);
        c.cleanup(&d);
    }
    c.save("bsbm", json!({"products": a.products, "mixes": a.mixes, "warmup_mixes": a.warmup_mixes,
        "explore_mix": EXPLORE_MIX, "update_chunks": updates.len(), "current_date": CURRENT_DATE}), json!({"adapters": out}))
}
