//! Datalog parser ([[13-datalog#Surface syntax]], [[adr:0040]]).
//!
//! A hand-written tokenizer plus recursive-descent parser for the
//! Nemo-compatible surface. Prefixed names, bare constants and relative IRIs
//! are resolved here, so the AST only holds absolute IRIs. Every Nemo
//! construct outside the supported subset fails with an error that names it
//! (rule `dl-nemo-compatible`).

// @tg: implements:: [[openspec:datalog-query#Nemo-compatible syntax]]

use std::collections::HashMap;

use super::ast::{
    AggFunc, Arg, Atom, BinOp, BodyItem, CmpOp, Expr, Goal, Program, Rule, RuleMode, TermAst,
};
use crate::term::{Literal, XSD, xsd};

/// A parse error naming the line and the construct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parses a program. `base_prefixes` are prefixes known before the text
/// (for example from the API); the text's `@prefix` directives extend them.
pub fn parse(text: &str, base_prefixes: &[(String, String)]) -> Result<Program, ParseError> {
    // A lexing error is reported only if the parser reaches it, so an
    // unsupported directive is named before a stray character after it.
    let (toks, lex_err) = lex(text);
    let last_line = text.lines().count().max(1);
    let mut prefixes: HashMap<String, String> = HashMap::new();
    prefixes.insert("xsd".into(), XSD.into());
    for (p, iri) in base_prefixes {
        prefixes.insert(p.clone(), iri.clone());
    }
    let mut p = Parser {
        toks,
        pos: 0,
        last_line,
        prefixes,
        base: None,
        hit_end: std::cell::Cell::new(false),
    };
    let res = p.program();
    match (res, lex_err) {
        (Ok(_), Some(e)) => Err(e),
        (Err(_), Some(e)) if p.hit_end.get() => Err(e),
        (res, _) => res,
    }
}

fn err<T>(line: usize, message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError {
        line,
        message: message.into(),
    })
}

// ---------------------------------------------------------------- tokens

#[derive(Debug, Clone, PartialEq)]
enum NumKind {
    Integer,
    Decimal,
    Double,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// `<...>`, raw (unresolved) content.
    Iri(String),
    /// `prefix:local`; `:local` has an empty prefix.
    PName(String, String),
    Ident(String),
    /// `?name` without the `?`; `?_` gives `_`.
    Var(String),
    /// A string literal and its optional `@lang` tag.
    Str(String, Option<String>),
    Num(NumKind, String),
    /// `@name`
    Directive(String),
    /// `#name`
    Hash(String),
    LParen,
    RParen,
    Comma,
    Dot,
    ColonDash,
    GoalStart,
    Tilde,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Caret2,
    /// `|` or `;`: disjunction, never supported.
    Disj(char),
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Iri(s) => format!("`<{s}>`"),
        Tok::PName(p, l) => format!("`{p}:{l}`"),
        Tok::Ident(s) => format!("`{s}`"),
        Tok::Var(s) => format!("`?{s}`"),
        Tok::Str(s, _) => format!("string \"{s}\""),
        Tok::Num(_, s) => format!("number `{s}`"),
        Tok::Directive(s) => format!("`@{s}`"),
        Tok::Hash(s) => format!("`#{s}`"),
        Tok::LParen => "`(`".into(),
        Tok::RParen => "`)`".into(),
        Tok::Comma => "`,`".into(),
        Tok::Dot => "`.`".into(),
        Tok::ColonDash => "`:-`".into(),
        Tok::GoalStart => "`?-`".into(),
        Tok::Tilde => "`~`".into(),
        Tok::Eq => "`=`".into(),
        Tok::Ne => "`!=`".into(),
        Tok::Lt => "`<`".into(),
        Tok::Le => "`<=`".into(),
        Tok::Gt => "`>`".into(),
        Tok::Ge => "`>=`".into(),
        Tok::Plus => "`+`".into(),
        Tok::Minus => "`-`".into(),
        Tok::Star => "`*`".into(),
        Tok::Slash => "`/`".into(),
        Tok::Caret2 => "`^^`".into(),
        Tok::Disj(c) => format!("`{c}`"),
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Characters that end an `<...>` scan; if one is met before `>`, the `<` is
/// the less-than operator instead of an IRI.
fn ends_iri(c: char) -> bool {
    c.is_whitespace() || matches!(c, '<' | '"' | '{' | '}' | '|' | '^' | '`' | '\\')
}

/// Tokenizes `text`, returning the tokens before the first lexing error and
/// that error, if any.
fn lex(text: &str) -> (Vec<(Tok, usize)>, Option<ParseError>) {
    let mut out = Vec::new();
    match lex_into(text, &mut out) {
        Ok(()) => (out, None),
        Err(e) => (out, Some(e)),
    }
}

fn lex_into(text: &str, out: &mut Vec<(Tok, usize)>) -> Result<(), ParseError> {
    let cs: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut line = 1;
    let at = |i: usize| cs.get(i).copied();
    while i < cs.len() {
        let c = cs[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '%' || (c == '/' && at(i + 1) == Some('/')) {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
            continue;
        }
        let tok_line = line;
        let push = |t: Tok, out: &mut Vec<(Tok, usize)>| out.push((t, tok_line));
        match c {
            '<' => {
                if at(i + 1) == Some('=') {
                    push(Tok::Le, out);
                    i += 2;
                    continue;
                }
                let mut j = i + 1;
                while j < cs.len() && cs[j] != '>' && !ends_iri(cs[j]) {
                    j += 1;
                }
                if j < cs.len() && cs[j] == '>' && j > i + 1 {
                    push(Tok::Iri(cs[i + 1..j].iter().collect()), out);
                    i = j + 1;
                } else {
                    push(Tok::Lt, out);
                    i += 1;
                }
            }
            '>' => {
                if at(i + 1) == Some('=') {
                    push(Tok::Ge, out);
                    i += 2;
                } else {
                    push(Tok::Gt, out);
                    i += 1;
                }
            }
            '!' => {
                if at(i + 1) == Some('=') {
                    push(Tok::Ne, out);
                    i += 2;
                } else if at(i + 1).is_some_and(is_ident_start) {
                    let mut j = i + 1;
                    while j < cs.len() && is_ident_char(cs[j]) {
                        j += 1;
                    }
                    let name: String = cs[i + 1..j].iter().collect();
                    return err(
                        line,
                        format!("existential variables (`!{name}`) are not supported"),
                    );
                } else {
                    return err(line, "unexpected `!`");
                }
            }
            '?' => {
                if at(i + 1) == Some('-') {
                    push(Tok::GoalStart, out);
                    i += 2;
                } else if at(i + 1).is_some_and(is_ident_start) {
                    let mut j = i + 1;
                    while j < cs.len() && is_ident_char(cs[j]) {
                        j += 1;
                    }
                    push(Tok::Var(cs[i + 1..j].iter().collect()), out);
                    i = j;
                } else {
                    return err(line, "`?` must start a variable name or a `?-` goal");
                }
            }
            '"' => {
                let mut s = String::new();
                let mut j = i + 1;
                loop {
                    match at(j) {
                        None => return err(tok_line, "unterminated string literal"),
                        Some('"') => break,
                        Some('\\') => {
                            let e = match at(j + 1) {
                                Some('"') => '"',
                                Some('\\') => '\\',
                                Some('n') => '\n',
                                Some('t') => '\t',
                                Some('r') => '\r',
                                Some('\'') => '\'',
                                Some(o) => {
                                    return err(line, format!("unknown string escape `\\{o}`"));
                                }
                                None => return err(tok_line, "unterminated string literal"),
                            };
                            s.push(e);
                            j += 2;
                        }
                        Some(ch) => {
                            if ch == '\n' {
                                line += 1;
                            }
                            s.push(ch);
                            j += 1;
                        }
                    }
                }
                j += 1;
                let mut lang = None;
                if at(j) == Some('@') && at(j + 1).is_some_and(|c| c.is_ascii_alphabetic()) {
                    let mut k = j + 1;
                    while k < cs.len() && (cs[k].is_ascii_alphanumeric() || cs[k] == '-') {
                        k += 1;
                    }
                    lang = Some(cs[j + 1..k].iter().collect());
                    j = k;
                }
                push(Tok::Str(s, lang), out);
                i = j;
            }
            '@' => {
                let mut j = i + 1;
                while j < cs.len() && is_ident_char(cs[j]) {
                    j += 1;
                }
                if j == i + 1 {
                    return err(line, "`@` must start a directive or annotation name");
                }
                push(Tok::Directive(cs[i + 1..j].iter().collect()), out);
                i = j;
            }
            '#' => {
                let mut j = i + 1;
                while j < cs.len() && is_ident_char(cs[j]) {
                    j += 1;
                }
                push(Tok::Hash(cs[i + 1..j].iter().collect()), out);
                i = j;
            }
            '0'..='9' => {
                let mut j = i;
                while j < cs.len() && cs[j].is_ascii_digit() {
                    j += 1;
                }
                let mut kind = NumKind::Integer;
                if at(j) == Some('.') && at(j + 1).is_some_and(|c| c.is_ascii_digit()) {
                    kind = NumKind::Decimal;
                    j += 1;
                    while j < cs.len() && cs[j].is_ascii_digit() {
                        j += 1;
                    }
                }
                if matches!(at(j), Some('e' | 'E')) {
                    let mut k = j + 1;
                    if matches!(at(k), Some('+' | '-')) {
                        k += 1;
                    }
                    if at(k).is_some_and(|c| c.is_ascii_digit()) {
                        while k < cs.len() && cs[k].is_ascii_digit() {
                            k += 1;
                        }
                        kind = NumKind::Double;
                        j = k;
                    }
                }
                push(Tok::Num(kind, cs[i..j].iter().collect()), out);
                i = j;
            }
            ':' => {
                if at(i + 1) == Some('-') {
                    push(Tok::ColonDash, out);
                    i += 2;
                } else {
                    let (local, j) = lex_local(&cs, i + 1);
                    push(Tok::PName(String::new(), local), out);
                    i = j;
                }
            }
            c if is_ident_start(c) => {
                let mut j = i;
                while j < cs.len() && is_ident_char(cs[j]) {
                    j += 1;
                }
                let name: String = cs[i..j].iter().collect();
                if at(j) == Some(':') && at(j + 1) != Some('-') {
                    let (local, k) = lex_local(&cs, j + 1);
                    push(Tok::PName(name, local), out);
                    i = k;
                } else {
                    push(Tok::Ident(name), out);
                    i = j;
                }
            }
            '(' => {
                push(Tok::LParen, out);
                i += 1;
            }
            ')' => {
                push(Tok::RParen, out);
                i += 1;
            }
            ',' => {
                push(Tok::Comma, out);
                i += 1;
            }
            '.' => {
                push(Tok::Dot, out);
                i += 1;
            }
            '~' => {
                push(Tok::Tilde, out);
                i += 1;
            }
            '=' => {
                if at(i + 1) == Some('=') {
                    return err(line, "use `=` for equality, not `==`");
                }
                push(Tok::Eq, out);
                i += 1;
            }
            '+' => {
                push(Tok::Plus, out);
                i += 1;
            }
            '-' => {
                push(Tok::Minus, out);
                i += 1;
            }
            '*' => {
                push(Tok::Star, out);
                i += 1;
            }
            '/' => {
                push(Tok::Slash, out);
                i += 1;
            }
            '^' if at(i + 1) == Some('^') => {
                push(Tok::Caret2, out);
                i += 2;
            }
            '|' | ';' => {
                push(Tok::Disj(c), out);
                i += 1;
            }
            '{' | '}' => {
                return err(line, "maps (`{...}`) are not supported");
            }
            '[' | ']' => {
                return err(line, "lists (`[...]`) are not supported");
            }
            '\'' => {
                return err(line, "single-quoted strings are not supported; use \"...\"");
            }
            other => return err(line, format!("unexpected character `{other}`")),
        }
    }
    Ok(())
}

/// Lexes the local part of a prefixed name starting at `i`. A `.` belongs to
/// the name only when another name character follows, so `:a.` ends with `.`.
fn lex_local(cs: &[char], mut i: usize) -> (String, usize) {
    let mut s = String::new();
    while i < cs.len() {
        let c = cs[i];
        let name_char = is_local_char(c) && !(s.is_empty() && c == '-');
        let inner_dot =
            c == '.' && !s.is_empty() && cs.get(i + 1).is_some_and(|&n| is_local_char(n));
        if name_char || inner_dot {
            s.push(c);
            i += 1;
        } else {
            break;
        }
    }
    (s, i)
}

// ---------------------------------------------------------------- parser

struct Parser {
    toks: Vec<(Tok, usize)>,
    pos: usize,
    last_line: usize,
    prefixes: HashMap<String, String>,
    base: Option<String>,
    /// Set when the parser looked past the last token.
    hit_end: std::cell::Cell<bool>,
}

#[derive(Default)]
struct Pending {
    assert: bool,
    mode: Option<RuleMode>,
    line: usize,
    name: Option<String>,
}

fn is_absolute_iri(s: &str) -> bool {
    let mut it = s.char_indices();
    match it.next() {
        Some((_, c)) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for (_, c) in it {
        if c == ':' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')) {
            return false;
        }
    }
    false
}

/// Built-in function names are ALL-CAPS (`STRLEN`, `CONCAT`, `ABS`).
fn is_builtin_name(n: &str) -> bool {
    n.chars().any(|c| c.is_ascii_uppercase()) && !n.chars().any(|c| c.is_ascii_lowercase())
}

fn is_var_name(n: &str) -> bool {
    n.starts_with(|c: char| c.is_ascii_uppercase() || c == '_')
}

impl Parser {
    fn get(&self, k: usize) -> Option<&(Tok, usize)> {
        let t = self.toks.get(self.pos + k);
        if t.is_none() {
            self.hit_end.set(true);
        }
        t
    }

    fn peek(&self) -> Option<&Tok> {
        self.get(0).map(|(t, _)| t)
    }

    fn peek_at(&self, k: usize) -> Option<&Tok> {
        self.get(k).map(|(t, _)| t)
    }

    fn line(&self) -> usize {
        self.toks
            .get(self.pos)
            .map(|(_, l)| *l)
            .unwrap_or(self.last_line)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.get(0).map(|(t, _)| t.clone());
        self.pos += 1;
        t
    }

    fn found(&self) -> String {
        self.peek()
            .map(describe)
            .unwrap_or_else(|| "end of input".into())
    }

    fn expect(&mut self, want: Tok, what: &str) -> Result<(), ParseError> {
        if self.peek() == Some(&want) {
            self.pos += 1;
            Ok(())
        } else {
            err(
                self.line(),
                format!("expected {what}, found {}", self.found()),
            )
        }
    }

    fn program(&mut self) -> Result<Program, ParseError> {
        let mut prog = Program::default();
        let mut pending = Pending::default();
        while let Some(tok) = self.peek().cloned() {
            let line = self.line();
            match tok {
                Tok::Directive(name) => {
                    self.pos += 1;
                    match name.as_str() {
                        "assert" => {
                            pending.assert = true;
                            pending.line = line;
                            pending.name.get_or_insert(name);
                        }
                        "materialized" | "on_demand" => {
                            let mode = if name == "on_demand" {
                                RuleMode::OnDemand
                            } else if self.peek() == Some(&Tok::LParen) {
                                self.pos += 1;
                                match self.next() {
                                    Some(Tok::Ident(a)) if a == "force" => {}
                                    _ => {
                                        return err(
                                            line,
                                            "`@materialized(...)` accepts only `force`",
                                        );
                                    }
                                }
                                self.expect(Tok::RParen, "`)`")?;
                                RuleMode::Materialized { force: true }
                            } else {
                                RuleMode::Materialized { force: false }
                            };
                            if pending.mode.is_some_and(|m| m != mode) {
                                return err(line, "conflicting rule annotations");
                            }
                            pending.mode = Some(mode);
                            pending.line = line;
                            pending.name.get_or_insert(name);
                        }
                        _ => {
                            if let Some(a) = &pending.name {
                                return err(
                                    pending.line,
                                    format!("annotation `@{a}` must precede a rule or fact"),
                                );
                            }
                            self.directive(&name, line, &mut prog)?;
                        }
                    }
                }
                Tok::GoalStart => {
                    if let Some(a) = &pending.name {
                        return err(
                            pending.line,
                            format!("annotation `@{a}` cannot apply to a `?-` goal"),
                        );
                    }
                    self.pos += 1;
                    let body = self.body()?;
                    prog.goals.push(Goal { body, line });
                }
                _ => {
                    let rule = self.rule(std::mem::take(&mut pending))?;
                    prog.rules.push(rule);
                }
            }
        }
        if let Some(a) = &pending.name {
            return err(
                pending.line,
                format!("annotation `@{a}` must precede a rule or fact"),
            );
        }
        Ok(prog)
    }

    fn directive(&mut self, name: &str, line: usize, prog: &mut Program) -> Result<(), ParseError> {
        match name {
            "prefix" => {
                let p = match self.next() {
                    Some(Tok::PName(p, l)) if l.is_empty() => p,
                    _ => return err(line, "expected `name:` after `@prefix`"),
                };
                let iri = match self.next() {
                    Some(Tok::Iri(raw)) => self.resolve_iri(&raw, line)?,
                    _ => return err(line, "expected `<iri>` in `@prefix`"),
                };
                self.expect(Tok::Dot, "`.` after `@prefix`")?;
                self.prefixes.insert(p, iri);
            }
            "base" => {
                let iri = match self.next() {
                    Some(Tok::Iri(raw)) => self.resolve_iri(&raw, line)?,
                    _ => return err(line, "expected `<iri>` after `@base`"),
                };
                self.expect(Tok::Dot, "`.` after `@base`")?;
                self.base = Some(iri);
            }
            "output" => {
                let name = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    _ => return err(line, "expected a predicate name after `@output`"),
                };
                self.expect(Tok::Dot, "`.` after `@output`")?;
                prog.outputs.push(name);
            }
            "import" | "export" => {
                return err(
                    line,
                    format!(
                        "`@{name}` is not supported: programs read and write the store directly"
                    ),
                );
            }
            "declare" | "parameter" | "source" => {
                return err(line, format!("`@{name}` is not supported"));
            }
            other => return err(line, format!("unknown directive `@{other}`")),
        }
        Ok(())
    }

    fn rule(&mut self, pending: Pending) -> Result<Rule, ParseError> {
        let line = self.line();
        if self.peek() == Some(&Tok::Tilde)
            || (self.peek() == Some(&Tok::Ident("not".into()))
                && self.peek_at(1) != Some(&Tok::LParen))
        {
            return err(line, "negated heads are not supported");
        }
        if !self.at_atom_start(true) {
            return err(
                line,
                format!("expected a rule, fact or directive, found {}", self.found()),
            );
        }
        let head = self.atom(true)?;
        let body = match self.peek() {
            Some(Tok::Dot) => {
                self.pos += 1;
                for a in &head.args {
                    match a {
                        Arg::Term(TermAst::Var(v)) => {
                            return err(
                                line,
                                format!("variable `{v}` in a fact; facts must be ground"),
                            );
                        }
                        Arg::Term(TermAst::Anon) => {
                            return err(
                                line,
                                "anonymous variable `_` in a fact; facts must be ground",
                            );
                        }
                        Arg::Aggregate { .. } => {
                            return err(line, "aggregates are not allowed in facts");
                        }
                        Arg::Term(_) => {}
                    }
                }
                Vec::new()
            }
            Some(Tok::ColonDash) => {
                self.pos += 1;
                self.body()?
            }
            Some(Tok::Comma) => {
                return err(self.line(), "multiple head atoms are not supported");
            }
            Some(Tok::Disj(_)) => {
                return err(self.line(), "disjunctive heads are not supported");
            }
            _ => {
                return err(
                    self.line(),
                    format!(
                        "expected `:-` or `.` after the head, found {}",
                        self.found()
                    ),
                );
            }
        };
        Ok(Rule {
            head,
            body,
            assert: pending.assert,
            mode: pending.mode,
            line,
        })
    }

    /// Body items up to and including the closing `.`.
    fn body(&mut self) -> Result<Vec<BodyItem>, ParseError> {
        let mut items = Vec::new();
        loop {
            if self.peek() == Some(&Tok::Dot) {
                return err(self.line(), "empty body item");
            }
            items.push(self.body_item()?);
            match self.peek() {
                Some(Tok::Comma) => self.pos += 1,
                Some(Tok::Dot) => {
                    self.pos += 1;
                    return Ok(items);
                }
                Some(Tok::Disj(_)) => {
                    return err(self.line(), "disjunction in rule bodies is not supported");
                }
                _ => {
                    return err(
                        self.line(),
                        format!("expected `,` or `.` in body, found {}", self.found()),
                    );
                }
            }
        }
    }

    /// True when the next tokens are `name(`, `p:l(` or `<iri>(`. In a body,
    /// ALL-CAPS names followed by `(` are built-in calls, not atoms.
    fn at_atom_start(&self, head: bool) -> bool {
        let open = self.peek_at(1) == Some(&Tok::LParen);
        match self.peek() {
            Some(Tok::Ident(n)) => open && (head || !is_builtin_name(n)),
            Some(Tok::PName(..)) | Some(Tok::Iri(_)) => open,
            _ => false,
        }
    }

    fn body_item(&mut self) -> Result<BodyItem, ParseError> {
        if self.peek() == Some(&Tok::Tilde) {
            self.pos += 1;
            return Ok(BodyItem::Neg(self.neg_atom()?));
        }
        if self.peek() == Some(&Tok::Ident("not".into())) && self.peek_at(1) != Some(&Tok::LParen) {
            self.pos += 1;
            return Ok(BodyItem::Neg(self.neg_atom()?));
        }
        if self.at_atom_start(false) {
            return Ok(BodyItem::Pos(self.atom(false)?));
        }
        let line = self.line();
        let lhs = self.expr()?;
        let op = match self.peek() {
            Some(Tok::Eq) => CmpOp::Eq,
            Some(Tok::Ne) => CmpOp::Ne,
            Some(Tok::Lt) => CmpOp::Lt,
            Some(Tok::Le) => CmpOp::Le,
            Some(Tok::Gt) => CmpOp::Gt,
            Some(Tok::Ge) => CmpOp::Ge,
            _ => {
                return err(
                    line,
                    format!(
                        "expected an atom or a comparison in body, found {} after the expression",
                        self.found()
                    ),
                );
            }
        };
        self.pos += 1;
        let rhs = self.expr()?;
        Ok(BodyItem::Cmp { op, lhs, rhs })
    }

    fn neg_atom(&mut self) -> Result<Atom, ParseError> {
        if !self.at_atom_start(false) {
            return err(
                self.line(),
                format!("expected an atom after negation, found {}", self.found()),
            );
        }
        self.atom(false)
    }

    fn atom(&mut self, head: bool) -> Result<Atom, ParseError> {
        let line = self.line();
        let (pred, iri_pred) = match self.next() {
            Some(Tok::Ident(n)) => (n, false),
            Some(Tok::PName(p, l)) => (self.resolve_pname(&p, &l, line)?, true),
            Some(Tok::Iri(raw)) => (self.resolve_iri(&raw, line)?, true),
            _ => return err(line, "expected an atom"),
        };
        self.expect(Tok::LParen, "`(`")?;
        let mut args = Vec::new();
        if self.peek() == Some(&Tok::RParen) {
            self.pos += 1;
            return Ok(Atom {
                pred,
                iri_pred,
                args,
            });
        }
        loop {
            args.push(self.arg(head)?);
            match self.peek() {
                Some(Tok::Comma) => self.pos += 1,
                Some(Tok::RParen) => {
                    self.pos += 1;
                    break;
                }
                Some(Tok::Plus | Tok::Minus | Tok::Star | Tok::Slash) => {
                    return err(
                        self.line(),
                        "arithmetic in atom arguments is not supported; bind it with `?v = expr` in the body",
                    );
                }
                _ => {
                    return err(
                        self.line(),
                        format!("expected `,` or `)` in atom, found {}", self.found()),
                    );
                }
            }
        }
        Ok(Atom {
            pred,
            iri_pred,
            args,
        })
    }

    fn arg(&mut self, head: bool) -> Result<Arg, ParseError> {
        let line = self.line();
        if let Some(Tok::Hash(name)) = self.peek().cloned() {
            if !head {
                return err(line, "aggregates are only allowed in rule heads");
            }
            self.pos += 1;
            let func = match name.as_str() {
                "count" => AggFunc::Count,
                "sum" => AggFunc::Sum,
                "min" => AggFunc::Min,
                "max" => AggFunc::Max,
                other => return err(line, format!("aggregate `#{other}` is not supported")),
            };
            self.expect(Tok::LParen, "`(` after aggregate")?;
            let var = match self.next() {
                Some(Tok::Var(v)) if v != "_" => v,
                Some(Tok::Ident(v)) if is_var_name(&v) && v != "_" => v,
                _ => return err(line, format!("`#{name}` takes one named variable")),
            };
            match self.next() {
                Some(Tok::RParen) => {}
                Some(Tok::Comma) => {
                    return err(
                        line,
                        format!("`#{name}` with several variables is not supported"),
                    );
                }
                _ => return err(line, format!("expected `)` after `#{name}(...`")),
            }
            return Ok(Arg::Aggregate { func, var });
        }
        Ok(Arg::Term(self.term()?))
    }

    /// A variable, constant or literal.
    fn term(&mut self) -> Result<TermAst, ParseError> {
        let line = self.line();
        let tok = self.next();
        let call_follows = self.peek() == Some(&Tok::LParen);
        let t = match tok {
            Some(Tok::Var(v)) => {
                if v == "_" {
                    TermAst::Anon
                } else {
                    TermAst::Var(v)
                }
            }
            Some(Tok::Ident(n)) => {
                if call_follows {
                    return err(
                        line,
                        format!("function terms (`{n}(...)`) are not supported"),
                    );
                }
                if n == "_" {
                    TermAst::Anon
                } else if is_var_name(&n) {
                    TermAst::Var(n)
                } else if n == "true" || n == "false" {
                    TermAst::Literal(Literal::typed(n, xsd("boolean")))
                } else {
                    match self.prefixes.get("") {
                        Some(ns) => TermAst::Iri(format!("{ns}{n}")),
                        None => {
                            return err(
                                line,
                                format!(
                                    "bare constant `{n}` needs a default prefix (`@prefix : <...> .`)"
                                ),
                            );
                        }
                    }
                }
            }
            Some(Tok::PName(p, l)) => {
                if call_follows {
                    return err(
                        line,
                        format!("function terms (`{p}:{l}(...)`) are not supported"),
                    );
                }
                TermAst::Iri(self.resolve_pname(&p, &l, line)?)
            }
            Some(Tok::Iri(raw)) => {
                if call_follows {
                    return err(line, "function terms (`<iri>(...)`) are not supported");
                }
                TermAst::Iri(self.resolve_iri(&raw, line)?)
            }
            Some(Tok::Str(s, lang)) => {
                if let Some(tag) = lang {
                    TermAst::Literal(Literal::lang(s, tag))
                } else if self.peek() == Some(&Tok::Caret2) {
                    self.pos += 1;
                    let dt = match self.next() {
                        Some(Tok::Iri(raw)) => self.resolve_iri(&raw, line)?,
                        Some(Tok::PName(p, l)) => self.resolve_pname(&p, &l, line)?,
                        _ => return err(line, "expected a datatype IRI after `^^`"),
                    };
                    TermAst::Literal(Literal::typed(s, dt))
                } else {
                    TermAst::Literal(Literal::string(s))
                }
            }
            Some(Tok::Num(kind, lex)) => TermAst::Literal(number(kind, &lex, false)),
            Some(Tok::Minus) => match self.next() {
                Some(Tok::Num(kind, lex)) => TermAst::Literal(number(kind, &lex, true)),
                _ => return err(line, "expected a number after `-`"),
            },
            Some(Tok::LParen) => return err(line, "nested tuples (`(...)`) are not supported"),
            Some(Tok::Directive(d)) => {
                return err(line, format!("`@{d}` cannot appear inside a term"));
            }
            Some(t) => return err(line, format!("expected a term, found {}", describe(&t))),
            None => return err(line, "expected a term, found end of input"),
        };
        Ok(t)
    }

    // expr := mul (('+'|'-') mul)*
    fn expr(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.mul()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => return Ok(lhs),
            };
            self.pos += 1;
            let rhs = self.mul()?;
            lhs = Expr::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    // mul := unary (('*'|'/') unary)*
    fn mul(&mut self) -> Result<Expr, ParseError> {
        let mut lhs = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => BinOp::Mul,
                Some(Tok::Slash) => BinOp::Div,
                _ => return Ok(lhs),
            };
            self.pos += 1;
            let rhs = self.unary()?;
            lhs = Expr::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        if self.peek() == Some(&Tok::Minus) {
            if matches!(self.peek_at(1), Some(Tok::Num(..))) {
                return Ok(Expr::Term(self.term()?));
            }
            self.pos += 1;
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let line = self.line();
        match self.peek().cloned() {
            Some(Tok::LParen) => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(Tok::RParen, "`)`")?;
                Ok(e)
            }
            Some(Tok::Ident(n)) if self.peek_at(1) == Some(&Tok::LParen) => {
                if !is_builtin_name(&n) {
                    return err(
                        line,
                        format!(
                            "`{n}(...)` in an expression: function terms are not supported (built-ins are ALL-CAPS)"
                        ),
                    );
                }
                self.pos += 2;
                let mut args = Vec::new();
                if self.peek() == Some(&Tok::RParen) {
                    self.pos += 1;
                } else {
                    loop {
                        args.push(self.expr()?);
                        match self.next() {
                            Some(Tok::Comma) => {}
                            Some(Tok::RParen) => break,
                            _ => {
                                return err(line, format!("expected `,` or `)` in call to `{n}`"));
                            }
                        }
                    }
                }
                Ok(Expr::Call {
                    name: n.to_ascii_uppercase(),
                    args,
                })
            }
            Some(Tok::Hash(h)) => err(
                line,
                format!("aggregate `#{h}` is only allowed in rule heads"),
            ),
            _ => Ok(Expr::Term(self.term()?)),
        }
    }

    fn resolve_pname(&self, p: &str, local: &str, line: usize) -> Result<String, ParseError> {
        if p == "_" {
            return err(line, format!("blank nodes (`_:{local}`) are not supported"));
        }
        match self.prefixes.get(p) {
            Some(ns) => Ok(format!("{ns}{local}")),
            None => err(line, format!("unknown prefix `{p}:`")),
        }
    }

    fn resolve_iri(&self, raw: &str, line: usize) -> Result<String, ParseError> {
        if is_absolute_iri(raw) {
            return Ok(raw.to_string());
        }
        match &self.base {
            Some(b) => Ok(format!("{b}{raw}")),
            None => err(
                line,
                format!("relative IRI `<{raw}>` needs a `@base` directive"),
            ),
        }
    }
}

fn number(kind: NumKind, lex: &str, neg: bool) -> Literal {
    let signed = if neg {
        format!("-{lex}")
    } else {
        lex.to_string()
    };
    match kind {
        NumKind::Integer => match signed.parse::<i64>() {
            Ok(v) => Literal::integer(v),
            Err(_) => Literal::typed(signed, xsd("integer")),
        },
        NumKind::Decimal => Literal::decimal(signed),
        NumKind::Double => match signed.parse::<f64>() {
            Ok(v) => Literal::double(v),
            Err(_) => Literal::typed(signed, xsd("double")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EX: &str = "http://example.org/";
    const PFX: &str = "@prefix : <http://example.org/> .\n";

    fn ok(src: &str) -> Program {
        parse(src, &[]).unwrap_or_else(|e| panic!("parse failed: {e}\n{src}"))
    }

    fn fail(src: &str) -> ParseError {
        match parse(src, &[]) {
            Ok(p) => panic!("expected error, got {p:?}"),
            Err(e) => e,
        }
    }

    fn ex(l: &str) -> TermAst {
        TermAst::Iri(format!("{EX}{l}"))
    }

    fn v(n: &str) -> TermAst {
        TermAst::Var(n.into())
    }

    fn a(pred: &str, args: Vec<TermAst>) -> Atom {
        Atom {
            pred: pred.into(),
            iri_pred: false,
            args: args.into_iter().map(Arg::Term).collect(),
        }
    }

    fn lit_of(src_term: &str) -> TermAst {
        let p = ok(&format!("{PFX}f({src_term}) ."));
        match &p.rules[0].head.args[0] {
            Arg::Term(t) => t.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn transitive_closure_scenario() {
        let p = ok(&format!(
            "{PFX}reach(X,Y) :- triple(X, :knows, Y).\nreach(X,Z) :- reach(X,Y), triple(Y, :knows, Z).\n?- reach(:a, Y).\n"
        ));
        assert_eq!(p.rules.len(), 2);
        assert_eq!(p.rules[0].head, a("reach", vec![v("X"), v("Y")]));
        assert_eq!(
            p.rules[0].body,
            vec![BodyItem::Pos(a(
                "triple",
                vec![v("X"), ex("knows"), v("Y")]
            ))]
        );
        assert_eq!(p.rules[1].body.len(), 2);
        assert_eq!(p.rules[0].line, 2);
        assert_eq!(p.rules[1].line, 3);
        assert!(!p.rules[0].assert);
        assert_eq!(p.goals.len(), 1);
        assert_eq!(p.goals[0].line, 4);
        assert_eq!(
            p.goals[0].body,
            vec![BodyItem::Pos(a("reach", vec![ex("a"), v("Y")]))]
        );
    }

    #[test]
    fn nemo_question_variables() {
        let p = ok(&format!(
            "{PFX}reach(?x, ?z) :- reach(?x, ?y), edge(?y, ?z) .\n?- reach(:a, ?y) ."
        ));
        assert_eq!(p.rules[0].head, a("reach", vec![v("x"), v("z")]));
        assert_eq!(
            p.goals[0].body,
            vec![BodyItem::Pos(a("reach", vec![ex("a"), v("y")]))]
        );
    }

    #[test]
    fn output_directive() {
        let p = ok(&format!(
            "{PFX}reach(X,Y) :- triple(X, :knows, Y).\n@output reach .\n"
        ));
        assert_eq!(p.outputs, vec!["reach".to_string()]);
        assert!(p.goals.is_empty());
    }

    #[test]
    fn assert_scenario() {
        let p = ok(&format!(
            "{PFX}@assert triple(X, :colleague, Y) :- triple(X, :worksAt, O), triple(Y, :worksAt, O), X != Y."
        ));
        let r = &p.rules[0];
        assert!(r.assert);
        assert_eq!(r.mode, None);
        assert_eq!(r.head, a("triple", vec![v("X"), ex("colleague"), v("Y")]));
        assert_eq!(r.body.len(), 3);
        assert_eq!(
            r.body[2],
            BodyItem::Cmp {
                op: CmpOp::Ne,
                lhs: Expr::Term(v("X")),
                rhs: Expr::Term(v("Y")),
            }
        );
    }

    #[test]
    fn annotation_applies_to_next_rule_only() {
        let p = ok(&format!(
            "{PFX}@assert\nedge(:a, :b) .\nedge(:b, :c) .\n@assert @materialized\nr(X) :- edge(X, _) ."
        ));
        assert!(p.rules[0].assert);
        assert!(p.rules[0].body.is_empty());
        assert!(!p.rules[1].assert);
        assert!(p.rules[2].assert);
        assert_eq!(
            p.rules[2].mode,
            Some(RuleMode::Materialized { force: false })
        );
    }

    #[test]
    fn mode_annotations() {
        let p = ok(&format!(
            "{PFX}@materialized\nancestor(?x, ?y) :- :parent(?x, ?y) .\n@materialized(force)\nb(?x) :- a(?x) .\n@on_demand\nc(?x) :- a(?x) .\nd(?x) :- a(?x) ."
        ));
        assert_eq!(
            p.rules[0].mode,
            Some(RuleMode::Materialized { force: false })
        );
        assert_eq!(
            p.rules[1].mode,
            Some(RuleMode::Materialized { force: true })
        );
        assert_eq!(p.rules[2].mode, Some(RuleMode::OnDemand));
        assert_eq!(p.rules[3].mode, None);
        assert_eq!(
            p.rules[0].body[0],
            BodyItem::Pos(Atom {
                pred: format!("{EX}parent"),
                iri_pred: true,
                args: vec![Arg::Term(v("x")), Arg::Term(v("y"))],
            })
        );
    }

    #[test]
    fn dangling_or_misplaced_annotation() {
        assert!(fail("@assert").message.contains("@assert"));
        let e = fail("@materialized\n@output p .");
        assert!(e.message.contains("must precede"));
        assert!(fail("@assert ?- p(X) .").message.contains("goal"));
        assert!(
            fail("@materialized(lazy) p(X) :- q(X) .")
                .message
                .contains("force")
        );
    }

    #[test]
    fn negation_forms() {
        let p = ok("p(X) :- q(X), not p(X).\nr(X) :- q(X), ~s(X) .");
        assert_eq!(
            p.rules[0].body,
            vec![
                BodyItem::Pos(a("q", vec![v("X")])),
                BodyItem::Neg(a("p", vec![v("X")])),
            ]
        );
        assert_eq!(p.rules[1].body[1], BodyItem::Neg(a("s", vec![v("X")])));
    }

    #[test]
    fn negated_iri_atom() {
        let p = ok(&format!("{PFX}p(?x) :- q(?x), ~:blocked(?x) ."));
        match &p.rules[0].body[1] {
            BodyItem::Neg(at) => {
                assert!(at.iri_pred);
                assert_eq!(at.pred, format!("{EX}blocked"));
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn aggregates_in_heads() {
        let p = ok(
            "cnt(?x, #count(?y)) :- edge(?x, ?y) .\ns(X, #sum(Y)) :- w(X, Y) .\nlo(#min(?y)) :- w(_, ?y) .\nhi(#max(?y)) :- w(_, ?y) .",
        );
        let aggs: Vec<_> = p
            .rules
            .iter()
            .map(|r| r.head.args.last().unwrap().clone())
            .collect();
        assert_eq!(
            aggs,
            vec![
                Arg::Aggregate {
                    func: AggFunc::Count,
                    var: "y".into()
                },
                Arg::Aggregate {
                    func: AggFunc::Sum,
                    var: "Y".into()
                },
                Arg::Aggregate {
                    func: AggFunc::Min,
                    var: "y".into()
                },
                Arg::Aggregate {
                    func: AggFunc::Max,
                    var: "y".into()
                },
            ]
        );
        assert_eq!(
            p.rules[2].body[0],
            BodyItem::Pos(a("w", vec![TermAst::Anon, v("y")]))
        );
    }

    #[test]
    fn aggregate_errors() {
        assert!(
            fail("p(#count(?y)) :- q(#count(?y)) .")
                .message
                .contains("only allowed in rule heads")
        );
        assert!(fail("p(#avg(?y)) :- q(?y) .").message.contains("`#avg`"));
        assert!(
            fail("p(#count(?y, ?z)) :- q(?y, ?z) .")
                .message
                .contains("several variables")
        );
        assert!(
            fail("p(#count(:a)) :- q(?y) .")
                .message
                .contains("named variable")
        );
        assert!(fail("p(#count(?y)) .").message.contains("facts"));
    }

    #[test]
    fn literals_of_each_kind() {
        assert_eq!(lit_of("\"hi\""), TermAst::Literal(Literal::string("hi")));
        assert_eq!(
            lit_of(r#""a\"b\\c\nd\te""#),
            TermAst::Literal(Literal::string("a\"b\\c\nd\te"))
        );
        assert_eq!(
            lit_of("\"chat\"@fr"),
            TermAst::Literal(Literal::lang("chat", "fr"))
        );
        assert_eq!(
            lit_of("\"colour\"@en-GB"),
            TermAst::Literal(Literal::lang("colour", "en-GB"))
        );
        assert_eq!(
            lit_of("\"5\"^^xsd:integer"),
            TermAst::Literal(Literal::typed("5", xsd("integer")))
        );
        assert_eq!(
            lit_of("\"x\"^^<http://ex.org/dt>"),
            TermAst::Literal(Literal::typed("x", "http://ex.org/dt"))
        );
        assert_eq!(lit_of("42"), TermAst::Literal(Literal::integer(42)));
        assert_eq!(lit_of("-7"), TermAst::Literal(Literal::integer(-7)));
        assert_eq!(lit_of("1.5"), TermAst::Literal(Literal::decimal("1.5")));
        assert_eq!(lit_of("-0.25"), TermAst::Literal(Literal::decimal("-0.25")));
        assert_eq!(lit_of("1.5e3"), TermAst::Literal(Literal::double(1500.0)));
        assert_eq!(lit_of("2E-1"), TermAst::Literal(Literal::double(0.2)));
        assert_eq!(lit_of("true"), TermAst::Literal(Literal::boolean(true)));
        assert_eq!(lit_of("false"), TermAst::Literal(Literal::boolean(false)));
        assert_eq!(
            lit_of("99999999999999999999"),
            TermAst::Literal(Literal::typed("99999999999999999999", xsd("integer")))
        );
    }

    #[test]
    fn integer_before_statement_dot() {
        let p = ok("age(\"bob\", 42).");
        assert_eq!(
            p.rules[0].head.args[1],
            Arg::Term(TermAst::Literal(Literal::integer(42)))
        );
    }

    #[test]
    fn iri_resolution() {
        let src = "@prefix ex: <http://ex.org/ns#> .\n@base <http://base.org/> .\n@prefix rel: <r/> .\nf(<http://abs.org/x>, ex:y, <rel>, rel:z, ex:a.b) .";
        let p = ok(src);
        let args: Vec<_> = p.rules[0].head.args.clone();
        let iri = |s: &str| Arg::Term(TermAst::Iri(s.into()));
        assert_eq!(
            args,
            vec![
                iri("http://abs.org/x"),
                iri("http://ex.org/ns#y"),
                iri("http://base.org/rel"),
                iri("http://base.org/r/z"),
                iri("http://ex.org/ns#a.b"),
            ]
        );
        assert!(fail("f(<rel>) .").message.contains("@base"));
    }

    #[test]
    fn base_prefixes_argument() {
        let pre = vec![("ex".to_string(), "http://ex.org/".to_string())];
        let p = parse("f(ex:a) .", &pre).unwrap();
        assert_eq!(
            p.rules[0].head.args[0],
            Arg::Term(TermAst::Iri("http://ex.org/a".into()))
        );
        let p = parse("@prefix ex: <http://other.org/> .\nf(ex:a) .", &pre).unwrap();
        assert_eq!(
            p.rules[0].head.args[0],
            Arg::Term(TermAst::Iri("http://other.org/a".into()))
        );
    }

    #[test]
    fn bare_constants() {
        let p = ok(&format!("{PFX}edge(alice, bob) ."));
        assert_eq!(p.rules[0].head, a("edge", vec![ex("alice"), ex("bob")]));
        let e = fail("edge(alice, bob) .");
        assert!(
            e.message
                .contains("bare constant `alice` needs a default prefix")
        );
    }

    #[test]
    fn iri_and_pname_predicates() {
        let p = ok(&format!(
            "{PFX}@prefix ex: <http://ex.org/> .\nq(?x) :- :knows(?x, ?y), ex:likes(?y, ?x), <http://z.org/p>(?x, ?x) ."
        ));
        let preds: Vec<_> = p.rules[0]
            .body
            .iter()
            .map(|b| match b {
                BodyItem::Pos(at) => (at.pred.clone(), at.iri_pred),
                o => panic!("{o:?}"),
            })
            .collect();
        assert_eq!(
            preds,
            vec![
                (format!("{EX}knows"), true),
                ("http://ex.org/likes".to_string(), true),
                ("http://z.org/p".to_string(), true),
            ]
        );
        let p = ok(&format!("{PFX}:knows(:a, :b) ."));
        assert!(p.rules[0].head.iri_pred);
    }

    #[test]
    fn comparisons_and_expressions() {
        let p = ok(
            "p(?n, ?l) :- q(?a, ?s), ?n = ?a + 1 * -?a, ?l = STRLEN(?s), ?a >= 3, ?a <= 9, ?a < 10, ?a > (2 - 1) / 2, ?a = -5 .",
        );
        let b = &p.rules[0].body;
        assert_eq!(
            b[1],
            BodyItem::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Term(v("n")),
                rhs: Expr::Bin {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Term(v("a"))),
                    rhs: Box::new(Expr::Bin {
                        op: BinOp::Mul,
                        lhs: Box::new(Expr::Term(TermAst::Literal(Literal::integer(1)))),
                        rhs: Box::new(Expr::Neg(Box::new(Expr::Term(v("a"))))),
                    }),
                },
            }
        );
        assert_eq!(
            b[2],
            BodyItem::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Term(v("l")),
                rhs: Expr::Call {
                    name: "STRLEN".into(),
                    args: vec![Expr::Term(v("s"))],
                },
            }
        );
        let ops: Vec<_> = b[3..7]
            .iter()
            .map(|i| match i {
                BodyItem::Cmp { op, .. } => *op,
                o => panic!("{o:?}"),
            })
            .collect();
        assert_eq!(ops, vec![CmpOp::Ge, CmpOp::Le, CmpOp::Lt, CmpOp::Gt]);
        match &b[6] {
            BodyItem::Cmp { rhs, .. } => assert!(matches!(rhs, Expr::Bin { op: BinOp::Div, .. })),
            o => panic!("{o:?}"),
        }
        assert_eq!(
            b[7],
            BodyItem::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Term(v("a")),
                rhs: Expr::Term(TermAst::Literal(Literal::integer(-5))),
            }
        );
    }

    #[test]
    fn builtin_calls_nested_and_on_lhs() {
        let p = ok(
            "p(?r) :- q(?a, ?b), ?r = CONCAT(?a, \"-\", ?b), ABS(?a - ?b) > 2, STRLEN(?a) = 3, ?z = NOW() .",
        );
        let b = &p.rules[0].body;
        match &b[1] {
            BodyItem::Cmp {
                rhs: Expr::Call { name, args },
                ..
            } => {
                assert_eq!(name, "CONCAT");
                assert_eq!(args.len(), 3);
            }
            o => panic!("{o:?}"),
        }
        match &b[2] {
            BodyItem::Cmp {
                lhs: Expr::Call { name, args },
                op: CmpOp::Gt,
                ..
            } => {
                assert_eq!(name, "ABS");
                assert!(matches!(args[0], Expr::Bin { op: BinOp::Sub, .. }));
            }
            o => panic!("{o:?}"),
        }
        match &b[4] {
            BodyItem::Cmp {
                rhs: Expr::Call { name, args },
                ..
            } => {
                assert_eq!(name, "NOW");
                assert!(args.is_empty());
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn less_than_without_spaces_is_not_an_iri() {
        let p = ok("p(?a) :- q(?a), ?a<3 .");
        assert!(matches!(
            p.rules[0].body[1],
            BodyItem::Cmp { op: CmpOp::Lt, .. }
        ));
    }

    #[test]
    fn mixed_case_predicate_is_atom() {
        let p = ok("Person(X) :- Human(X) .");
        assert_eq!(p.rules[0].head.pred, "Person");
        assert_eq!(p.rules[0].body[0], BodyItem::Pos(a("Human", vec![v("X")])));
    }

    #[test]
    fn anonymous_variables() {
        let p = ok("p(X) :- q(X, _, ?_) .");
        assert_eq!(
            p.rules[0].body[0],
            BodyItem::Pos(a("q", vec![v("X"), TermAst::Anon, TermAst::Anon]))
        );
    }

    #[test]
    fn zero_arity_atoms() {
        let p = ok("flag() .\ngo() :- flag() .");
        assert!(p.rules[0].head.args.is_empty());
        assert_eq!(p.rules[1].body[0], BodyItem::Pos(a("flag", vec![])));
    }

    #[test]
    fn comments() {
        let p = ok(&format!(
            "% a Nemo comment\n{PFX}// a C++ comment\nedge(:a, :b) . % trailing\nedge(:b, :c) . // trailing too\n"
        ));
        assert_eq!(p.rules.len(), 2);
        assert_eq!(p.rules[0].line, 4);
        assert_eq!(p.rules[1].line, 5);
    }

    #[test]
    fn multiline_rule_lines() {
        let p = ok(&format!(
            "{PFX}\n\nr(X,\n  Y) :-\n  e(X, Y) .\n?- r(:a,\n Y) ."
        ));
        assert_eq!(p.rules[0].line, 4);
        assert_eq!(p.goals[0].line, 7);
    }

    #[test]
    fn fact_with_variable_is_error() {
        let e = fail("p(X) .");
        assert!(e.message.contains("variable `X`"), "{}", e.message);
        assert!(fail("p(_) .").message.contains("ground"));
    }

    #[test]
    fn unknown_prefix() {
        let e = fail("@prefix ex: <http://ex.org/> .\n\np(X) :- foo:bar(X) .");
        assert_eq!(e.line, 3);
        assert_eq!(e.message, "unknown prefix `foo:`");
        assert!(fail("f(\"1\"^^zz:int) .").message.contains("`zz:`"));
    }

    #[test]
    fn unsupported_directives() {
        let e = fail("@import edge :- csv{resource=\"e.csv\"} .");
        assert!(e.message.contains("`@import`"), "{}", e.message);
        assert!(
            fail("@export reach :- csv{} .")
                .message
                .contains("`@export`")
        );
        assert!(
            fail("@declare edge(a:any, b:any) .")
                .message
                .contains("`@declare`")
        );
        assert!(fail("@parameter $x = 3 .").message.contains("`@parameter`"));
        assert!(
            fail("@frobnicate .")
                .message
                .contains("unknown directive `@frobnicate`")
        );
    }

    #[test]
    fn existential_variables() {
        let e = fail("p(?x, !y) :- q(?x) .");
        assert!(e.message.contains("existential variables"), "{}", e.message);
        assert!(e.message.contains("!y"));
    }

    #[test]
    fn function_terms_and_tuples() {
        let e = fail("p(f(?x)) :- q(?x) .");
        assert!(e.message.contains("function terms"), "{}", e.message);
        assert!(
            fail("p(?y) :- q(?x), ?y = f(?x) .")
                .message
                .contains("function terms")
        );
        assert!(
            fail("p((?x, ?y)) :- q(?x, ?y) .")
                .message
                .contains("nested tuples")
        );
        assert!(
            fail("p(?x + 1) :- q(?x) .")
                .message
                .contains("arithmetic in atom arguments")
        );
        assert!(
            fail("p(?x) :- q(?x), r({a = 1}) .")
                .message
                .contains("maps")
        );
        assert!(fail("p(?x) :- q([1, 2]) .").message.contains("lists"));
    }

    #[test]
    fn multiple_and_disjunctive_heads() {
        let e = fail("a(?x), b(?x) :- c(?x) .");
        assert_eq!(e.message, "multiple head atoms are not supported");
        assert!(
            fail("a(?x) | b(?x) :- c(?x) .")
                .message
                .contains("disjunctive heads")
        );
        assert!(
            fail("a(?x) :- b(?x) | c(?x) .")
                .message
                .contains("disjunction")
        );
        assert!(fail("~a(?x) :- c(?x) .").message.contains("negated heads"));
    }

    #[test]
    fn blank_nodes_rejected() {
        assert!(fail("p(_:b1) .").message.contains("blank nodes"));
    }

    #[test]
    fn syntax_errors_carry_lines() {
        let e = fail("p(X) :- \n q(X)\n r(X) .");
        assert_eq!(e.line, 3);
        let e = fail(&format!("{PFX}p(X) :- q(X)"));
        assert_eq!(e.line, 2);
        let e = fail("\n\n\"unterminated");
        assert_eq!(e.line, 3);
        let e = fail("p(X) :- q(X), X .");
        assert!(e.message.contains("comparison"));
        let e = fail("p(X) :- .");
        assert!(e.message.contains("empty body"));
        let e = fail("\n?x");
        assert_eq!(e.line, 2);
    }

    #[test]
    fn absolute_iri_detection() {
        assert!(is_absolute_iri("http://x"));
        assert!(is_absolute_iri("urn:isbn:1"));
        assert!(is_absolute_iri("a+b.c-d:x"));
        assert!(!is_absolute_iri("rel"));
        assert!(!is_absolute_iri("1http://x"));
        assert!(!is_absolute_iri("a/b:c"));
    }

    #[test]
    fn display_error() {
        let e = fail("@import x .");
        assert!(e.to_string().starts_with("line 1: "));
    }
}
