//! Terms as callers write and read them ([[01-data-model#Terms]]).

use std::fmt;

use crate::id::Id;

pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
pub const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
/// The system namespace ([[adr:0031]]).
pub const META_NS: &str = "https://graphlava.org/ns#";
/// The membership and nesting predicate `meta:in` ([[09-graphs]]).
pub const META_IN: &str = "https://graphlava.org/ns#in";
/// Names of the two fixed graphs ([[adr:0038]]).
pub const DEFAULT_GRAPH_IRI: &str = "https://graphlava.org/ns#defaultGraph";
pub const SYSTEM_GRAPH_IRI: &str = "https://graphlava.org/ns#systemGraph";
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
/// Prefix of statement reifier IRIs ([[adr:0031]]).
pub const STMT_IRI_PREFIX: &str = "https://graphlava.org/ns/stmt/";

/// `xsd:<local>` as a full IRI.
pub fn xsd(local: &str) -> String {
    format!("{XSD}{local}")
}

/// A literal: lexical form, datatype IRI and optional language tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Literal {
    pub lexical: String,
    pub datatype: String,
    pub lang: Option<String>,
}

impl Literal {
    /// A plain string (`xsd:string`).
    pub fn string(s: impl Into<String>) -> Literal {
        Literal {
            lexical: s.into(),
            datatype: xsd("string"),
            lang: None,
        }
    }
    /// A typed literal.
    pub fn typed(lexical: impl Into<String>, datatype: impl Into<String>) -> Literal {
        Literal {
            lexical: lexical.into(),
            datatype: datatype.into(),
            lang: None,
        }
    }
    /// A language-tagged string.
    pub fn lang(s: impl Into<String>, tag: impl Into<String>) -> Literal {
        Literal {
            lexical: s.into(),
            datatype: RDF_LANG_STRING.to_string(),
            lang: Some(tag.into()),
        }
    }
    pub fn integer(v: i64) -> Literal {
        Literal::typed(v.to_string(), xsd("integer"))
    }
    pub fn double(v: f64) -> Literal {
        Literal::typed(crate::literal::double_lexical(v), xsd("double"))
    }
    pub fn boolean(v: bool) -> Literal {
        Literal::typed(v.to_string(), xsd("boolean"))
    }
    pub fn decimal(lexical: impl Into<String>) -> Literal {
        Literal::typed(lexical, xsd("decimal"))
    }
    pub fn date_time(lexical: impl Into<String>) -> Literal {
        Literal::typed(lexical, xsd("dateTime"))
    }
}

/// A term in any statement position. Non-exhaustive: further RDF term kinds
/// may arrive as variants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Term {
    Iri(String),
    /// A blank node. Labels written by a caller are scoped to one request
    /// (rule `dm-blank-scope`); decoded blank nodes carry `b<id>` labels.
    Blank(String),
    Literal(Literal),
    /// An RDF 1.2 triple term `<<( s p o )>>`. A stored statement is its
    /// reifier; queries build this value only where SPARQL needs a triple
    /// term (rule `sparql-triple-term-lazy`,
    /// [[22-implementation-decisions#Triple terms map to statement IDs]]).
    Triple(Box<TripleTerm>),
}

/// The three components of a triple term.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TripleTerm {
    pub subject: Term,
    pub predicate: Term,
    pub object: Term,
}

impl Term {
    pub fn iri(s: impl Into<String>) -> Term {
        Term::Iri(s.into())
    }
    pub fn lit(l: Literal) -> Term {
        Term::Literal(l)
    }
    pub fn string(s: impl Into<String>) -> Term {
        Term::Literal(Literal::string(s))
    }
    pub fn integer(v: i64) -> Term {
        Term::Literal(Literal::integer(v))
    }
    pub fn double(v: f64) -> Term {
        Term::Literal(Literal::double(v))
    }
    /// A triple term `<<( s p o )>>`.
    pub fn triple(subject: Term, predicate: Term, object: Term) -> Term {
        Term::Triple(Box::new(TripleTerm {
            subject,
            predicate,
            object,
        }))
    }
    pub fn as_triple(&self) -> Option<&TripleTerm> {
        match self {
            Term::Triple(t) => Some(t),
            _ => None,
        }
    }
    pub fn as_literal(&self) -> Option<&Literal> {
        match self {
            Term::Literal(l) => Some(l),
            _ => None,
        }
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Iri(i) => write!(f, "<{i}>"),
            Term::Blank(b) => write!(f, "_:{b}"),
            Term::Literal(l) => match &l.lang {
                Some(t) => write!(f, "{:?}@{t}", l.lexical),
                None => write!(f, "{:?}^^<{}>", l.lexical, l.datatype),
            },
            Term::Triple(t) => write!(f, "<<( {} {} {} )>>", t.subject, t.predicate, t.object),
        }
    }
}

/// A term given to a write: an existing ID, a term to intern, or a
/// request-scoped blank-node label.
#[derive(Debug, Clone, PartialEq)]
pub enum TermRef {
    Id(Id),
    Term(Term),
}

impl From<Id> for TermRef {
    fn from(id: Id) -> Self {
        TermRef::Id(id)
    }
}

impl From<Term> for TermRef {
    fn from(t: Term) -> Self {
        TermRef::Term(t)
    }
}

impl From<&Term> for TermRef {
    fn from(t: &Term) -> Self {
        TermRef::Term(t.clone())
    }
}

impl From<Literal> for TermRef {
    fn from(l: Literal) -> Self {
        TermRef::Term(Term::Literal(l))
    }
}

/// A statement as read back: subject, predicate and object IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Triple {
    pub s: Id,
    pub p: Id,
    pub o: Id,
}
