//! `info`, `stats`, `check`, `sweep`, `build-ordering` and `build-index`.

use std::path::Path;
use std::time::Instant;

use graphlava::id::{DEFAULT_GRAPH, SYSTEM_GRAPH};
use graphlava::tables::{self, feature, mk};
use graphlava::{Database, Id, Perm, QueryResult, Reader, Term};

use crate::cli::{BuildIndexArgs, BuildOrderingArgs, OrderingArg, SweepArgs};
use crate::error::{CliError, CliResult};
use crate::render::{Prefixes, compact};
use crate::run::{Ctx, ms, open_existing};

const FEATURES: &[(u64, &str)] = &[
    (feature::ON_DEMAND_ORDERINGS, "on-demand-orderings"),
    (feature::VALUE_INDEX, "value-index"),
    (feature::PROPS_SPILL, "props-spill"),
    (feature::UNASSERTED, "unasserted"),
    (feature::MULTI, "multi"),
    (feature::MEMBERSHIP, "membership"),
    (feature::NESTING, "nesting"),
    (feature::HYPEREDGES, "hyperedges"),
    (feature::ALIASES, "aliases"),
];

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut u = "B";
    for unit in UNITS {
        if v < 1024.0 {
            break;
        }
        v /= 1024.0;
        u = unit;
    }
    format!("{v:.1} {u}")
}

fn print_pairs(rows: &[(String, String)]) {
    let w = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    for (k, v) in rows {
        println!("{k:<w$}  {v}");
    }
}

pub fn info_cmd(_ctx: Ctx, file: &Path) -> CliResult<u8> {
    let db = open_existing(file)?;
    let size = db.file_size()?;
    let r = db.read()?;
    let kv = r.kv();
    let fv = tables::meta_u64(kv, mk::FORMAT)?;
    let feats = tables::meta_u64(kv, mk::FEATURES)?.unwrap_or(0);
    let names: Vec<&str> = FEATURES
        .iter()
        .filter(|(b, _)| feats & b != 0)
        .map(|(_, n)| *n)
        .collect();
    let mut orderings = vec!["spo pos osp (base)".to_string()];
    for p in Perm::ON_DEMAND {
        let st = graphlava::store::ordering_state(kv, p)?;
        orderings.push(format!(
            "{} {}",
            p.name(),
            match st {
                2 => "ready",
                1 => "building",
                _ => "absent",
            }
        ));
    }
    let opt = |k: &str| -> CliResult<String> {
        Ok(tables::meta_u64(kv, k)?.map_or_else(|| "-".into(), |v| v.to_string()))
    };
    let rows = vec![
        ("file".to_string(), file.display().to_string()),
        (
            "size".into(),
            format!("{} ({size} bytes)", human_bytes(size)),
        ),
        (
            "format version".into(),
            format!(
                "{} (this build reads {})",
                fv.map_or_else(|| "-".into(), |v| v.to_string()),
                tables::FORMAT_VERSION
            ),
        ),
        (
            "features".into(),
            if names.is_empty() {
                format!("none ({feats:#x})")
            } else {
                format!("{} ({feats:#x})", names.join(", "))
            },
        ),
        ("commits".into(), r.commit_counter()?.to_string()),
        ("orderings".into(), orderings.join(", ")),
        ("graph depth limit".into(), opt(mk::DEPTH_LIMIT)?),
        (
            "owned literal threshold".into(),
            format!("{} bytes", opt(mk::OWNED_THRESHOLD)?),
        ),
        ("namespace cap".into(), opt(mk::NS_CAP)?),
        (
            "core version".into(),
            format!("{} (cli {})", graphlava::VERSION, env!("CARGO_PKG_VERSION")),
        ),
    ];
    print_pairs(&rows);
    drop(r);
    db.close()?;
    Ok(0)
}

fn count(db: &Database, q: &str) -> CliResult<u64> {
    match db.sparql(q)? {
        QueryResult::Solutions { rows, .. } => Ok(rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .and_then(|t| t.as_literal().and_then(|l| l.lexical.parse().ok()))
            .unwrap_or(0)),
        _ => Ok(0),
    }
}

fn name(r: &impl Reader, id: Id, px: &Prefixes) -> CliResult<String> {
    if id == DEFAULT_GRAPH {
        return Ok("(default graph)".into());
    }
    Ok(compact(&r.decode(id)?, px))
}

pub fn stats_cmd(_ctx: Ctx, file: &Path) -> CliResult<u8> {
    let db = open_existing(file)?;
    let px = Prefixes::with_defaults();
    const NOT_IN: &str = "FILTER(?p != <https://graphlava.org/ns#in>)";
    let total = count(
        &db,
        &format!("SELECT (COUNT(*) AS ?n) WHERE {{ ?s ?p ?o {NOT_IN} }}"),
    )?;
    println!("statements  {total}");
    println!();
    // Statements per predicate.
    let res = db.sparql(&format!(
        "SELECT ?p (COUNT(*) AS ?n) WHERE {{ ?s ?p ?o {NOT_IN} }} GROUP BY ?p ORDER BY DESC(?n) ?p"
    ))?;
    let mut rows: Vec<(String, String)> = Vec::new();
    for r in res.rows() {
        let p = r[0].as_ref().map(|t| compact(t, &px)).unwrap_or_default();
        let n = r[1]
            .as_ref()
            .and_then(|t| t.as_literal().map(|l| l.lexical.clone()))
            .unwrap_or_default();
        rows.push((p, n));
    }
    let wp = rows
        .iter()
        .map(|(p, _)| p.chars().count())
        .max()
        .unwrap_or(0)
        .max(9);
    println!("{:<wp$}  statements", "predicate");
    for (p, n) in &rows {
        println!("{p:<wp$}  {n:>10}");
    }
    println!();
    // Graphs and nesting.
    let r = db.read()?;
    let graphs: Vec<Id> = graphlava::graph::all_graphs(r.kv())?
        .into_iter()
        .filter(|g| *g != SYSTEM_GRAPH)
        .collect();
    let mut grows = Vec::new();
    for g in &graphs {
        let n = name(&r, *g, &px)?;
        let (direct, scoped) = if *g == DEFAULT_GRAPH {
            let q = format!(
                "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH! <{}> {{ ?s ?p ?o {NOT_IN} }} }}",
                graphlava::term::DEFAULT_GRAPH_IRI
            );
            let c = count(&db, &q)?;
            (c, c)
        } else {
            let iri = match r.decode(*g)? {
                Term::Iri(i) => i,
                other => other.to_string(),
            };
            (
                count(
                    &db,
                    &format!(
                        "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH! <{iri}> {{ ?s ?p ?o {NOT_IN} }} }}"
                    ),
                )?,
                count(
                    &db,
                    &format!(
                        "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{iri}> {{ ?s ?p ?o {NOT_IN} }} }}"
                    ),
                )?,
            )
        };
        let parents: Vec<String> = r
            .parents(*g)?
            .into_iter()
            .filter(|p| *p != SYSTEM_GRAPH)
            .map(|p| name(&r, p, &px))
            .collect::<CliResult<_>>()?;
        grows.push((n, direct, scoped, parents.join(", ")));
    }
    let wg = grows
        .iter()
        .map(|g| g.0.chars().count())
        .max()
        .unwrap_or(0)
        .max(5);
    println!(
        "{:<wg$}  {:>10}  {:>10}  nested in",
        "graph", "direct", "in scope"
    );
    for (n, d, s, p) in &grows {
        println!(
            "{n:<wg$}  {d:>10}  {s:>10}  {}",
            if p.is_empty() { "-" } else { p }
        );
    }
    println!();
    // Hyperedge types.
    let types = graphlava::hedge::hedge_types(r.kv())?;
    if types.is_empty() {
        println!("hyperedge types  none");
    } else {
        let mut hrows = Vec::new();
        for (ty, decl) in types {
            let roles: Vec<String> = decl
                .roles
                .iter()
                .map(|rd| name(&r, rd.role, &px))
                .collect::<CliResult<_>>()?;
            let n = graphlava::hedge::hedges_of_type(r.kv(), ty)?.len();
            hrows.push((name(&r, ty, &px)?, n, roles.join(" ")));
        }
        let wt = hrows
            .iter()
            .map(|h| h.0.chars().count())
            .max()
            .unwrap_or(0)
            .max(14);
        println!("{:<wt$}  {:>10}  roles", "hyperedge type", "hyperedges");
        for (t, n, roles) in &hrows {
            println!("{t:<wt$}  {n:>10}  {roles}");
        }
    }
    drop(r);
    db.close()?;
    Ok(0)
}

pub fn check_cmd(ctx: Ctx, file: &Path) -> CliResult<u8> {
    let db = open_existing(file)?;
    let t0 = Instant::now();
    let found = db.read()?.check_invariants()?;
    db.close()?;
    match found {
        None => {
            println!("ok: no invariant violations");
            ctx.note(format!("checked in {}", ms(t0.elapsed())));
            Ok(0)
        }
        Some(v) => Err(CliError::Failed(format!("invariant violation: {v}"))),
    }
}

pub fn sweep_cmd(ctx: Ctx, a: SweepArgs) -> CliResult<u8> {
    let db = open_existing(&a.file)?;
    let t0 = Instant::now();
    let rep = db.admin().sweep(a.window)?;
    println!(
        "marked {}, examined {}, deleted {}, kept (young) {}, kept (re-referenced) {}",
        rep.marked, rep.examined, rep.deleted, rep.kept_young, rep.kept_rereferenced
    );
    ctx.note(format!("swept in {}", ms(t0.elapsed())));
    db.close()?;
    Ok(0)
}

pub fn build_ordering_cmd(ctx: Ctx, a: BuildOrderingArgs) -> CliResult<u8> {
    let perm = match a.ordering {
        OrderingArg::Pso => Perm::Pso,
        OrderingArg::Sop => Perm::Sop,
        OrderingArg::Ops => Perm::Ops,
    };
    let db = open_existing(&a.file)?;
    let t0 = Instant::now();
    db.admin().build_ordering(perm)?;
    println!("ordering {} ready", perm.name());
    ctx.note(format!("built in {}", ms(t0.elapsed())));
    db.close()?;
    Ok(0)
}

/// A predicate argument: `<iri>`, a bare IRI, or a prefixed name with a
/// default prefix (`rdfs:label`).
fn predicate_iri(s: &str) -> CliResult<String> {
    let s = s.trim();
    if let Some(i) = s.strip_prefix('<').and_then(|x| x.strip_suffix('>')) {
        return Ok(i.to_string());
    }
    if let Some((p, local)) = s.split_once(':') {
        if !local.starts_with("//") {
            if let Some((_, ns)) = crate::render::DEFAULT_PREFIXES
                .iter()
                .find(|(n, _)| *n == p)
            {
                return Ok(format!("{ns}{local}"));
            }
        }
        return Ok(s.to_string());
    }
    Err(CliError::Usage(format!(
        "'{s}' is not an IRI: write <http://...>, http://... or rdfs:label"
    )))
}

pub fn build_index_cmd(ctx: Ctx, a: BuildIndexArgs) -> CliResult<u8> {
    let iri = predicate_iri(&a.predicate)?;
    let db = open_existing(&a.file)?;
    let t0 = Instant::now();
    db.admin().build_value_index(&Term::iri(iri.clone()))?;
    println!("value index on <{iri}> ready");
    ctx.note(format!("built in {}", ms(t0.elapsed())));
    db.close()?;
    Ok(0)
}
