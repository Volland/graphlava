//! WatDiv (Aluç et al. 2014) basic testing use case: linear (L1–L5), star
//! (S1–S7), snowflake (F1–F5) and complex (C1–C3) templates, 20 instances each.
//! `bench/scripts/fetch-watdiv.sh` builds the official generator in Docker and
//! writes `bench/data/watdiv/sf<S>/watdiv.nt` plus `queries/<T>.sparql`
//! (one file per template, the generator's instances one after another).

use super::{files_in, load_engine, read_rdf, run_queries, Lang, Query};
use crate::run::Ctx;
use anyhow::{bail, Result};
use serde_json::json;

#[derive(Clone, Debug, clap::Args)]
pub struct WatdivArgs {
    /// Scale factor directory (sf1 = 10^5 triples… sf100 = 10^7; sf1000 = 10^8).
    #[arg(long, default_value_t = 100)]
    pub scale: u32,
    /// Instances per template to run.
    #[arg(long, default_value_t = 20)]
    pub instances: usize,
}

/// Splits a generator output file into query instances (each starts at `SELECT`).
pub fn split_instances(text: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if l.starts_with("SELECT") || v.is_empty() {
            v.push(String::new());
        }
        let cur = v.last_mut().expect("instance");
        cur.push_str(l);
        cur.push('\n');
    }
    v
}

pub fn run(c: &Ctx, a: &WatdivArgs) -> Result<()> {
    let dir = crate::run::bench_dir().join("data").join("watdiv").join(format!("sf{}", a.scale));
    let data = files_in(&dir, &["nt"])?;
    let qfiles = files_in(&dir.join("queries"), &["sparql", "txt"])?;
    let mut qs = Vec::new();
    for f in &qfiles {
        let t = f.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
        for (i, q) in split_instances(&std::fs::read_to_string(f)?).into_iter().take(a.instances).enumerate() {
            let _ = i;
            qs.push(Query { template: t.clone(), lang: Lang::Sparql, text: q });
        }
    }
    if qs.is_empty() {
        bail!("no WatDiv query instances in {}", dir.join("queries").display());
    }
    eprintln!("[watdiv] parsing {} data files, {} query instances", data.len(), qs.len());
    let quads = read_rdf(&data, None)?;
    let mut out = Vec::new();
    for &e in &c.c.adapters {
        match load_engine(c, e, &format!("watdiv{}", a.scale), &quads) {
            Ok((ad, d, load)) => {
                let mut r = run_queries(c, e, ad.as_ref(), &qs)?;
                r["load"] = load;
                r["describe"] = ad.describe();
                out.push(r);
                drop(ad);
                c.cleanup(&d);
            }
            Err(x) => out.push(json!({"adapter": e.name(), "skipped": format!("{x:#}")})),
        }
    }
    c.save("watdiv", json!({"scale": a.scale, "instances": a.instances, "quads": quads.len()}), json!({"adapters": out}))
}

#[cfg(test)]
mod tests {
    #[test]
    fn splits() {
        let t = "SELECT ?a WHERE {\n ?a <p> ?b .\n}\nSELECT ?c WHERE { ?c <q> ?d . }\n";
        assert_eq!(super::split_instances(t).len(), 2);
    }
}
