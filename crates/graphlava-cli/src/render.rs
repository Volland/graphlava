//! Rendering terms and query results: aligned tables, SPARQL 1.1 Query
//! Results JSON, CSV and TSV, and N-Triples for graph results.

use std::io::{self, Write};

use graphlava::term::{RDF_LANG_STRING, STMT_IRI_PREFIX, XSD};
use graphlava::{Literal, QueryResult, Term};

use crate::cli::OutFormat;

/// Prefixes always known to the renderer and the shell.
pub const DEFAULT_PREFIXES: &[(&str, &str)] = &[
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("xsd", XSD),
    ("owl", "http://www.w3.org/2002/07/owl#"),
    ("meta", "https://graphlava.org/ns#"),
    ("stmt", STMT_IRI_PREFIX),
];

/// Longest cell in a table before it is cut with an ellipsis.
const MAX_CELL: usize = 80;

/// A prefix map for compact rendering: user prefixes first, then defaults.
#[derive(Debug, Clone, Default)]
pub struct Prefixes {
    pub map: Vec<(String, String)>,
}

impl Prefixes {
    pub fn with_defaults() -> Prefixes {
        Prefixes {
            map: DEFAULT_PREFIXES
                .iter()
                .map(|(p, i)| (p.to_string(), i.to_string()))
                .collect(),
        }
    }

    /// Adds or replaces a prefix; user prefixes win over defaults.
    pub fn set(&mut self, name: &str, iri: &str) {
        self.map.retain(|(p, _)| p != name);
        self.map.insert(0, (name.to_string(), iri.to_string()));
    }

    /// Adds the `PREFIX p: <iri>` (SPARQL) and `@prefix p: <iri> .` (Datalog,
    /// Turtle) declarations found in `text`.
    pub fn add_declared(&mut self, text: &str) {
        for (p, i) in declared_prefixes(text) {
            self.set(&p, &i);
        }
    }

    fn compact(&self, iri: &str) -> Option<String> {
        let mut best: Option<(usize, String)> = None;
        for (p, ns) in &self.map {
            if let Some(local) = iri.strip_prefix(ns.as_str()) {
                if is_local_name(local) && best.as_ref().is_none_or(|(l, _)| ns.len() > *l) {
                    best = Some((ns.len(), format!("{p}:{local}")));
                }
            }
        }
        best.map(|(_, s)| s)
    }
}

/// Finds `prefix name: <iri>` declarations, case-insensitively, with or
/// without a leading `@`.
pub fn declared_prefixes(text: &str) -> Vec<(String, String)> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(k) = lower[from..].find("prefix") {
        let start = from + k;
        from = start + 6;
        let before_ok = start == 0 || {
            let c = text.as_bytes()[start - 1];
            c.is_ascii_whitespace() || c == b'@' || c == b'{' || c == b'.'
        };
        if !before_ok {
            continue;
        }
        let rest = &text[from..];
        let trimmed = rest.trim_start();
        if trimmed.len() == rest.len() {
            continue; // `prefix` must be followed by whitespace
        }
        let Some(colon) = trimmed.find(':') else {
            continue;
        };
        let name = &trimmed[..colon];
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
        {
            continue;
        }
        let after = trimmed[colon + 1..].trim_start();
        let Some(iri) = after.strip_prefix('<') else {
            continue;
        };
        let Some(end) = iri.find('>') else { continue };
        out.push((name.to_string(), iri[..end].to_string()));
    }
    out
}

fn is_local_name(s: &str) -> bool {
    !s.is_empty()
        && !s.ends_with('.')
        && !s.starts_with('-')
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
}

// ---- term syntaxes ----------------------------------------------------------

fn escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
}

fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    escape_into(&mut out, s);
    out.push('"');
    out
}

/// A term in N-Triples syntax (RDF 1.2: triple terms as `<<( s p o )>>`).
pub fn nt(t: &Term) -> String {
    match t {
        Term::Iri(i) => format!("<{i}>"),
        Term::Blank(b) => format!("_:{b}"),
        Term::Literal(l) => nt_literal(l),
        Term::Triple(tt) => format!(
            "<<( {} {} {} )>>",
            nt(&tt.subject),
            nt(&tt.predicate),
            nt(&tt.object)
        ),
        other => format!("{other}"),
    }
}

fn nt_literal(l: &Literal) -> String {
    let q = quoted(&l.lexical);
    match &l.lang {
        Some(tag) => format!("{q}@{tag}"),
        None if l.datatype == format!("{XSD}string") => q,
        None => format!("{q}^^<{}>", l.datatype),
    }
}

/// A term for a terminal: prefixed names, bare numbers and booleans,
/// statement reifiers as `stmt:<id>`.
pub fn compact(t: &Term, px: &Prefixes) -> String {
    match t {
        Term::Iri(i) => px.compact(i).unwrap_or_else(|| format!("<{i}>")),
        Term::Blank(b) => format!("_:{b}"),
        Term::Literal(l) => {
            if let Some(tag) = &l.lang {
                return format!("{}@{tag}", quoted(&l.lexical));
            }
            let local = l.datatype.strip_prefix(XSD).unwrap_or("");
            match local {
                "string" => quoted(&l.lexical),
                "integer" | "decimal" | "double" | "boolean" => l.lexical.clone(),
                _ if l.datatype == RDF_LANG_STRING => quoted(&l.lexical),
                _ => format!(
                    "{}^^{}",
                    quoted(&l.lexical),
                    px.compact(&l.datatype)
                        .unwrap_or_else(|| format!("<{}>", l.datatype))
                ),
            }
        }
        Term::Triple(tt) => format!(
            "<<( {} {} {} )>>",
            compact(&tt.subject, px),
            compact(&tt.predicate, px),
            compact(&tt.object, px)
        ),
        other => format!("{other}"),
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A term as a SPARQL 1.2 Query Results JSON object.
pub fn json_term(t: &Term) -> String {
    match t {
        Term::Iri(i) => format!("{{\"type\":\"uri\",\"value\":{}}}", json_str(i)),
        Term::Blank(b) => format!("{{\"type\":\"bnode\",\"value\":{}}}", json_str(b)),
        Term::Literal(l) => {
            let v = json_str(&l.lexical);
            match &l.lang {
                Some(tag) => format!(
                    "{{\"type\":\"literal\",\"value\":{v},\"xml:lang\":{}}}",
                    json_str(tag)
                ),
                None if l.datatype == format!("{XSD}string") => {
                    format!("{{\"type\":\"literal\",\"value\":{v}}}")
                }
                None => format!(
                    "{{\"type\":\"literal\",\"value\":{v},\"datatype\":{}}}",
                    json_str(&l.datatype)
                ),
            }
        }
        Term::Triple(tt) => format!(
            "{{\"type\":\"triple\",\"value\":{{\"subject\":{},\"predicate\":{},\"object\":{}}}}}",
            json_term(&tt.subject),
            json_term(&tt.predicate),
            json_term(&tt.object)
        ),
        other => json_str(&other.to_string()),
    }
}

fn csv_term(t: &Term) -> String {
    match t {
        Term::Iri(i) => i.clone(),
        Term::Blank(b) => format!("_:{b}"),
        Term::Literal(l) => l.lexical.clone(),
        other => nt(other),
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ---- results ----------------------------------------------------------------

/// What was printed, for the summary line.
#[derive(Debug, Clone, Copy)]
pub enum Printed {
    Rows(usize),
    Boolean(bool),
    Triples(usize),
}

impl Printed {
    pub fn summary(self) -> String {
        match self {
            Printed::Rows(1) => "1 row".into(),
            Printed::Rows(n) => format!("{n} rows"),
            Printed::Boolean(b) => format!("{b}"),
            Printed::Triples(1) => "1 triple".into(),
            Printed::Triples(n) => format!("{n} triples"),
        }
    }
}

/// Writes a result in the chosen format.
pub fn write_result(
    out: &mut dyn Write,
    res: &QueryResult,
    format: OutFormat,
    px: &Prefixes,
) -> io::Result<Printed> {
    match res {
        QueryResult::Solutions { vars, rows } => {
            match format {
                OutFormat::Table => write_table(out, vars, rows, px)?,
                OutFormat::Json => write_json(out, vars, rows)?,
                OutFormat::Csv => {
                    writeln!(out, "{}\r", vars.join(","))?;
                    for r in rows {
                        let cells: Vec<String> = r
                            .iter()
                            .map(|c| {
                                c.as_ref()
                                    .map(|t| csv_field(&csv_term(t)))
                                    .unwrap_or_default()
                            })
                            .collect();
                        writeln!(out, "{}\r", cells.join(","))?;
                    }
                }
                OutFormat::Tsv => {
                    let head: Vec<String> = vars.iter().map(|v| format!("?{v}")).collect();
                    writeln!(out, "{}", head.join("\t"))?;
                    for r in rows {
                        let cells: Vec<String> = r
                            .iter()
                            .map(|c| c.as_ref().map(nt).unwrap_or_default())
                            .collect();
                        writeln!(out, "{}", cells.join("\t"))?;
                    }
                }
            }
            Ok(Printed::Rows(rows.len()))
        }
        QueryResult::Boolean(b) => {
            match format {
                OutFormat::Json => writeln!(out, "{{\"head\":{{}},\"boolean\":{b}}}")?,
                _ => writeln!(out, "{b}")?,
            }
            Ok(Printed::Boolean(*b))
        }
        QueryResult::Graph(triples) => {
            for [s, p, o] in triples {
                writeln!(out, "{} {} {} .", nt(s), nt(p), nt(o))?;
            }
            Ok(Printed::Triples(triples.len()))
        }
    }
}

fn write_json(out: &mut dyn Write, vars: &[String], rows: &[Vec<Option<Term>>]) -> io::Result<()> {
    let head: Vec<String> = vars.iter().map(|v| json_str(v)).collect();
    write!(
        out,
        "{{\"head\":{{\"vars\":[{}]}},\"results\":{{\"bindings\":[",
        head.join(",")
    )?;
    for (k, r) in rows.iter().enumerate() {
        if k > 0 {
            write!(out, ",")?;
        }
        let cells: Vec<String> = vars
            .iter()
            .zip(r)
            .filter_map(|(v, c)| {
                c.as_ref()
                    .map(|t| format!("{}:{}", json_str(v), json_term(t)))
            })
            .collect();
        write!(out, "\n{{{}}}", cells.join(","))?;
    }
    writeln!(out, "\n]}}}}")
}

fn width(s: &str) -> usize {
    s.chars().count()
}

fn cut(s: String) -> String {
    if width(&s) <= MAX_CELL {
        return s;
    }
    let mut t: String = s.chars().take(MAX_CELL - 1).collect();
    t.push('…');
    t
}

fn write_table(
    out: &mut dyn Write,
    vars: &[String],
    rows: &[Vec<Option<Term>>],
    px: &Prefixes,
) -> io::Result<()> {
    let head: Vec<String> = vars.iter().map(|v| format!("?{v}")).collect();
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|c| c.as_ref().map(|t| cut(compact(t, px))).unwrap_or_default())
                .collect()
        })
        .collect();
    let mut w: Vec<usize> = head.iter().map(|h| width(h)).collect();
    for r in &cells {
        for (k, c) in r.iter().enumerate() {
            w[k] = w[k].max(width(c));
        }
    }
    let line = |out: &mut dyn Write, r: &[String]| -> io::Result<()> {
        let mut s = String::new();
        for (k, c) in r.iter().enumerate() {
            if k > 0 {
                s.push_str("  ");
            }
            s.push_str(c);
            if k + 1 < r.len() {
                s.push_str(&" ".repeat(w[k] - width(c)));
            }
        }
        writeln!(out, "{}", s.trim_end())
    };
    line(out, &head)?;
    let rule: Vec<String> = w.iter().map(|n| "-".repeat(*n)).collect();
    line(out, &rule)?;
    for r in &cells {
        line(out, r)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_prefixes() {
        let p = declared_prefixes(
            "PREFIX : <http://ex.org/>\nprefix foaf:   <http://xmlns.com/foaf/0.1/>\n@prefix d: <http://d/> .",
        );
        assert_eq!(
            p,
            vec![
                ("".into(), "http://ex.org/".into()),
                ("foaf".into(), "http://xmlns.com/foaf/0.1/".into()),
                ("d".into(), "http://d/".into())
            ]
        );
    }

    #[test]
    fn compacts_terms() {
        let mut px = Prefixes::with_defaults();
        px.set("", "http://ex.org/");
        assert_eq!(compact(&Term::iri("http://ex.org/alice"), &px), ":alice");
        assert_eq!(
            compact(&Term::iri("http://other/x y"), &px),
            "<http://other/x y>"
        );
        assert_eq!(compact(&Term::integer(4), &px), "4");
        assert_eq!(compact(&Term::string("a\"b"), &px), "\"a\\\"b\"");
        assert_eq!(
            compact(&Term::iri(format!("{STMT_IRI_PREFIX}42")), &px),
            "stmt:42"
        );
        let tt = Term::triple(
            Term::iri("http://ex.org/a"),
            Term::iri("http://ex.org/b"),
            Term::integer(1),
        );
        assert_eq!(compact(&tt, &px), "<<( :a :b 1 )>>");
        assert_eq!(
            nt(&tt),
            "<<( <http://ex.org/a> <http://ex.org/b> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> )>>"
        );
    }
}
