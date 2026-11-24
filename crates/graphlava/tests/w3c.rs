//! W3C SPARQL conformance runner ([[11-sparql#Conformance]]).
//!
//! Runs the SPARQL 1.1 query and update suites and the SPARQL 1.2 suite of
//! `w3c/rdf-tests` at the commit pinned in `tests/w3c/PINNED.md`, in standard
//! mode ([`QueryOptions::standard`]: `GRAPH` is direct, the default graph is
//! the file's default graph). Fetch the suites with `tests/w3c/fetch.sh`; when
//! they are absent the test passes with a message unless
//! `GRAPHLAVA_REQUIRE_W3C=1`.
//!
//! Every failing test must be listed in `tests/w3c/skip.txt` with a reason;
//! a listed test that passes fails the run, so the list stays accurate.
//!
//! Environment: `GRAPHLAVA_W3C_DIR` (suite root, default
//! `<workspace>/target/w3c-tests`), `GRAPHLAVA_W3C_FILTER` (substring of the
//! test IRI to run), `GRAPHLAVA_W3C_VERBOSE=1` (print every failure in full),
//! `GRAPHLAVA_W3C_TIMEOUT` (seconds per test, default 60).

// @tg: verifies:: [[openspec:sparql-query#Conformance#Conformance run]] {level: conformance}
// @tg: verifies:: [[openspec:sparql-query#Graph scope semantics#Standard mode]] {level: conformance}

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use graphlava::{Database, Durability, OpenOptions, QueryOptions, QueryResult};
use oxrdf::{GraphName, NamedOrBlankNode, Quad, Term as OxTerm, Triple};
use sparesults::{QueryResultsFormat, QueryResultsParser, SliceQueryResultsParserOutput};
use spargebra::algebra::GraphPattern;
use spargebra::{Query, SparqlParser};

/// Public base IRI of the suite; files are resolved under the local root.
const BASE: &str = "https://w3c.github.io/rdf-tests/sparql/";
const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const QT: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#";
const UT: &str = "http://www.w3.org/2009/sparql/tests/test-update#";
const RS: &str = "http://www.w3.org/2001/sw/DataAccess/tests/result-set#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The suites: name and manifest path under the suite root.
const SUITES: &[(&str, &str)] = &[
    (
        "sparql11-query",
        "sparql/sparql11/manifest-sparql11-query.ttl",
    ),
    (
        "sparql11-update",
        "sparql/sparql11/manifest-sparql11-update.ttl",
    ),
    ("sparql12", "sparql/sparql12/manifest.ttl"),
];

// ---------------------------------------------------------------- terms ----

/// A term as compared: literals carry a normalised lexical form.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum T {
    Default,
    Iri(String),
    Blank(String),
    /// Lexical form, datatype, language tag (lower case, with `--dir`).
    Lit(String, String, String),
    Triple(Box<[T; 3]>),
}

impl fmt::Display for T {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            T::Default => write!(f, "DEFAULT"),
            T::Iri(i) => write!(f, "<{i}>"),
            T::Blank(b) => write!(f, "_:{b}"),
            T::Lit(l, _, lang) if !lang.is_empty() => write!(f, "{l:?}@{lang}"),
            T::Lit(l, d, _) => match d.strip_prefix(XSD) {
                Some("string") => write!(f, "{l:?}"),
                Some(x) => write!(f, "{l:?}^^xsd:{x}"),
                None => write!(f, "{l:?}^^<{d}>"),
            },
            T::Triple(t) => write!(f, "<<( {} {} {} )>>", t[0], t[1], t[2]),
        }
    }
}

impl T {
    fn has_blank(&self) -> bool {
        match self {
            T::Blank(_) => true,
            T::Triple(t) => t.iter().any(T::has_blank),
            _ => false,
        }
    }
    /// The term with every blank node replaced by one placeholder.
    fn masked(&self) -> T {
        match self {
            T::Blank(_) => T::Blank(String::new()),
            T::Triple(t) => T::Triple(Box::new([t[0].masked(), t[1].masked(), t[2].masked()])),
            x => x.clone(),
        }
    }
}

const INTEGER_TYPES: &[&str] = &[
    "integer",
    "int",
    "long",
    "short",
    "byte",
    "nonNegativeInteger",
    "positiveInteger",
    "negativeInteger",
    "nonPositiveInteger",
    "unsignedLong",
    "unsignedInt",
    "unsignedShort",
    "unsignedByte",
];

/// Numeric and boolean literals compare by value: storage canonicalises them
/// (rule `dm-literal-canonical`), so `"01"^^xsd:integer` reads back as `"1"`.
fn norm_lexical(lex: &str, dt: &str) -> String {
    let Some(local) = dt.strip_prefix(XSD) else {
        return lex.to_string();
    };
    let s = lex.trim();
    if INTEGER_TYPES.contains(&local) {
        let (neg, digits) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return lex.to_string();
        }
        let d = digits.trim_start_matches('0');
        return match (d.is_empty(), neg) {
            (true, _) => "0".into(),
            (false, true) => format!("-{d}"),
            (false, false) => d.to_string(),
        };
    }
    match local {
        "decimal" => {
            let (neg, body) = match s.as_bytes().first() {
                Some(b'-') => (true, &s[1..]),
                Some(b'+') => (false, &s[1..]),
                _ => (false, s),
            };
            let (i, f) = body.split_once('.').unwrap_or((body, ""));
            if i.is_empty() && f.is_empty()
                || !i.bytes().all(|b| b.is_ascii_digit())
                || !f.bytes().all(|b| b.is_ascii_digit())
            {
                return lex.to_string();
            }
            let i = i.trim_start_matches('0');
            let f = f.trim_end_matches('0');
            let i = if i.is_empty() { "0" } else { i };
            let f = if f.is_empty() { "0" } else { f };
            if i == "0" && f == "0" {
                "0.0".into()
            } else {
                format!("{}{i}.{f}", if neg { "-" } else { "" })
            }
        }
        "double" | "float" => {
            let v = match s {
                "INF" | "+INF" => f64::INFINITY,
                "-INF" => f64::NEG_INFINITY,
                "NaN" => return "NaN".into(),
                _ => match s.parse::<f64>() {
                    Ok(v) => v,
                    Err(_) => return lex.to_string(),
                },
            };
            if local == "float" {
                format!("{:e}", v as f32)
            } else {
                format!("{v:e}")
            }
        }
        "boolean" => match s {
            "1" | "true" => "true".into(),
            "0" | "false" => "false".into(),
            _ => lex.to_string(),
        },
        _ => lex.to_string(),
    }
}

fn lit(lex: &str, dt: &str, lang: Option<&str>) -> T {
    match lang {
        Some(l) => T::Lit(
            lex.to_string(),
            format!("{RDF}langString"),
            l.to_ascii_lowercase(),
        ),
        None => T::Lit(norm_lexical(lex, dt), dt.to_string(), String::new()),
    }
}

fn from_ox(t: &OxTerm) -> T {
    match t {
        OxTerm::NamedNode(n) => T::Iri(n.as_str().into()),
        OxTerm::BlankNode(b) => T::Blank(b.as_str().into()),
        OxTerm::Literal(l) => {
            let lang = l.language().map(|g| match l.direction() {
                Some(d) => format!("{g}--{d}"),
                None => g.to_string(),
            });
            lit(l.value(), l.datatype().as_str(), lang.as_deref())
        }
        OxTerm::Triple(t) => T::Triple(Box::new(triple_ts(t))),
    }
}

fn from_subject(s: &NamedOrBlankNode) -> T {
    match s {
        NamedOrBlankNode::NamedNode(n) => T::Iri(n.as_str().into()),
        NamedOrBlankNode::BlankNode(b) => T::Blank(b.as_str().into()),
    }
}

fn triple_ts(t: &Triple) -> [T; 3] {
    [
        from_subject(&t.subject),
        T::Iri(t.predicate.as_str().into()),
        from_ox(&t.object),
    ]
}

fn from_gl(t: &graphlava::Term) -> T {
    match t {
        graphlava::Term::Iri(i) => T::Iri(i.clone()),
        graphlava::Term::Blank(b) => T::Blank(b.clone()),
        graphlava::Term::Literal(l) => lit(&l.lexical, &l.datatype, l.lang.as_deref()),
        graphlava::Term::Triple(t) => T::Triple(Box::new([
            from_gl(&t.subject),
            from_gl(&t.predicate),
            from_gl(&t.object),
        ])),
        other => panic!("unsupported term in a result: {other}"),
    }
}

// ----------------------------------------------------------- isomorphism ---

type Row = Vec<Option<T>>;

#[derive(Clone, Default)]
struct BMap {
    fwd: HashMap<String, String>,
    bwd: HashMap<String, String>,
}

/// Unifies an expected term with an actual one under a blank-node bijection.
fn unify(e: &T, a: &T, m: &mut BMap) -> bool {
    match (e, a) {
        (T::Blank(x), T::Blank(y)) => match (m.fwd.get(x), m.bwd.get(y)) {
            (Some(y2), Some(x2)) => y2 == y && x2 == x,
            (None, None) => {
                m.fwd.insert(x.clone(), y.clone());
                m.bwd.insert(y.clone(), x.clone());
                true
            }
            _ => false,
        },
        (T::Triple(x), T::Triple(y)) => (0..3).all(|i| unify(&x[i], &y[i], m)),
        _ => e == a,
    }
}

fn unify_row(e: &Row, a: &Row, m: &mut BMap) -> bool {
    e.len() == a.len()
        && e.iter().zip(a).all(|(x, y)| match (x, y) {
            (None, None) => true,
            (Some(x), Some(y)) => unify(x, y, m),
            _ => false,
        })
}

fn mask(r: &Row) -> Row {
    r.iter().map(|x| x.as_ref().map(T::masked)).collect()
}

/// Multiset (or sequence) equality of rows up to blank-node renaming.
fn rows_isomorphic(exp: &[Row], act: &[Row], ordered: bool) -> bool {
    if exp.len() != act.len() {
        return false;
    }
    if ordered {
        let mut m = BMap::default();
        return exp.iter().zip(act).all(|(e, a)| unify_row(e, a, &mut m));
    }
    let mut me: Vec<Row> = exp.iter().map(mask).collect();
    let mut ma: Vec<Row> = act.iter().map(mask).collect();
    me.sort();
    ma.sort();
    if me != ma {
        return false;
    }
    let blanks = |rs: &[Row]| rs.iter().flatten().flatten().any(T::has_blank);
    if !blanks(exp) && !blanks(act) {
        return true;
    }
    // candidates by masked key; expected rows with fewest candidates first
    let mut by_key: HashMap<Row, Vec<usize>> = HashMap::new();
    for (j, a) in act.iter().enumerate() {
        by_key.entry(mask(a)).or_default().push(j);
    }
    let mut order: Vec<usize> = (0..exp.len()).collect();
    order.sort_by_key(|&i| by_key[&mask(&exp[i])].len());
    let mut search = Search {
        order: &order,
        exp,
        act,
        by_key: &by_key,
        used: vec![false; act.len()],
        budget: 2_000_000,
    };
    search.go(0, &BMap::default())
}

/// Backtracking search for a row bijection under one blank-node mapping.
struct Search<'a> {
    order: &'a [usize],
    exp: &'a [Row],
    act: &'a [Row],
    by_key: &'a HashMap<Row, Vec<usize>>,
    used: Vec<bool>,
    budget: u64,
}

impl Search<'_> {
    fn go(&mut self, k: usize, m: &BMap) -> bool {
        if k == self.order.len() {
            return true;
        }
        let e = &self.exp[self.order[k]];
        for &j in &self.by_key[&mask(e)] {
            if self.used[j] {
                continue;
            }
            if self.budget == 0 {
                return false;
            }
            self.budget -= 1;
            let mut m2 = m.clone();
            if unify_row(e, &self.act[j], &mut m2) {
                self.used[j] = true;
                if self.go(k + 1, &m2) {
                    return true;
                }
                self.used[j] = false;
            }
        }
        false
    }
}

// --------------------------------------------------------- RDF reading -----

fn iri_to_path(root: &Path, iri: &str) -> Option<PathBuf> {
    iri.strip_prefix(BASE)
        .map(|rest| root.join("sparql").join(rest))
}

fn path_to_iri(root: &Path, p: &Path) -> String {
    let rel = p
        .strip_prefix(root.join("sparql"))
        .expect("file under the suite root");
    format!("{BASE}{}", rel.to_string_lossy().replace('\\', "/"))
}

fn read(root: &Path, iri: &str) -> Result<Vec<u8>, String> {
    let p = iri_to_path(root, iri).ok_or_else(|| format!("not a suite file: {iri}"))?;
    std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Parses an RDF file into quads (Turtle, N-Triples, N-Quads, TriG, RDF/XML).
fn parse_rdf(root: &Path, iri: &str) -> Result<Vec<Quad>, String> {
    let bytes = read(root, iri)?;
    let err = |e: &dyn fmt::Display| format!("{iri}: {e}");
    let to_quads = |ts: Vec<Triple>| {
        ts.into_iter()
            .map(|t| t.in_graph(GraphName::DefaultGraph))
            .collect()
    };
    if iri.ends_with(".rdf") {
        let p = oxrdfxml::RdfXmlParser::new()
            .with_base_iri(iri)
            .map_err(|e| err(&e))?;
        let ts = p
            .for_slice(&bytes)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| err(&e))?;
        Ok(to_quads(ts))
    } else if iri.ends_with(".nq") {
        oxttl::NQuadsParser::new()
            .for_slice(&bytes)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| err(&e))
    } else if iri.ends_with(".trig") {
        let p = oxttl::TriGParser::new()
            .with_base_iri(iri)
            .map_err(|e| err(&e))?;
        p.for_slice(&bytes)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| err(&e))
    } else if iri.ends_with(".nt") {
        let ts = oxttl::NTriplesParser::new()
            .for_slice(&bytes)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| err(&e))?;
        Ok(to_quads(ts))
    } else {
        let p = oxttl::TurtleParser::new()
            .with_base_iri(iri)
            .map_err(|e| err(&e))?;
        let ts = p
            .for_slice(&bytes)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| err(&e))?;
        Ok(to_quads(ts))
    }
}

/// A small in-memory graph for manifests and RDF result sets.
struct Graph(Vec<Triple>);

impl Graph {
    fn load(root: &Path, iri: &str) -> Result<Graph, String> {
        Ok(Graph(
            parse_rdf(root, iri)?
                .into_iter()
                .map(Triple::from)
                .collect(),
        ))
    }
    fn objects<'a>(&'a self, s: &'a OxTerm, p: &'a str) -> impl Iterator<Item = &'a OxTerm> + 'a {
        self.0
            .iter()
            .filter(move |t| subject_term(&t.subject) == *s && t.predicate.as_str() == p)
            .map(|t| &t.object)
    }
    fn object(&self, s: &OxTerm, p: &str) -> Option<OxTerm> {
        self.objects(s, p).next().cloned()
    }
    fn subjects_with(&self, p: &str, o: &str) -> Vec<OxTerm> {
        self.0
            .iter()
            .filter(|t| {
                t.predicate.as_str() == p
                    && matches!(&t.object, OxTerm::NamedNode(n) if n.as_str() == o)
            })
            .map(|t| subject_term(&t.subject))
            .collect()
    }
    fn list(&self, head: &OxTerm) -> Vec<OxTerm> {
        let mut out = Vec::new();
        let mut cur = head.clone();
        let nil = format!("{RDF}nil");
        while !matches!(&cur, OxTerm::NamedNode(n) if n.as_str() == nil) {
            match self.object(&cur, &format!("{RDF}first")) {
                Some(f) => out.push(f),
                None => break,
            }
            match self.object(&cur, &format!("{RDF}rest")) {
                Some(r) => cur = r,
                None => break,
            }
        }
        out
    }
}

fn subject_term(s: &NamedOrBlankNode) -> OxTerm {
    match s {
        NamedOrBlankNode::NamedNode(n) => OxTerm::NamedNode(n.clone()),
        NamedOrBlankNode::BlankNode(b) => OxTerm::BlankNode(b.clone()),
    }
}

fn iri_of(t: &OxTerm) -> Option<String> {
    match t {
        OxTerm::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    }
}

// ------------------------------------------------------------ manifests ----

#[derive(Debug, Clone)]
enum Kind {
    QueryEval {
        query: String,
        data: Vec<String>,
        /// (graph name, file)
        graph_data: Vec<(String, String)>,
        result: Option<String>,
    },
    UpdateEval {
        request: String,
        data: Vec<String>,
        graph_data: Vec<(String, String)>,
        result_data: Vec<String>,
        result_graphs: Vec<(String, String)>,
    },
    Syntax {
        file: String,
        positive: bool,
        update: bool,
    },
}

#[derive(Debug, Clone)]
struct Test {
    iri: String,
    kind: Kind,
}

fn collect_tests(
    root: &Path,
    manifest: &str,
    out: &mut Vec<Test>,
    unsupported: &mut Vec<(String, String)>,
) {
    let g = Graph::load(root, manifest).unwrap_or_else(|e| panic!("manifest {manifest}: {e}"));
    let me = OxTerm::NamedNode(oxrdf::NamedNode::new_unchecked(manifest));
    for inc in g
        .objects(&me, &format!("{MF}include"))
        .flat_map(|h| g.list(h))
        .collect::<Vec<_>>()
    {
        if let Some(i) = iri_of(&inc) {
            collect_tests(root, &i, out, unsupported);
        }
    }
    let entries: Vec<OxTerm> = g
        .objects(&me, &format!("{MF}entries"))
        .flat_map(|h| g.list(h))
        .collect();
    for e in entries {
        let iri = match &e {
            OxTerm::NamedNode(n) => n.as_str().to_string(),
            other => other.to_string(),
        };
        let ty = g
            .object(&e, &format!("{RDF}type"))
            .and_then(|t| iri_of(&t))
            .unwrap_or_default();
        let ty = ty.strip_prefix(MF).unwrap_or(&ty).to_string();
        let action = g.object(&e, &format!("{MF}action"));
        let result = g.object(&e, &format!("{MF}result"));
        let kind = match ty.as_str() {
            "QueryEvaluationTest" => {
                let a = action.expect("action");
                let graph_data = g
                    .objects(&a, &format!("{QT}graphData"))
                    .filter_map(|x| match x {
                        OxTerm::NamedNode(n) => {
                            Some((n.as_str().to_string(), n.as_str().to_string()))
                        }
                        other => {
                            let f = g
                                .object(other, &format!("{UT}graph"))
                                .and_then(|t| iri_of(&t))?;
                            let name = match g.object(other, &format!("{RDFS}label")) {
                                Some(OxTerm::Literal(l)) => l.value().to_string(),
                                _ => f.clone(),
                            };
                            Some((name, f))
                        }
                    })
                    .collect();
                Kind::QueryEval {
                    query: g
                        .object(&a, &format!("{QT}query"))
                        .and_then(|t| iri_of(&t))
                        .expect("query"),
                    data: g
                        .objects(&a, &format!("{QT}data"))
                        .filter_map(iri_of)
                        .collect(),
                    graph_data,
                    result: result.as_ref().and_then(iri_of),
                }
            }
            "UpdateEvaluationTest" => {
                let a = action.expect("action");
                let graphs = |n: &OxTerm| -> Vec<(String, String)> {
                    g.objects(n, &format!("{UT}graphData"))
                        .filter_map(|x| {
                            let f = g
                                .object(x, &format!("{UT}graph"))
                                .and_then(|t| iri_of(&t))?;
                            let name = match g.object(x, &format!("{RDFS}label")) {
                                Some(OxTerm::Literal(l)) => l.value().to_string(),
                                _ => f.clone(),
                            };
                            Some((name, f))
                        })
                        .collect()
                };
                let (result_data, result_graphs) = match &result {
                    Some(r) => (
                        g.objects(r, &format!("{UT}data"))
                            .filter_map(iri_of)
                            .collect(),
                        graphs(r),
                    ),
                    None => (Vec::new(), Vec::new()),
                };
                Kind::UpdateEval {
                    request: g
                        .object(&a, &format!("{UT}request"))
                        .and_then(|t| iri_of(&t))
                        .expect("request"),
                    data: g
                        .objects(&a, &format!("{UT}data"))
                        .filter_map(iri_of)
                        .collect(),
                    graph_data: graphs(&a),
                    result_data,
                    result_graphs,
                }
            }
            "PositiveSyntaxTest"
            | "PositiveSyntaxTest11"
            | "NegativeSyntaxTest"
            | "NegativeSyntaxTest11"
            | "PositiveUpdateSyntaxTest11"
            | "NegativeUpdateSyntaxTest11"
            | "PositiveUpdateSyntaxTest"
            | "NegativeUpdateSyntaxTest" => {
                let file = action.as_ref().and_then(iri_of).expect("syntax action");
                Kind::Syntax {
                    positive: ty.starts_with("Positive"),
                    update: ty.contains("Update") || file.ends_with(".ru"),
                    file,
                }
            }
            other => {
                unsupported.push((iri, other.to_string()));
                continue;
            }
        };
        out.push(Test { iri, kind });
    }
}

// ------------------------------------------------------------- running -----

fn standard(base: &str) -> QueryOptions {
    let mut o = QueryOptions::standard();
    o.base_iri = Some(base.to_string());
    o
}

fn ox_term_text(t: &OxTerm) -> String {
    t.to_string()
}

/// Loads quads into graph `target` (or their own graph when `None`) through
/// `INSERT DATA` in standard mode.
fn load(db: &Database, quads: &[Quad], target: Option<&str>) -> Result<(), String> {
    if quads.is_empty() {
        return Ok(());
    }
    let mut by_graph: BTreeMap<Option<String>, String> = BTreeMap::new();
    for q in quads {
        let g = match target {
            Some(t) => Some(t.to_string()),
            None => match &q.graph_name {
                GraphName::NamedNode(n) => Some(n.as_str().to_string()),
                GraphName::BlankNode(b) => Some(format!("urn:w3c-test:bnode-graph:{}", b.as_str())),
                GraphName::DefaultGraph => None,
            },
        };
        let body = by_graph.entry(g).or_default();
        let _ = writeln!(
            body,
            "{} <{}> {} .",
            ox_term_text(&subject_term(&q.subject)),
            q.predicate.as_str(),
            ox_term_text(&q.object)
        );
    }
    let mut text = String::from("INSERT DATA {\n");
    for (g, body) in &by_graph {
        match g {
            None => text.push_str(body),
            Some(g) => {
                let _ = write!(text, "GRAPH <{g}> {{\n{body}}}\n");
            }
        }
    }
    text.push('}');
    db.sparql_update_with(&text, Durability::Relaxed, &QueryOptions::standard())
        .map(|_| ())
        .map_err(|e| format!("loading data failed: {e}"))
}

/// The expected result of a query test.
enum Expected {
    Solutions(Vec<String>, Vec<HashMap<String, T>>),
    Boolean(bool),
    Graph(Vec<[T; 3]>),
}

fn parse_expected(root: &Path, iri: &str) -> Result<Expected, String> {
    let fmt = if iri.ends_with(".srx") {
        Some(QueryResultsFormat::Xml)
    } else if iri.ends_with(".srj") {
        Some(QueryResultsFormat::Json)
    } else if iri.ends_with(".tsv") {
        Some(QueryResultsFormat::Tsv)
    } else if iri.ends_with(".csv") {
        Some(QueryResultsFormat::Csv)
    } else {
        None
    };
    if let Some(fmt) = fmt {
        let bytes = read(root, iri)?;
        return match QueryResultsParser::from_format(fmt)
            .for_slice(&bytes)
            .map_err(|e| format!("{iri}: {e}"))?
        {
            SliceQueryResultsParserOutput::Boolean(b) => Ok(Expected::Boolean(b)),
            SliceQueryResultsParserOutput::Solutions(s) => {
                let vars = s
                    .variables()
                    .iter()
                    .map(|v| v.as_str().to_string())
                    .collect();
                let mut rows = Vec::new();
                for sol in s {
                    let sol = sol.map_err(|e| format!("{iri}: {e}"))?;
                    rows.push(
                        sol.iter()
                            .map(|(v, t)| (v.as_str().to_string(), from_ox(t)))
                            .collect(),
                    );
                }
                Ok(Expected::Solutions(vars, rows))
            }
        };
    }
    // RDF: a graph, or a result set in the `rs:` vocabulary.
    let g = Graph::load(root, iri)?;
    let sets = g.subjects_with(&format!("{RDF}type"), &format!("{RS}ResultSet"));
    if let Some(rs) = sets.first() {
        if let Some(OxTerm::Literal(b)) = g.object(rs, &format!("{RS}boolean")) {
            return Ok(Expected::Boolean(b.value() == "true"));
        }
        let vars = g
            .objects(rs, &format!("{RS}resultVariable"))
            .filter_map(|t| match t {
                OxTerm::Literal(l) => Some(l.value().to_string()),
                _ => None,
            })
            .collect();
        let mut rows = Vec::new();
        for sol in g.objects(rs, &format!("{RS}solution")) {
            let mut row = HashMap::new();
            for b in g.objects(sol, &format!("{RS}binding")) {
                if let (Some(OxTerm::Literal(v)), Some(val)) = (
                    g.object(b, &format!("{RS}variable")),
                    g.object(b, &format!("{RS}value")),
                ) {
                    row.insert(v.value().to_string(), from_ox(&val));
                }
            }
            rows.push(row);
        }
        return Ok(Expected::Solutions(vars, rows));
    }
    Ok(Expected::Graph(g.0.iter().map(triple_ts).collect()))
}

/// Whether the query's outermost solution sequence is ordered.
fn is_ordered(q: &Query) -> bool {
    fn top(p: &GraphPattern) -> bool {
        match p {
            GraphPattern::OrderBy { .. } => true,
            GraphPattern::Project { inner, .. }
            | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. } => top(inner),
            _ => false,
        }
    }
    match q {
        Query::Select { pattern, .. } => top(pattern),
        _ => false,
    }
}

fn graph_rows(ts: &[[T; 3]]) -> Vec<Row> {
    let set: HashSet<&[T; 3]> = ts.iter().collect();
    let mut v: Vec<Row> = set
        .into_iter()
        .map(|t| t.iter().cloned().map(Some).collect())
        .collect();
    v.sort();
    v
}

fn show_rows(rows: &[Row]) -> String {
    let mut s = String::new();
    for r in rows.iter().take(40) {
        let cells: Vec<String> = r
            .iter()
            .map(|c| c.as_ref().map_or_else(|| "-".to_string(), T::to_string))
            .collect();
        let _ = writeln!(s, "    {}", cells.join("  "));
    }
    if rows.len() > 40 {
        let _ = writeln!(s, "    … {} rows", rows.len());
    }
    s
}

fn fresh_db() -> Result<(tempfile::TempDir, Database), String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let db = Database::open(dir.path().join("w3c.glv"), OpenOptions::default())
        .map_err(|e| e.to_string())?;
    Ok((dir, db))
}

fn run_query_test(
    root: &Path,
    query: &str,
    data: &[String],
    graph_data: &[(String, String)],
    result: Option<&str>,
) -> Result<(), String> {
    let text = String::from_utf8(read(root, query)?).map_err(|e| e.to_string())?;
    let parsed = SparqlParser::new()
        .with_base_iri(query)
        .map_err(|e| e.to_string())?
        .parse_query(&text)
        .map_err(|e| format!("spargebra rejects the query: {e}"))?;
    let (_dir, db) = fresh_db()?;
    let mut loaded = HashSet::new();
    for d in data {
        load(&db, &parse_rdf(root, d)?, None)?;
    }
    for (name, f) in graph_data {
        load(&db, &parse_rdf(root, f)?, Some(name))?;
        loaded.insert(name.clone());
    }
    // FROM / FROM NAMED graphs of the query itself are loaded as named graphs.
    if data.is_empty() && graph_data.is_empty() {
        let ds = match &parsed {
            Query::Select { dataset, .. }
            | Query::Ask { dataset, .. }
            | Query::Construct { dataset, .. }
            | Query::Describe { dataset, .. } => dataset.clone(),
        };
        if let Some(ds) = ds {
            for g in ds.default.iter().chain(ds.named.iter().flatten()) {
                let i = g.as_str();
                if iri_to_path(root, i).is_some() && loaded.insert(i.to_string()) {
                    load(&db, &parse_rdf(root, i)?, Some(i))?;
                }
            }
        }
    }
    let actual = db
        .sparql_with(&text, &standard(query))
        .map_err(|e| format!("query failed: {e}"))?;
    let Some(result) = result else {
        return Ok(());
    };
    let expected = parse_expected(root, result)?;
    match (expected, actual) {
        (Expected::Boolean(e), QueryResult::Boolean(a)) => {
            if e == a {
                Ok(())
            } else {
                Err(format!("expected {e}, got {a}"))
            }
        }
        (Expected::Solutions(evars, erows), QueryResult::Solutions { vars, rows }) => {
            let mut all: Vec<String> = evars.clone();
            for v in &vars {
                if !all.contains(v) {
                    all.push(v.clone());
                }
            }
            for r in &erows {
                for k in r.keys() {
                    if !all.contains(k) {
                        all.push(k.clone());
                    }
                }
            }
            let exp: Vec<Row> = erows
                .iter()
                .map(|r| all.iter().map(|v| r.get(v).cloned()).collect())
                .collect();
            let act: Vec<Row> = rows
                .iter()
                .map(|r| {
                    all.iter()
                        .map(|v| {
                            vars.iter()
                                .position(|x| x == v)
                                .and_then(|i| r[i].as_ref().map(from_gl))
                        })
                        .collect()
                })
                .collect();
            let ordered = is_ordered(&parsed);
            if rows_isomorphic(&exp, &act, ordered) {
                Ok(())
            } else {
                let (mut se, mut sa) = (exp.clone(), act.clone());
                if !ordered {
                    se.sort();
                    sa.sort();
                }
                Err(format!(
                    "solutions differ{} (vars {})\n  expected {}:\n{}  actual {}:\n{}",
                    if ordered { " (ordered)" } else { "" },
                    all.join(" "),
                    exp.len(),
                    show_rows(&se),
                    act.len(),
                    show_rows(&sa)
                ))
            }
        }
        (Expected::Graph(e), QueryResult::Graph(a)) => {
            let a: Vec<[T; 3]> = a
                .iter()
                .map(|t| [from_gl(&t[0]), from_gl(&t[1]), from_gl(&t[2])])
                .collect();
            let (e, a) = (graph_rows(&e), graph_rows(&a));
            if rows_isomorphic(&e, &a, false) {
                Ok(())
            } else {
                Err(format!(
                    "graphs differ\n  expected {}:\n{}  actual {}:\n{}",
                    e.len(),
                    show_rows(&e),
                    a.len(),
                    show_rows(&a)
                ))
            }
        }
        (_, a) => Err(format!(
            "result form differs from the expected one: got {a:?}"
        )),
    }
}

/// The whole store in standard mode as rows (graph, s, p, o).
fn dump(db: &Database) -> Result<Vec<Row>, String> {
    let o = QueryOptions::standard();
    let mut out = Vec::new();
    let q = |text: &str| {
        db.sparql_with(text, &o)
            .map_err(|e| format!("dump query failed: {e}"))
    };
    for r in q("SELECT ?s ?p ?o WHERE { ?s ?p ?o }")?.rows() {
        let mut row = vec![Some(T::Default)];
        row.extend(r.iter().map(|x| x.as_ref().map(from_gl)));
        out.push(row);
    }
    for r in q("SELECT ?g ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } }")?.rows() {
        if let Some(graphlava::Term::Iri(g)) = &r[0] {
            if g.starts_with("https://graphlava.org/ns#") {
                continue;
            }
        }
        out.push(r.iter().map(|x| x.as_ref().map(from_gl)).collect());
    }
    Ok(out)
}

fn quads_rows(qs: &[Quad], target: Option<&str>) -> Vec<Row> {
    qs.iter()
        .map(|q| {
            let g = match (target, &q.graph_name) {
                (Some(t), _) => T::Iri(t.to_string()),
                (None, GraphName::NamedNode(n)) => T::Iri(n.as_str().to_string()),
                (None, GraphName::BlankNode(b)) => {
                    T::Iri(format!("urn:w3c-test:bnode-graph:{}", b.as_str()))
                }
                (None, GraphName::DefaultGraph) => T::Default,
            };
            vec![
                Some(g),
                Some(from_subject(&q.subject)),
                Some(T::Iri(q.predicate.as_str().into())),
                Some(from_ox(&q.object)),
            ]
        })
        .collect()
}

fn dedup(mut v: Vec<Row>) -> Vec<Row> {
    v.sort();
    v.dedup();
    v
}

fn run_update_test(
    root: &Path,
    request: &str,
    data: &[String],
    graph_data: &[(String, String)],
    result_data: &[String],
    result_graphs: &[(String, String)],
) -> Result<(), String> {
    let text = String::from_utf8(read(root, request)?).map_err(|e| e.to_string())?;
    let (_dir, db) = fresh_db()?;
    for d in data {
        load(&db, &parse_rdf(root, d)?, None)?;
    }
    for (name, f) in graph_data {
        load(&db, &parse_rdf(root, f)?, Some(name))?;
    }
    db.sparql_update_with(&text, Durability::Relaxed, &standard(request))
        .map_err(|e| format!("update failed: {e}"))?;
    let mut exp = Vec::new();
    for d in result_data {
        exp.extend(quads_rows(&parse_rdf(root, d)?, None));
    }
    for (name, f) in result_graphs {
        exp.extend(quads_rows(&parse_rdf(root, f)?, Some(name)));
    }
    let (exp, act) = (dedup(exp), dedup(dump(&db)?));
    if rows_isomorphic(&exp, &act, false) {
        Ok(())
    } else {
        Err(format!(
            "datasets differ\n  expected {}:\n{}  actual {}:\n{}",
            exp.len(),
            show_rows(&exp),
            act.len(),
            show_rows(&act)
        ))
    }
}

fn run_syntax_test(root: &Path, file: &str, positive: bool, update: bool) -> Result<(), String> {
    let text = String::from_utf8(read(root, file)?).map_err(|e| format!("not UTF-8: {e}"));
    let parsed: Result<(), String> = match text {
        Err(e) => Err(e),
        Ok(text) if update => SparqlParser::new()
            .with_base_iri(file)
            .map_err(|e| e.to_string())?
            .parse_update(&graphlava::sparql::prepass(&text))
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Ok(text) => graphlava::sparql::parse_query(&text, Some(file))
            .map(|_| ())
            .map_err(|e| e.to_string()),
    };
    match (positive, parsed) {
        (true, Ok(())) | (false, Err(_)) => Ok(()),
        (true, Err(e)) => Err(format!("rejected a valid request: {e}")),
        (false, Ok(())) => Err("accepted an invalid request".into()),
    }
}

fn run_test(root: &Path, t: &Test) -> Result<(), String> {
    match &t.kind {
        Kind::QueryEval {
            query,
            data,
            graph_data,
            result,
        } => run_query_test(root, query, data, graph_data, result.as_deref()),
        Kind::UpdateEval {
            request,
            data,
            graph_data,
            result_data,
            result_graphs,
        } => run_update_test(root, request, data, graph_data, result_data, result_graphs),
        Kind::Syntax {
            file,
            positive,
            update,
        } => run_syntax_test(root, file, *positive, *update),
    }
}

/// Runs one test on its own thread with a deadline; panics become failures.
fn run_isolated(root: &Path, t: &Test, timeout: Duration) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let (root, t) = (root.to_path_buf(), t.clone());
    std::thread::Builder::new()
        .name("w3c-test".into())
        .stack_size(64 << 20)
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_test(&root, &t)));
            let r = r.unwrap_or_else(|p| {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                Err(format!("panic: {msg}"))
            });
            let _ = tx.send(r);
        })
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(timeout)
        .unwrap_or_else(|_| Err(format!("timed out after {}s", timeout.as_secs())))
}

// ------------------------------------------------------------ skip list ----

fn skip_list(path: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (iri, reason) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let reason = reason.trim();
        assert!(
            !reason.is_empty(),
            "skip.txt line {}: {iri} has no reason",
            n + 1
        );
        assert!(
            out.insert(iri.to_string(), reason.to_string()).is_none(),
            "skip.txt: {iri} listed twice"
        );
    }
    out
}

fn pinned_commit(fetch_sh: &Path) -> String {
    std::fs::read_to_string(fetch_sh)
        .expect("fetch.sh")
        .lines()
        .find_map(|l| l.strip_prefix("COMMIT=").map(str::to_string))
        .expect("COMMIT= in fetch.sh")
}

#[test]
fn w3c_sparql_conformance() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let here = manifest_dir.join("tests/w3c");
    let root = std::env::var_os("GRAPHLAVA_W3C_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../target/w3c-tests"));
    let require = std::env::var("GRAPHLAVA_REQUIRE_W3C").is_ok_and(|v| v == "1");
    let commit = pinned_commit(&here.join("fetch.sh"));
    let present = std::fs::read_to_string(root.join(".commit"))
        .ok()
        .map(|s| s.trim().to_string());
    if present.as_deref() != Some(commit.as_str()) {
        let msg = format!(
            "W3C suites at {commit} not found in {} (found {present:?}); run crates/graphlava/tests/w3c/fetch.sh",
            root.display()
        );
        assert!(!require, "{msg}");
        eprintln!("skipping: {msg}");
        return;
    }
    let root = root.canonicalize().expect("suite root");
    let filter = std::env::var("GRAPHLAVA_W3C_FILTER").ok();
    let verbose = std::env::var("GRAPHLAVA_W3C_VERBOSE").is_ok_and(|v| v == "1");
    let timeout = Duration::from_secs(
        std::env::var("GRAPHLAVA_W3C_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
    );
    let skips = skip_list(&here.join("skip.txt"));
    let mut seen = HashSet::new();
    let mut unexpected_fail = Vec::new();
    let mut unexpected_pass = Vec::new();
    let mut all_failures = Vec::new();
    let mut summary = String::new();

    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for (suite, manifest) in SUITES {
        let mut tests = Vec::new();
        let mut unsupported = Vec::new();
        collect_tests(
            &root,
            &path_to_iri(&root, &root.join(manifest)),
            &mut tests,
            &mut unsupported,
        );
        let (mut passed, mut skipped, mut failed) = (0, 0, 0);
        for t in &tests {
            if filter.as_ref().is_some_and(|f| !t.iri.contains(f.as_str())) {
                continue;
            }
            seen.insert(t.iri.clone());
            let r = run_isolated(&root, t, timeout);
            if let Err(e) = &r {
                all_failures.push(format!("{}  {}", t.iri, e.lines().next().unwrap_or("")));
            }
            match (r, skips.get(&t.iri)) {
                (Ok(()), None) => passed += 1,
                (Ok(()), Some(_)) => {
                    failed += 1;
                    unexpected_pass.push(t.iri.clone());
                }
                (Err(_), Some(_)) => skipped += 1,
                (Err(e), None) => {
                    failed += 1;
                    if verbose {
                        eprintln!("FAIL {}\n  {e}", t.iri);
                    }
                    unexpected_fail.push(format!(
                        "{}\n    {}",
                        t.iri,
                        e.lines().next().unwrap_or("")
                    ));
                }
            }
        }
        let _ = writeln!(
            summary,
            "{suite:16} passed {passed:4}  skipped {skipped:4}  failed {failed:4}  (not run: {} tests of types without a runner)",
            unsupported.len()
        );
    }
    std::panic::set_hook(prev_hook);
    let _ = std::fs::write(
        root.join("last-failures.txt"),
        all_failures.join("\n") + "\n",
    );
    let stale: Vec<&String> = if filter.is_none() {
        skips.keys().filter(|k| !seen.contains(*k)).collect()
    } else {
        Vec::new()
    };
    eprintln!("W3C SPARQL conformance (rdf-tests {commit}, standard mode)\n{summary}");
    let mut problems = String::new();
    if !unexpected_fail.is_empty() {
        let _ = writeln!(
            problems,
            "{} failing tests not in skip.txt:\n  {}",
            unexpected_fail.len(),
            unexpected_fail.join("\n  ")
        );
    }
    if !unexpected_pass.is_empty() {
        let _ = writeln!(
            problems,
            "{} tests in skip.txt now pass; remove them:\n  {}",
            unexpected_pass.len(),
            unexpected_pass.join("\n  ")
        );
    }
    if !stale.is_empty() {
        let _ = writeln!(
            problems,
            "{} skip.txt entries name no test of the suites:\n  {}",
            stale.len(),
            stale
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        );
    }
    assert!(problems.is_empty(), "{problems}");
}
