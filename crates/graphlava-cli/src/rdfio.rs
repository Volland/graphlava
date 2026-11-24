//! `import` and `export`: RDF 1.2 in N-Triples, N-Quads, Turtle and TriG.
//!
//! Import maps the RDF 1.2 reification vocabulary onto statement IDs, the way
//! SPARQL Update does: a quad `r rdf:reifies <<( s p o )>>` makes `r` stand
//! for the statement of `(s, p, o)` (created unasserted when the triple is not
//! asserted), so `r :confidence 0.9` annotates that statement. A blank-node
//! reifier is just the statement ID; an IRI reifier also becomes an alias of
//! it. Quads go through [`Database::submit`] in relaxed batches, and one
//! durable flush ends the import.
//!
//! Export walks every statement with the core `find` API and writes the
//! inverse mapping, so import → export → import gives the same dataset.

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::time::Instant;

use graphlava::id::{DEFAULT_GRAPH, Kind, SYSTEM_GRAPH};
use graphlava::term::{DEFAULT_GRAPH_IRI, META_IN, STMT_IRI_PREFIX, SYSTEM_GRAPH_IRI};
use graphlava::{
    CoreOp, Database, Durability, Id, Literal, OpResult, Reader, Term, TermRef, WriteRequest,
};
use oxrdf::{
    BlankNode, GraphName, NamedNode, NamedOrBlankNode, Quad as OxQuad, Term as OxTerm,
    Triple as OxTriple,
};
use oxttl::{
    NQuadsParser, NQuadsSerializer, NTriplesParser, NTriplesSerializer, TriGParser, TriGSerializer,
    TurtleParser, TurtleSerializer,
};

use crate::cli::{ExportArgs, ImportArgs, RdfFormat};
use crate::error::{CliError, CliResult, io_ctx};
use crate::run::{Ctx, ms, open_existing, open_or_create};

const RDF_REIFIES: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies";

/// The format of a path from its extension.
pub fn format_of(p: &Path) -> Option<RdfFormat> {
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "nt" => RdfFormat::Nt,
        "nq" => RdfFormat::Nq,
        "ttl" | "turtle" => RdfFormat::Ttl,
        "trig" => RdfFormat::Trig,
        _ => return None,
    })
}

// ---- parsing -----------------------------------------------------------------

type QuadIter<'a> = Box<dyn Iterator<Item = Result<OxQuad, String>> + 'a>;

fn parse<'a>(bytes: &'a [u8], fmt: RdfFormat, base: Option<&str>) -> CliResult<QuadIter<'a>> {
    let bad_base = |e: oxrdf::IriParseError| CliError::Usage(format!("invalid --base IRI: {e}"));
    let dg = |t: OxTriple| t.in_graph(GraphName::DefaultGraph);
    Ok(match fmt {
        RdfFormat::Nt => Box::new(
            NTriplesParser::new()
                .for_slice(bytes)
                .map(move |r| r.map(dg).map_err(|e| e.to_string())),
        ),
        RdfFormat::Nq => Box::new(
            NQuadsParser::new()
                .for_slice(bytes)
                .map(|r| r.map_err(|e| e.to_string())),
        ),
        RdfFormat::Ttl => {
            let mut p = TurtleParser::new();
            if let Some(b) = base {
                p = p.with_base_iri(b).map_err(bad_base)?;
            }
            Box::new(
                p.for_slice(bytes)
                    .map(move |r| r.map(dg).map_err(|e| e.to_string())),
            )
        }
        RdfFormat::Trig => {
            let mut p = TriGParser::new();
            if let Some(b) = base {
                p = p.with_base_iri(b).map_err(bad_base)?;
            }
            Box::new(p.for_slice(bytes).map(|r| r.map_err(|e| e.to_string())))
        }
    })
}

// ---- import ------------------------------------------------------------------

/// What an import did.
#[derive(Debug, Default)]
pub struct ImportReport {
    pub quads: u64,
    pub statements: u64,
    pub reifiers: u64,
    pub nestings: u64,
    pub graphs: usize,
}

/// A node that stands for a statement: a blank node, a statement IRI from
/// another graphlava file, or an IRI reifier.
fn reifier_key(s: &NamedOrBlankNode) -> String {
    match s {
        NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
        NamedOrBlankNode::NamedNode(n) => format!("<{}>", n.as_str()),
    }
}

fn is_stmt_iri(iri: &str) -> bool {
    iri.strip_prefix(STMT_IRI_PREFIX)
        .is_some_and(|n| n.parse::<u64>().is_ok())
}

struct Importer<'d> {
    db: &'d Database,
    batch: usize,
    ops: Vec<CoreOp>,
    /// Blank-node labels of the file mapped to blank nodes of the database.
    blanks: HashMap<String, Id>,
    /// Reifier keys mapped to the triple they reify.
    reifiers: HashMap<String, OxTriple>,
    /// IRI reifiers whose alias is written.
    aliased: HashSet<String>,
    /// Triple terms already resolved to statement IDs.
    triple_ids: HashMap<OxTriple, Id>,
    graph: Option<String>,
    graphs: HashSet<String>,
    rep: ImportReport,
}

impl Importer<'_> {
    fn flush(&mut self) -> CliResult<()> {
        if self.ops.is_empty() {
            return Ok(());
        }
        let ops = std::mem::take(&mut self.ops);
        self.db
            .submit(WriteRequest { ops }, Durability::Relaxed)
            .map_err(|e| {
                CliError::Failed(format!(
                    "import failed in the batch ending at quad {}: {e}",
                    self.rep.quads
                ))
            })?;
        Ok(())
    }

    fn push(&mut self, op: CoreOp) -> CliResult<()> {
        self.ops.push(op);
        if self.ops.len() >= self.batch {
            self.flush()?;
        }
        Ok(())
    }

    /// Whether a term mentions a blank node that is not a reifier, at any
    /// depth: such a triple term must be resolved to an ID first, since the
    /// blank labels of one request do not reach the next.
    fn needs_ids(&self, t: &OxTerm) -> bool {
        match t {
            OxTerm::BlankNode(b) => !self.reifiers.contains_key(&format!("_:{}", b.as_str())),
            OxTerm::NamedNode(n) => self.reifiers.contains_key(&format!("<{}>", n.as_str())),
            OxTerm::Literal(_) => false,
            OxTerm::Triple(t) => {
                self.needs_ids(&t.subject.clone().into()) || self.needs_ids(&t.object)
            }
        }
    }

    /// The statement ID of a triple term: flushes the open batch, then
    /// writes the statement unasserted unless the triple exists.
    fn triple_id(&mut self, t: &OxTriple) -> CliResult<Id> {
        if let Some(id) = self.triple_ids.get(t) {
            return Ok(*id);
        }
        let s = self.term(&t.subject.clone().into())?;
        let o = self.term(&t.object)?;
        let p = TermRef::Term(Term::Iri(t.predicate.as_str().into()));
        self.flush()?;
        let ticket = self.db.submit(
            WriteRequest {
                ops: vec![CoreOp::InsertUnasserted { s, p, o }],
            },
            Durability::Relaxed,
        )?;
        let id = match ticket.results.first() {
            Some(OpResult::Id(Some(id))) => *id,
            other => {
                return Err(CliError::Failed(format!(
                    "unexpected result for a triple term: {other:?}"
                )));
            }
        };
        self.triple_ids.insert(t.clone(), id);
        Ok(id)
    }

    /// A triple term as a term the store resolves itself (no blank nodes).
    fn plain_triple(&self, t: &OxTriple) -> Term {
        Term::triple(
            self.plain(&t.subject.clone().into()),
            Term::Iri(t.predicate.as_str().into()),
            self.plain(&t.object),
        )
    }

    fn plain(&self, t: &OxTerm) -> Term {
        match t {
            OxTerm::NamedNode(n) => match self.reifiers.get(&format!("<{}>", n.as_str())) {
                Some(tt) => self.plain_triple(tt),
                None => Term::Iri(n.as_str().into()),
            },
            OxTerm::BlankNode(b) => match self.reifiers.get(&format!("_:{}", b.as_str())) {
                Some(tt) => self.plain_triple(tt),
                None => unreachable!("needs_ids is checked first"),
            },
            OxTerm::Literal(l) => Term::Literal(literal(l)),
            OxTerm::Triple(tt) => self.plain_triple(tt),
        }
    }

    /// A term of the file as a term to write.
    fn term(&mut self, t: &OxTerm) -> CliResult<TermRef> {
        Ok(match t {
            OxTerm::NamedNode(n) => {
                let key = format!("<{}>", n.as_str());
                if let Some(tt) = self.reifiers.get(&key).cloned() {
                    self.alias(n.as_str(), &tt)?;
                    self.triple(&tt)?
                } else {
                    TermRef::Term(Term::Iri(n.as_str().into()))
                }
            }
            OxTerm::BlankNode(b) => {
                let key = format!("_:{}", b.as_str());
                if let Some(tt) = self.reifiers.get(&key).cloned() {
                    self.triple(&tt)?
                } else {
                    TermRef::Id(self.blanks[b.as_str()])
                }
            }
            OxTerm::Literal(l) => TermRef::Term(Term::Literal(literal(l))),
            OxTerm::Triple(tt) => self.triple(tt)?,
        })
    }

    fn triple(&mut self, t: &OxTriple) -> CliResult<TermRef> {
        if self.needs_ids(&OxTerm::Triple(Box::new(t.clone()))) {
            Ok(TermRef::Id(self.triple_id(t)?))
        } else {
            Ok(TermRef::Term(self.plain_triple(t)))
        }
    }

    /// Writes `<iri>` as an alias of the statement it reifies, once, through
    /// SPARQL Update (the core API has no alias operation).
    fn alias(&mut self, iri: &str, t: &OxTriple) -> CliResult<()> {
        if !self.aliased.insert(iri.to_string()) {
            return Ok(());
        }
        if is_stmt_iri(iri) {
            return Ok(()); // a statement IRI of another file: no alias
        }
        if self.needs_ids(&OxTerm::Triple(Box::new(t.clone()))) {
            return Err(CliError::Failed(format!(
                "the IRI reifier <{iri}> reifies a triple with blank nodes; \
                 use a blank-node reifier instead"
            )));
        }
        let tt = self.plain_triple(t);
        self.flush()?;
        let upd = format!(
            "INSERT DATA {{ <{iri}> <{RDF_REIFIES}> {} }}",
            crate::render::nt(&tt)
        );
        self.db
            .sparql_update(&upd, Durability::Relaxed)
            .map_err(|e| CliError::Failed(format!("cannot write reifier <{iri}>: {e}")))?;
        Ok(())
    }

    fn graph_of(&mut self, q: &OxQuad, subject_is_stmt: bool) -> CliResult<Option<TermRef>> {
        let name = match &q.graph_name {
            GraphName::NamedNode(n) => Some(n.as_str().to_string()),
            GraphName::BlankNode(b) => {
                return Err(CliError::Failed(format!(
                    "blank-node graph names are not supported (_:{})",
                    b.as_str()
                )));
            }
            GraphName::DefaultGraph => self.graph.clone(),
        };
        Ok(match name {
            Some(n) if n == DEFAULT_GRAPH_IRI => Some(TermRef::Id(DEFAULT_GRAPH)),
            Some(n) => {
                if !self.graphs.contains(&n) {
                    self.graphs.insert(n.clone());
                }
                Some(TermRef::Term(Term::Iri(n)))
            }
            // An annotation goes with its statement, as in SPARQL Update.
            None if subject_is_stmt => None,
            None => Some(TermRef::Id(DEFAULT_GRAPH)),
        })
    }

    fn quad(&mut self, q: &OxQuad) -> CliResult<()> {
        self.rep.quads += 1;
        if q.predicate.as_str() == RDF_REIFIES {
            if let OxTerm::Triple(t) = &q.object {
                // The reifier: make sure the statement exists.
                if let NamedOrBlankNode::NamedNode(n) = &q.subject {
                    self.alias(n.as_str(), t)?;
                }
                let s = self.term(&t.subject.clone().into())?;
                let o = self.term(&t.object)?;
                let p = TermRef::Term(Term::Iri(t.predicate.as_str().into()));
                self.push(CoreOp::InsertUnasserted { s, p, o })?;
                self.rep.reifiers += 1;
                return Ok(());
            }
        }
        let subject: OxTerm = q.subject.clone().into();
        let subject_is_stmt = self.reifiers.contains_key(&reifier_key(&q.subject))
            || matches!(&q.subject, NamedOrBlankNode::NamedNode(n) if is_stmt_iri(n.as_str()));
        if q.predicate.as_str() == META_IN && !subject_is_stmt {
            // Graph nesting between two graph names.
            if let (NamedOrBlankNode::NamedNode(c), OxTerm::NamedNode(p)) = (&q.subject, &q.object)
            {
                self.push(CoreOp::Nest {
                    child: TermRef::Term(Term::Iri(c.as_str().into())),
                    parent: TermRef::Term(Term::Iri(p.as_str().into())),
                })?;
                self.rep.nestings += 1;
                return Ok(());
            }
        }
        if q.predicate.as_str() == META_IN && subject_is_stmt {
            // An explicit membership of a statement in a graph.
            if let OxTerm::NamedNode(g) = &q.object {
                let key = reifier_key(&q.subject);
                if let Some(t) = self.reifiers.get(&key).cloned() {
                    let item = self.triple_id(&t)?;
                    self.push(CoreOp::AddMember {
                        item,
                        graph: TermRef::Term(Term::Iri(g.as_str().into())),
                    })?;
                    return Ok(());
                }
            }
        }
        let s = self.term(&subject)?;
        let o = self.term(&q.object)?;
        let p = TermRef::Term(Term::Iri(q.predicate.as_str().into()));
        let graph = self.graph_of(q, subject_is_stmt)?;
        self.push(CoreOp::Insert { s, p, o, graph })?;
        self.rep.statements += 1;
        Ok(())
    }
}

fn literal(l: &oxrdf::Literal) -> Literal {
    match l.language() {
        Some(tag) => Literal::lang(l.value(), tag),
        None => Literal::typed(l.value(), l.datatype().as_str()),
    }
}

/// Loads RDF into a database. Blank nodes of the file become blank nodes of
/// the database (one per label), reifiers become statement IDs.
pub fn import(
    db: &Database,
    bytes: &[u8],
    fmt: RdfFormat,
    base: Option<&str>,
    graph: Option<String>,
    batch: usize,
    progress: &mut dyn FnMut(u64),
) -> CliResult<ImportReport> {
    // Parse once: generated blank-node labels (`[]`, `{| |}`) differ
    // between two parses of the same text.
    let mut parsed = Vec::new();
    for q in parse(bytes, fmt, base)? {
        parsed.push(q.map_err(|e| CliError::Failed(format!("parse error: {e}")))?);
    }
    // Reifiers and blank-node labels.
    let mut reifiers = HashMap::new();
    let mut labels: HashSet<String> = HashSet::new();
    fn walk(t: &OxTerm, labels: &mut HashSet<String>) {
        match t {
            OxTerm::BlankNode(b) => {
                labels.insert(b.as_str().to_string());
            }
            OxTerm::Triple(t) => {
                walk(&t.subject.clone().into(), labels);
                walk(&t.object, labels);
            }
            _ => {}
        }
    }
    for q in &parsed {
        if q.predicate.as_str() == RDF_REIFIES {
            if let OxTerm::Triple(t) = &q.object {
                reifiers
                    .entry(reifier_key(&q.subject))
                    .or_insert_with(|| (**t).clone());
            }
        }
        walk(&q.subject.clone().into(), &mut labels);
        walk(&q.object, &mut labels);
    }
    for k in reifiers.keys() {
        if let Some(l) = k.strip_prefix("_:") {
            labels.remove(l);
        }
    }
    // Allocate one database blank node per remaining label.
    let mut blanks = HashMap::with_capacity(labels.len());
    let labels: Vec<String> = labels.into_iter().collect();
    for chunk in labels.chunks(4096) {
        let mut w = db.write(Durability::Relaxed)?;
        for l in chunk {
            let id = w.intern(Term::Blank(l.clone()))?;
            blanks.insert(l.clone(), id);
        }
        w.commit()?;
    }
    // Pass 2: write.
    let mut im = Importer {
        db,
        batch: batch.max(1),
        ops: Vec::new(),
        blanks,
        reifiers,
        aliased: HashSet::new(),
        triple_ids: HashMap::new(),
        graph,
        graphs: HashSet::new(),
        rep: ImportReport::default(),
    };
    for q in &parsed {
        im.quad(q)?;
        if im.rep.quads.is_multiple_of(100_000) {
            progress(im.rep.quads);
        }
    }
    im.flush()?;
    db.flush()?;
    im.rep.graphs = im.graphs.len();
    Ok(im.rep)
}

pub fn import_cmd(ctx: Ctx, a: ImportArgs) -> CliResult<u8> {
    let stdin = a.data.as_os_str() == "-";
    let fmt = match a.format.or_else(|| format_of(&a.data)) {
        Some(f) => f,
        None if stdin => {
            return Err(CliError::Usage(
                "reading stdin needs --format nt|nq|ttl|trig".into(),
            ));
        }
        None => {
            return Err(CliError::Usage(format!(
                "cannot tell the format of {} from its extension; pass --format nt|nq|ttl|trig",
                a.data.display()
            )));
        }
    };
    let bytes = if stdin {
        let mut b = Vec::new();
        std::io::stdin()
            .read_to_end(&mut b)
            .map_err(io_ctx("cannot read stdin"))?;
        b
    } else {
        std::fs::read(&a.data).map_err(io_ctx(format!("cannot read {}", a.data.display())))?
    };
    let db = open_or_create(&a.file)?;
    let t0 = Instant::now();
    let tty = std::io::stderr().is_terminal();
    let quiet = ctx.quiet;
    let mut progress = |n: u64| {
        if quiet {
            return;
        }
        if tty {
            eprint!("\rimported {n} quads…");
        } else {
            eprintln!("imported {n} quads…");
        }
    };
    let rep = import(
        &db,
        &bytes,
        fmt,
        a.base.as_deref(),
        a.graph,
        a.batch,
        &mut progress,
    );
    if tty && !quiet && bytes.len() > 1 << 20 {
        eprint!("\r");
    }
    let rep = rep?;
    ctx.note(format!(
        "imported {} quads: {} statements, {} reifiers, {} nestings, {} graphs ({})",
        rep.quads,
        rep.statements,
        rep.reifiers,
        rep.nestings,
        rep.graphs,
        ms(t0.elapsed())
    ));
    db.close()?;
    Ok(0)
}

// ---- export ------------------------------------------------------------------

enum Sink<W: Write> {
    Nt(oxttl::ntriples::WriterNTriplesSerializer<W>),
    Nq(oxttl::nquads::WriterNQuadsSerializer<W>),
    Ttl(oxttl::turtle::WriterTurtleSerializer<W>),
    /// TriG groups quads by graph, so they are collected first.
    Trig(Option<W>, Vec<OxQuad>),
}

impl<W: Write> Sink<W> {
    fn new(fmt: RdfFormat, w: W) -> Sink<W> {
        let prefixes = [
            ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
            ("xsd", "http://www.w3.org/2001/XMLSchema#"),
            ("meta", "https://graphlava.org/ns#"),
        ];
        match fmt {
            RdfFormat::Nt => Sink::Nt(NTriplesSerializer::new().for_writer(w)),
            RdfFormat::Nq => Sink::Nq(NQuadsSerializer::new().for_writer(w)),
            RdfFormat::Ttl => {
                let mut s = TurtleSerializer::new();
                for (p, i) in prefixes {
                    s = s.with_prefix(p, i).expect("valid prefix");
                }
                Sink::Ttl(s.for_writer(w))
            }
            RdfFormat::Trig => Sink::Trig(Some(w), Vec::new()),
        }
    }

    fn quads(&self) -> bool {
        matches!(self, Sink::Nq(_) | Sink::Trig(..))
    }

    fn put(&mut self, q: OxQuad) -> std::io::Result<()> {
        match self {
            Sink::Nt(s) => s.serialize_triple(&OxTriple::from(q)),
            Sink::Ttl(s) => s.serialize_triple(&OxTriple::from(q)),
            Sink::Nq(s) => s.serialize_quad(&q),
            Sink::Trig(_, v) => {
                v.push(q);
                Ok(())
            }
        }
    }

    fn finish(self) -> std::io::Result<W> {
        match self {
            Sink::Nt(s) => Ok(s.finish()),
            Sink::Nq(s) => Ok(s.finish()),
            Sink::Ttl(s) => s.finish(),
            Sink::Trig(w, mut v) => {
                v.sort_by_cached_key(|q| match &q.graph_name {
                    GraphName::DefaultGraph => String::new(),
                    g => g.to_string(),
                });
                let mut s = TriGSerializer::new();
                for (p, i) in [
                    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
                    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
                    ("meta", "https://graphlava.org/ns#"),
                ] {
                    s = s.with_prefix(p, i).expect("valid prefix");
                }
                let mut s = s.for_writer(w.expect("writer"));
                for q in &v {
                    s.serialize_quad(q)?;
                }
                s.finish()
            }
        }
    }
}

struct Exporter<'r, R: Reader> {
    r: &'r R,
    /// (statement, graph) pairs whose `rdf:reifies` quad is written.
    reified: HashSet<(Id, Option<Id>)>,
    /// Statements a written quad uses as a reifier, waiting for their
    /// `rdf:reifies` quad.
    pending: Vec<Id>,
}

impl<R: Reader> Exporter<'_, R> {
    fn ox(&self, t: Term) -> OxTerm {
        to_ox(t)
    }

    /// The reifier of a statement: its alias IRI, or a blank node.
    fn reifier(&mut self, id: Id) -> graphlava::Result<NamedOrBlankNode> {
        self.pending.push(id);
        let kv = self.r.kv();
        if let Some(e) =
            graphlava::storage::prefix_iter(kv, &graphlava::tables::ALIAS_REV, &id.be()).next()
        {
            let alias = Id::from_be(&e?.0[8..]);
            if let Term::Iri(i) = self.r.decode(alias)? {
                return Ok(NamedNode::new_unchecked(i).into());
            }
        }
        Ok(BlankNode::new_unchecked(format!("s{:x}", id.raw())).into())
    }

    /// A node in subject position.
    fn subject(&mut self, id: Id) -> graphlava::Result<NamedOrBlankNode> {
        match id.kind() {
            Kind::Statement => self.reifier(id),
            Kind::Hyperedge => Ok(BlankNode::new_unchecked(format!("h{:x}", id.raw())).into()),
            _ => match self.r.decode(id)? {
                Term::Iri(i) => Ok(NamedNode::new_unchecked(i).into()),
                Term::Blank(b) => Ok(BlankNode::new_unchecked(b).into()),
                other => Err(graphlava::Error::Invalid(format!(
                    "{other} cannot be a subject"
                ))),
            },
        }
    }

    /// A node in object position: statements are triple terms.
    fn object(&mut self, id: Id) -> graphlava::Result<OxTerm> {
        match id.kind() {
            Kind::Statement => Ok(OxTerm::Triple(Box::new(self.triple_of(id)?))),
            Kind::Hyperedge => Ok(self.subject(id)?.into()),
            _ => Ok(self.ox(self.r.decode(id)?)),
        }
    }

    fn triple_of(&mut self, id: Id) -> graphlava::Result<OxTriple> {
        let t = self
            .r
            .stmt(id)?
            .ok_or(graphlava::Error::NotFound(id.raw()))?;
        let s = self.subject(t.s)?;
        let p = self.predicate(t.p)?;
        let o = self.object(t.o)?;
        Ok(OxTriple::new(s, p, o))
    }

    fn predicate(&self, id: Id) -> graphlava::Result<NamedNode> {
        match self.r.decode(id)? {
            Term::Iri(i) => Ok(NamedNode::new_unchecked(i)),
            other => Err(graphlava::Error::Invalid(format!(
                "{other} cannot be a predicate"
            ))),
        }
    }

    /// The `rdf:reifies` quads for reifiers used since the last call.
    fn reifies(&mut self, graph: Option<Id>, gname: &GraphName) -> graphlava::Result<Vec<OxQuad>> {
        let mut out = Vec::new();
        while let Some(id) = self.pending.pop() {
            if !self.reified.insert((id, graph)) {
                continue;
            }
            let r = self.reifier_term(id)?;
            let t = self.triple_of(id)?;
            out.push(OxQuad::new(
                r,
                NamedNode::new_unchecked(RDF_REIFIES),
                OxTerm::Triple(Box::new(t)),
                gname.clone(),
            ));
        }
        Ok(out)
    }

    fn reifier_term(&mut self, id: Id) -> graphlava::Result<NamedOrBlankNode> {
        let before = self.pending.len();
        let r = self.reifier(id)?;
        self.pending.truncate(before);
        Ok(r)
    }
}

pub fn to_ox(t: Term) -> OxTerm {
    match t {
        Term::Iri(i) => NamedNode::new_unchecked(i).into(),
        Term::Blank(b) => BlankNode::new_unchecked(b).into(),
        Term::Literal(l) => match l.lang {
            Some(tag) => {
                oxrdf::Literal::new_language_tagged_literal_unchecked(l.lexical, tag).into()
            }
            None => {
                oxrdf::Literal::new_typed_literal(l.lexical, NamedNode::new_unchecked(l.datatype))
                    .into()
            }
        },
        Term::Triple(tt) => {
            let s = match to_ox(tt.subject) {
                OxTerm::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
                OxTerm::BlankNode(b) => NamedOrBlankNode::BlankNode(b),
                // Decoded triple terms never have a literal subject.
                _ => NamedOrBlankNode::BlankNode(BlankNode::default()),
            };
            let p = match tt.predicate {
                Term::Iri(i) => NamedNode::new_unchecked(i),
                other => NamedNode::new_unchecked(other.to_string()),
            };
            OxTerm::Triple(Box::new(OxTriple::new(s, p, to_ox(tt.object))))
        }
        other => NamedNode::new_unchecked(other.to_string()).into(),
    }
}

/// What an export wrote.
#[derive(Debug, Default)]
pub struct ExportReport {
    pub quads: u64,
}

/// Writes the statements of `scope` (all graphs when `None`) as RDF.
pub fn export<W: Write>(
    db: &Database,
    out: W,
    fmt: RdfFormat,
    graph: Option<&str>,
    direct: bool,
) -> CliResult<(W, ExportReport)> {
    let r = db.read()?;
    let scope: Option<HashSet<Id>> = match graph {
        Some(g) => {
            let Some(gid) = r.graph(&Term::iri(g))? else {
                return Err(CliError::Failed(format!("no graph named <{g}>")));
            };
            let mut set: HashSet<Id> = r.scope(gid, direct)?.into_iter().collect();
            set.insert(gid);
            Some(set)
        }
        None => None,
    };
    let in_id = r.lookup(&Term::iri(META_IN))?;
    let mut ex = Exporter {
        r: &r,
        reified: HashSet::new(),
        pending: Vec::new(),
    };
    let mut sink = Sink::new(fmt, out);
    let quads = sink.quads();
    let mut rep = ExportReport::default();
    let ioerr = io_ctx("cannot write output");
    let mut names: HashMap<Id, GraphName> = HashMap::new();
    for m in r.find(None, None, None)? {
        let stmt_subject = matches!(m.s.kind(), Kind::Statement | Kind::Hyperedge);
        if Some(m.p) == in_id && stmt_subject {
            continue; // memberships are the graph of each quad
        }
        let mut graphs = r.graphs_of_match(&m)?;
        if let Some(s) = &scope {
            // A nesting edge lives in the system graph; keep it when its
            // child graph is in scope.
            let mut keep_nesting = false;
            if Some(m.p) == in_id && graphs.contains(&SYSTEM_GRAPH) {
                let child = r.graph(&r.decode(m.s)?)?;
                keep_nesting = child.is_some_and(|c| s.contains(&c));
            }
            graphs.retain(|g| {
                if *g == SYSTEM_GRAPH {
                    keep_nesting
                } else {
                    s.contains(g)
                }
            });
            if graphs.is_empty() {
                continue;
            }
        }
        if !quads {
            graphs.truncate(1); // one triple, whatever its graphs
        }
        ex.pending.clear();
        let s = ex.subject(m.s)?;
        let p = ex.predicate(m.p)?;
        let o = ex.object(m.o)?;
        let uses = std::mem::take(&mut ex.pending);
        for g in graphs {
            let gname = if !quads || g == DEFAULT_GRAPH || g == SYSTEM_GRAPH {
                GraphName::DefaultGraph
            } else {
                match names.get(&g) {
                    Some(n) => n.clone(),
                    None => {
                        let n = match r.decode(g)? {
                            Term::Iri(i) if i == DEFAULT_GRAPH_IRI || i == SYSTEM_GRAPH_IRI => {
                                GraphName::DefaultGraph
                            }
                            Term::Iri(i) => NamedNode::new_unchecked(i).into(),
                            other => {
                                return Err(CliError::Failed(format!(
                                    "graph {other} has no IRI name"
                                )));
                            }
                        };
                        names.insert(g, n.clone());
                        n
                    }
                }
            };
            let key = if quads { Some(g) } else { None };
            ex.pending = uses.clone();
            for q in ex.reifies(key, &gname)? {
                sink.put(q).map_err(io_ctx("cannot write output"))?;
                rep.quads += 1;
            }
            sink.put(OxQuad::new(s.clone(), p.clone(), o.clone(), gname))
                .map_err(io_ctx("cannot write output"))?;
            rep.quads += 1;
        }
    }
    let w = sink.finish().map_err(ioerr)?;
    Ok((w, rep))
}

pub fn export_cmd(ctx: Ctx, a: ExportArgs) -> CliResult<u8> {
    let fmt = a
        .format
        .or_else(|| a.output.as_deref().and_then(format_of))
        .unwrap_or(RdfFormat::Nq);
    let db = open_existing(&a.file)?;
    let t0 = Instant::now();
    let rep = match &a.output {
        Some(p) if p.as_os_str() != "-" => {
            let f = std::fs::File::create(p)
                .map_err(io_ctx(format!("cannot create {}", p.display())))?;
            let (w, rep) = export(
                &db,
                std::io::BufWriter::new(f),
                fmt,
                a.graph.as_deref(),
                a.direct,
            )?;
            w.into_inner()
                .map_err(|e| CliError::Io("cannot write output".into(), e.into_error()))?;
            rep
        }
        _ => {
            let stdout = std::io::stdout();
            let (mut w, rep) = export(
                &db,
                std::io::BufWriter::new(stdout.lock()),
                fmt,
                a.graph.as_deref(),
                a.direct,
            )?;
            w.flush().map_err(io_ctx("cannot write output"))?;
            rep
        }
    };
    ctx.note(format!(
        "exported {} {} ({})",
        rep.quads,
        if matches!(fmt, RdfFormat::Nq | RdfFormat::Trig) {
            "quads"
        } else {
            "triples"
        },
        ms(t0.elapsed())
    ));
    db.close()?;
    Ok(0)
}
