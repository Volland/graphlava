//! Standard workloads ([[17-benchmarks#Standard SPARQL workloads]],
//! [[17-benchmarks#Traversal workload]]) and the shared query-suite runner.

pub mod bsbm;
pub mod lubm;
pub mod snb;
pub mod watdiv;

use crate::adapter::{is_unsupported, Adapter, Engine, Quad, Term};
use crate::run::{durability_matches, Ctx};
use crate::stats::{Hists, RunSeries};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Query language of a query instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Sparql,
    Datalog,
}

/// One query instance of a template.
#[derive(Clone, Debug)]
pub struct Query {
    pub template: String,
    pub lang: Lang,
    pub text: String,
}

/// Parses RDF files (format from extension) into quads.
pub fn read_rdf(files: &[PathBuf], base: Option<&str>) -> Result<Vec<Quad>> {
    use oxigraph::io::{RdfFormat, RdfParser};
    use oxigraph::model as om;
    let mut out = Vec::new();
    for f in files {
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("");
        let fmt = match ext {
            "owl" | "rdf" | "xml" => RdfFormat::RdfXml,
            "nt" => RdfFormat::NTriples,
            "nq" => RdfFormat::NQuads,
            "ttl" => RdfFormat::Turtle,
            _ => RdfFormat::from_extension(ext).with_context(|| format!("unknown RDF format: {}", f.display()))?,
        };
        let mut p = RdfParser::from_format(fmt);
        if let Some(b) = base {
            p = p.with_base_iri(b)?;
        }
        let rd = std::io::BufReader::new(std::fs::File::open(f).with_context(|| f.display().to_string())?);
        for q in p.for_reader(rd) {
            let q = q.with_context(|| f.display().to_string())?;
            let conv = |t: om::Term| -> Option<Term> {
                Some(match t {
                    om::Term::NamedNode(n) => Term::Iri(n.into_string()),
                    om::Term::BlankNode(b) => Term::Blank(b.into_string()),
                    om::Term::Literal(l) => {
                        let (lex, dt, lang) = l.destruct();
                        Term::Lit {
                            lex,
                            dt: dt.map_or_else(|| format!("{}string", crate::adapter::XSD), |d| d.into_string()),
                            lang,
                        }
                    }
                    _ => return None,
                })
            };
            let s = match q.subject {
                om::Subject::NamedNode(n) => Term::Iri(n.into_string()),
                om::Subject::BlankNode(b) => Term::Blank(b.into_string()),
                _ => continue,
            };
            let Some(o) = conv(q.object) else { continue };
            let g = match q.graph_name {
                om::GraphName::NamedNode(n) => Some(n.into_string()),
                _ => None,
            };
            out.push(Quad { s, p: Term::Iri(q.predicate.into_string()), o, g });
        }
    }
    Ok(out)
}

/// Files in `dir` with one of the extensions, sorted.
pub fn files_in(dir: &Path, exts: &[&str]) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        bail!("{} not found: run the fetch script first (see bench/README.md)", dir.display());
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.contains(&e)))
        .collect();
    v.sort();
    if v.is_empty() {
        bail!("no {:?} files in {}", exts, dir.display());
    }
    Ok(v)
}

/// Loads `quads` into a fresh engine; returns adapter, dir and load metrics.
pub fn load_engine(c: &Ctx, e: Engine, tag: &str, quads: &[Quad]) -> Result<(Box<dyn Adapter>, PathBuf, Value)> {
    if e == Engine::Sqlite {
        return Err(crate::adapter::Unsupported("SPARQL workloads on SQLite (no SPARQL front end)").into());
    }
    let dir = c.fresh_dir(&format!("{}-{tag}", e.name()))?;
    let a = c.open(e, &dir)?;
    let t0 = Instant::now();
    let r = {
        let mut s = a.session()?;
        s.load(quads)
    };
    if let Err(x) = r {
        if is_unsupported(&x) {
            return Err(x);
        }
        return Err(x.context(format!("{} load", e.name())));
    }
    a.checkpoint()?;
    let secs = t0.elapsed().as_secs_f64();
    let load = json!({"quads": quads.len(), "load_s": secs, "quads_per_s": quads.len() as f64 / secs.max(1e-9),
        "disk_bytes": a.disk_size(), "bytes_per_quad": a.disk_size() as f64 / quads.len().max(1) as f64});
    Ok((a, dir, load))
}

/// Runs every query instance `runs` times on one loaded engine. Per template:
/// latency histogram per run, row counts (for cross-engine checks), errors.
pub fn run_queries(c: &Ctx, e: Engine, a: &dyn Adapter, queries: &[Query]) -> Result<Value> {
    let mut series = RunSeries::default();
    let mut runs = Vec::new();
    let mut rows: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut errors: BTreeMap<String, String> = BTreeMap::new();
    for run in 0..c.c.runs {
        let mut s = a.session()?;
        let mut h = Hists::default();
        for q in queries {
            if errors.contains_key(&q.template) {
                continue;
            }
            let t = Instant::now();
            let r = match q.lang {
                Lang::Sparql => s.sparql(&q.text),
                Lang::Datalog => s.datalog(&q.text),
            };
            let ns = t.elapsed().as_nanos() as u64;
            match r {
                Ok(n) => {
                    h.rec(&q.template, ns);
                    if run == 0 {
                        rows.entry(q.template.clone()).or_default().push(n as i64);
                    }
                }
                Err(x) => {
                    let msg = if is_unsupported(&x) { "unsupported".to_string() } else { format!("{x:#}") };
                    errors.insert(q.template.clone(), msg);
                }
            }
        }
        series.push_hists(&h);
        runs.push(json!({"run": run, "latency": h.to_json()}));
    }
    Ok(json!({"adapter": e.name(), "durability_matches": durability_matches(e, c.c.durability),
        "result_rows_first_run": rows, "errors": errors, "summary": series.to_json(), "runs": runs}))
}
