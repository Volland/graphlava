//! LUBM (Guo, Pan, Heflin 2005): the official UBA generator's data
//! (`bench/scripts/fetch-lubm.sh` writes `bench/data/lubm/u<N>/*.owl` and
//! `bench/data/lubm/univ-bench.owl`), the 14 benchmark queries, run plain
//! (no reasoning, every engine) and with RDFS rules through graphlava Datalog.
//!
//! The rule set is RDFS only (subClassOf, subPropertyOf, domain, range over the
//! univ-bench ontology). LUBM's reference answers need OWL (inverseOf,
//! TransitiveProperty, intersectionOf/someValuesFrom), so Q5, Q6–Q10 approach
//! them under RDFS while Q11, Q12 and Q13 stay incomplete; the result records
//! row counts per query so the gap is visible.

use super::{files_in, load_engine, read_rdf, run_queries, Lang, Query};
use crate::run::Ctx;
use anyhow::Result;
use serde_json::json;

#[derive(Clone, Debug, clap::Args)]
pub struct LubmArgs {
    /// Universities (data directory bench/data/lubm/u<N>).
    #[arg(long, default_value_t = 10)]
    pub universities: u32,
    /// Also run the 14 queries as Datalog with RDFS rules (graphlava only).
    #[arg(long, default_value_t = true)]
    pub rdfs: bool,
}

const PFX: &str = "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\nPREFIX ub: <http://swat.cse.lehigh.edu/onto/univ-bench.owl#>\n";

/// The 14 LUBM queries (SPARQL form of the official query file).
pub const QUERIES: [&str; 14] = [
    "SELECT ?X WHERE { ?X rdf:type ub:GraduateStudent . ?X ub:takesCourse <http://www.Department0.University0.edu/GraduateCourse0> }",
    "SELECT ?X ?Y ?Z WHERE { ?X rdf:type ub:GraduateStudent . ?Y rdf:type ub:University . ?Z rdf:type ub:Department . ?X ub:memberOf ?Z . ?Z ub:subOrganizationOf ?Y . ?X ub:undergraduateDegreeFrom ?Y }",
    "SELECT ?X WHERE { ?X rdf:type ub:Publication . ?X ub:publicationAuthor <http://www.Department0.University0.edu/AssistantProfessor0> }",
    "SELECT ?X ?Y1 ?Y2 ?Y3 WHERE { ?X rdf:type ub:Professor . ?X ub:worksFor <http://www.Department0.University0.edu> . ?X ub:name ?Y1 . ?X ub:emailAddress ?Y2 . ?X ub:telephone ?Y3 }",
    "SELECT ?X WHERE { ?X rdf:type ub:Person . ?X ub:memberOf <http://www.Department0.University0.edu> }",
    "SELECT ?X WHERE { ?X rdf:type ub:Student }",
    "SELECT ?X ?Y WHERE { ?X rdf:type ub:Student . ?Y rdf:type ub:Course . ?X ub:takesCourse ?Y . <http://www.Department0.University0.edu/AssociateProfessor0> ub:teacherOf ?Y }",
    "SELECT ?X ?Y ?Z WHERE { ?X rdf:type ub:Student . ?Y rdf:type ub:Department . ?X ub:memberOf ?Y . ?Y ub:subOrganizationOf <http://www.University0.edu> . ?X ub:emailAddress ?Z }",
    "SELECT ?X ?Y ?Z WHERE { ?X rdf:type ub:Student . ?Y rdf:type ub:Faculty . ?Z rdf:type ub:Course . ?X ub:advisor ?Y . ?Y ub:teacherOf ?Z . ?X ub:takesCourse ?Z }",
    "SELECT ?X WHERE { ?X rdf:type ub:Student . ?X ub:takesCourse <http://www.Department0.University0.edu/GraduateCourse0> }",
    "SELECT ?X WHERE { ?X rdf:type ub:ResearchGroup . ?X ub:subOrganizationOf <http://www.University0.edu> }",
    "SELECT ?X ?Y WHERE { ?X rdf:type ub:Chair . ?Y rdf:type ub:Department . ?X ub:worksFor ?Y . ?Y ub:subOrganizationOf <http://www.University0.edu> }",
    "SELECT ?X WHERE { ?X rdf:type ub:Person . <http://www.University0.edu> ub:hasAlumnus ?X }",
    "SELECT ?X WHERE { ?X rdf:type ub:UndergraduateStudent }",
];

/// RDFS closure as Datalog: `t/3` is the entailed triple relation, `ty/2` types.
pub const RDFS_RULES: &str = r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ub: <http://swat.cse.lehigh.edu/onto/univ-bench.owl#> .
sc(?a, ?b) :- triple(?a, rdfs:subClassOf, ?b) .
sc(?a, ?c) :- sc(?a, ?b), triple(?b, rdfs:subClassOf, ?c) .
sp(?a, ?b) :- triple(?a, rdfs:subPropertyOf, ?b) .
sp(?a, ?c) :- sp(?a, ?b), triple(?b, rdfs:subPropertyOf, ?c) .
t(?x, ?p, ?y) :- triple(?x, ?p, ?y) .
t(?x, ?q, ?y) :- triple(?x, ?p, ?y), sp(?p, ?q) .
ty(?x, ?c) :- triple(?x, rdf:type, ?c) .
ty(?x, ?c) :- t(?x, ?p, ?y), triple(?p, rdfs:domain, ?c) .
ty(?y, ?c) :- t(?x, ?p, ?y), triple(?p, rdfs:range, ?c) .
ty(?x, ?d) :- ty(?x, ?c), sc(?c, ?d) .
"#;

/// The 14 queries over the entailed relations.
pub const DATALOG_QUERIES: [&str; 14] = [
    "q(?x) :- ty(?x, ub:GraduateStudent), t(?x, ub:takesCourse, <http://www.Department0.University0.edu/GraduateCourse0>) .\n?- q(?x) .",
    "q(?x, ?y, ?z) :- ty(?x, ub:GraduateStudent), ty(?y, ub:University), ty(?z, ub:Department), t(?x, ub:memberOf, ?z), t(?z, ub:subOrganizationOf, ?y), t(?x, ub:undergraduateDegreeFrom, ?y) .\n?- q(?x, ?y, ?z) .",
    "q(?x) :- ty(?x, ub:Publication), t(?x, ub:publicationAuthor, <http://www.Department0.University0.edu/AssistantProfessor0>) .\n?- q(?x) .",
    "q(?x, ?a, ?b, ?c) :- ty(?x, ub:Professor), t(?x, ub:worksFor, <http://www.Department0.University0.edu>), t(?x, ub:name, ?a), t(?x, ub:emailAddress, ?b), t(?x, ub:telephone, ?c) .\n?- q(?x, ?a, ?b, ?c) .",
    "q(?x) :- ty(?x, ub:Person), t(?x, ub:memberOf, <http://www.Department0.University0.edu>) .\n?- q(?x) .",
    "q(?x) :- ty(?x, ub:Student) .\n?- q(?x) .",
    "q(?x, ?y) :- ty(?x, ub:Student), ty(?y, ub:Course), t(?x, ub:takesCourse, ?y), t(<http://www.Department0.University0.edu/AssociateProfessor0>, ub:teacherOf, ?y) .\n?- q(?x, ?y) .",
    "q(?x, ?y, ?z) :- ty(?x, ub:Student), ty(?y, ub:Department), t(?x, ub:memberOf, ?y), t(?y, ub:subOrganizationOf, <http://www.University0.edu>), t(?x, ub:emailAddress, ?z) .\n?- q(?x, ?y, ?z) .",
    "q(?x, ?y, ?z) :- ty(?x, ub:Student), ty(?y, ub:Faculty), ty(?z, ub:Course), t(?x, ub:advisor, ?y), t(?y, ub:teacherOf, ?z), t(?x, ub:takesCourse, ?z) .\n?- q(?x, ?y, ?z) .",
    "q(?x) :- ty(?x, ub:Student), t(?x, ub:takesCourse, <http://www.Department0.University0.edu/GraduateCourse0>) .\n?- q(?x) .",
    "q(?x) :- ty(?x, ub:ResearchGroup), t(?x, ub:subOrganizationOf, <http://www.University0.edu>) .\n?- q(?x) .",
    "q(?x, ?y) :- ty(?x, ub:Chair), ty(?y, ub:Department), t(?x, ub:worksFor, ?y), t(?y, ub:subOrganizationOf, <http://www.University0.edu>) .\n?- q(?x, ?y) .",
    "q(?x) :- ty(?x, ub:Person), t(<http://www.University0.edu>, ub:hasAlumnus, ?x) .\n?- q(?x) .",
    "q(?x) :- ty(?x, ub:UndergraduateStudent) .\n?- q(?x) .",
];

pub fn queries(rdfs: bool) -> Vec<Query> {
    let mut v: Vec<Query> = QUERIES
        .iter()
        .enumerate()
        .map(|(i, q)| Query { template: format!("Q{}", i + 1), lang: Lang::Sparql, text: format!("{PFX}{q}") })
        .collect();
    if rdfs {
        v.extend(DATALOG_QUERIES.iter().enumerate().map(|(i, q)| Query {
            template: format!("Q{}_rdfs", i + 1),
            lang: Lang::Datalog,
            text: format!("{RDFS_RULES}{q}"),
        }));
    }
    v
}

pub fn run(c: &Ctx, a: &LubmArgs) -> Result<()> {
    let root = crate::run::bench_dir().join("data").join("lubm");
    let dir = root.join(format!("u{}", a.universities));
    let mut files = files_in(&dir, &["owl", "nt"])?;
    let onto = root.join("univ-bench.owl");
    if onto.exists() {
        files.push(onto);
    } else if a.rdfs {
        eprintln!("warning: {} missing; RDFS queries will find no schema", onto.display());
    }
    eprintln!("[lubm] parsing {} files", files.len());
    let quads = read_rdf(&files, Some("http://swat.cse.lehigh.edu/onto/univ-bench.owl"))?;
    let mut out = Vec::new();
    for &e in &c.c.adapters {
        match load_engine(c, e, &format!("lubm{}", a.universities), &quads) {
            Ok((ad, d, load)) => {
                let qs: Vec<Query> = queries(a.rdfs && e == crate::adapter::Engine::Graphlava);
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
    c.save("lubm", json!({"universities": a.universities, "rdfs": a.rdfs, "quads": quads.len(),
        "rules": "RDFS (subClassOf, subPropertyOf, domain, range); see workload/lubm.rs"}), json!({"adapters": out}))
}
