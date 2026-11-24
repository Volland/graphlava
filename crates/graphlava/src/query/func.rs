//! SPARQL 1.1/1.2 operators and functions over terms
//! (SPARQL 1.1 Query §17, XPath and XQuery Functions 3.1).
//!
//! Every operator works on [`Term`] values and reports a type error as an
//! [`ExprError`]; a `FILTER` treats an error as `false`.
//!
//! Numeric values follow the XSD promotion chain
//! `integer → decimal → float → double`. `xsd:integer` (and its derived types)
//! is held as an `i128` with overflow reported as an error; `xsd:decimal` is an
//! exact `i128`-scaled fixed-point number (never an `f64`); integer division
//! yields a decimal with up to [`DEC_DIV_DIGITS`] fractional digits.
//!
//! Results use canonical lexical forms: integer `"1"`, decimal `"1.5"` /
//! `"1.0"`, double and float `"1.0E0"`, boolean `"true"`.
//!
//! Choices left open by the specification:
//! - `BNODE()` returns a fresh blank node per call. `BNODE(str)` returns the
//!   same blank node for the same string within one solution; the evaluator
//!   calls [`FnContext::next_row`] between solutions so different solutions
//!   get different nodes.
//! - `IRI(str)` resolves against [`FnContext::base_iri`] when set and returns
//!   the string unchanged otherwise.
//! - `TRIPLE(s, p, o)` needs an IRI or blank node subject and an IRI
//!   predicate; `=` on two triple terms compares their components with `=`,
//!   and `ORDER BY` puts triple terms after literals, ordered by subject,
//!   predicate and object (SPARQL 1.2 §15.1).
//! - `Term` has no base direction, so `STRLANGDIR` raises an error;
//!   `LANGDIR` returns `""`, `HASLANGDIR` returns `false`.
//! - `=` on two literals of different known datatypes (e.g. `1 = "1"`) is
//!   `false`; it is an error only when a literal has an unknown datatype or is
//!   ill-typed and the two terms are not the same term (§17.4.1.7).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

use spargebra::algebra::Function;

use crate::literal::{days_from_civil, double_lexical};
use crate::term::{Literal, RDF_LANG_STRING, Term, XSD, xsd};

/// A SPARQL expression error (type error / unbound). Filters treat it as false.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError(pub String);

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExprError {}

pub type FResult<T> = Result<T, ExprError>;

fn err<T>(msg: impl Into<String>) -> FResult<T> {
    Err(ExprError(msg.into()))
}

/// Fractional digits kept by decimal division.
pub const DEC_DIV_DIGITS: u32 = 24;

// ---- per-query context ------------------------------------------------------

/// Per-query context for non-deterministic functions.
#[derive(Debug)]
pub struct FnContext {
    /// `NOW()`: an `xsd:dateTime` literal fixed for the query.
    pub now: Term,
    /// Base IRI that `IRI()`/`URI()` resolve relative references against.
    pub base_iri: Option<String>,
    bnode_counter: u64,
    /// `BNODE(str)` labels by (solution key, string).
    row_bnodes: HashMap<(Vec<u64>, String), String>,
    /// Key of the current solution (see [`FnContext::set_row`]).
    row: Vec<u64>,
    rows_seen: u64,
    rng: u64,
    regex_cache: HashMap<(String, String), regex::Regex>,
}

impl FnContext {
    /// A context whose `NOW()` is `now_utc_lexical` as an `xsd:dateTime`.
    pub fn new(now_utc_lexical: &str) -> FnContext {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut seed = nanos ^ 0x9E37_79B9_7F4A_7C15;
        for b in now_utc_lexical.bytes() {
            seed = seed.rotate_left(5) ^ u64::from(b);
        }
        FnContext {
            now: Term::Literal(Literal::date_time(now_utc_lexical)),
            base_iri: None,
            bnode_counter: 0,
            row_bnodes: HashMap::new(),
            row: Vec::new(),
            rows_seen: 0,
            rng: seed,
            regex_cache: HashMap::new(),
        }
    }

    /// Marks the start of a new solution: `BNODE(str)` labels are scoped to
    /// one solution.
    pub fn next_row(&mut self) {
        self.rows_seen += 1;
        self.row = vec![u64::MAX, self.rows_seen];
    }

    /// Marks the solution being evaluated by a key: expressions evaluated
    /// for the same key (several `BIND`s or `SELECT` expressions of one
    /// solution) share `BNODE(str)` labels.
    pub fn set_row(&mut self, key: Vec<u64>) {
        self.row = key;
    }

    /// A blank node not returned before by this context.
    pub fn fresh_bnode(&mut self) -> Term {
        self.bnode_counter += 1;
        Term::Blank(format!("fn{}", self.bnode_counter))
    }

    /// splitmix64.
    fn next_u64(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uuid(&mut self) -> String {
        let a = self.next_u64().to_be_bytes();
        let b = self.next_u64().to_be_bytes();
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&a);
        bytes[8..].copy_from_slice(&b);
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let h = hex(&bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    }

    fn regex(&mut self, pattern: &str, flags: &str) -> FResult<&regex::Regex> {
        let key = (pattern.to_string(), flags.to_string());
        if !self.regex_cache.contains_key(&key) {
            let re = build_regex(pattern, flags)?;
            if self.regex_cache.len() > 256 {
                self.regex_cache.clear();
            }
            self.regex_cache.insert(key.clone(), re);
        }
        Ok(&self.regex_cache[&key])
    }
}

// ---- exact decimals ---------------------------------------------------------

fn pow10(n: u32) -> Option<i128> {
    10i128.checked_pow(n)
}

/// An exact decimal `m × 10^-s`, normalised (no trailing zeros in `m` when
/// `s > 0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Dec {
    m: i128,
    s: u32,
}

impl Dec {
    fn new(mut m: i128, mut s: u32) -> Dec {
        if m == 0 {
            return Dec { m: 0, s: 0 };
        }
        while s > 0 && m % 10 == 0 {
            m /= 10;
            s -= 1;
        }
        Dec { m, s }
    }

    fn int(i: i128) -> Dec {
        Dec { m: i, s: 0 }
    }

    /// Parses the `xsd:decimal` lexical space (no exponent).
    fn parse(lex: &str) -> Option<Dec> {
        let (neg, body) = match lex.as_bytes().first()? {
            b'-' => (true, &lex[1..]),
            b'+' => (false, &lex[1..]),
            _ => (false, lex),
        };
        let (ip, fp) = body.split_once('.').unwrap_or((body, ""));
        if ip.is_empty() && fp.is_empty() {
            return None;
        }
        if !ip.bytes().all(|b| b.is_ascii_digit()) || !fp.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let fp = fp.trim_end_matches('0');
        let mut m: i128 = 0;
        for b in ip.bytes().chain(fp.bytes()) {
            m = m.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
        }
        let s = u32::try_from(fp.len()).ok()?;
        Some(Dec::new(if neg { -m } else { m }, s))
    }

    fn from_f64(x: f64) -> Option<Dec> {
        if !x.is_finite() {
            return None;
        }
        Dec::parse(&format!("{x}"))
    }

    fn to_f64(self) -> f64 {
        self.to_string().parse().unwrap_or(f64::NAN)
    }

    fn rescale(self, s: u32) -> Option<i128> {
        self.m.checked_mul(pow10(s - self.s)?)
    }

    fn align(a: Dec, b: Dec) -> Option<(i128, i128, u32)> {
        let s = a.s.max(b.s);
        Some((a.rescale(s)?, b.rescale(s)?, s))
    }

    fn add(self, o: Dec) -> Option<Dec> {
        let (a, b, s) = Dec::align(self, o)?;
        Some(Dec::new(a.checked_add(b)?, s))
    }

    fn sub(self, o: Dec) -> Option<Dec> {
        let (a, b, s) = Dec::align(self, o)?;
        Some(Dec::new(a.checked_sub(b)?, s))
    }

    fn mul(self, o: Dec) -> Option<Dec> {
        Some(Dec::new(self.m.checked_mul(o.m)?, self.s + o.s))
    }

    fn neg(self) -> Option<Dec> {
        Some(Dec::new(self.m.checked_neg()?, self.s))
    }

    /// Division truncated to [`DEC_DIV_DIGITS`] fractional digits; `None` on
    /// a zero divisor or overflow.
    fn div(self, o: Dec) -> Option<Dec> {
        if o.m == 0 {
            return None;
        }
        let neg = (self.m < 0) != (o.m < 0);
        let d = o.m.unsigned_abs();
        let n = self.m.unsigned_abs();
        let mut q = n / d;
        let mut r = n % d;
        let mut scale = i64::from(self.s) - i64::from(o.s);
        let mut digits = 0;
        while r != 0 && digits < DEC_DIV_DIGITS {
            let (Some(q10), Some(r10)) = (q.checked_mul(10), r.checked_mul(10)) else {
                break;
            };
            q = q10 + r10 / d;
            r = r10 % d;
            scale += 1;
            digits += 1;
        }
        if scale < 0 {
            q = q.checked_mul(u128::try_from(pow10(u32::try_from(-scale).ok()?)?).ok()?)?;
            scale = 0;
        }
        let m = i128::try_from(q).ok()?;
        Some(Dec::new(
            if neg { -m } else { m },
            u32::try_from(scale).ok()?,
        ))
    }

    fn floor(self) -> Dec {
        match pow10(self.s) {
            Some(p) if self.s > 0 => Dec::int(self.m.div_euclid(p)),
            _ => self,
        }
    }

    fn ceil(self) -> Dec {
        match pow10(self.s) {
            Some(p) if self.s > 0 => Dec::int(-(-self.m).div_euclid(p)),
            _ => self,
        }
    }

    /// Round half towards positive infinity (`fn:round`).
    fn round(self) -> Option<Dec> {
        Some(self.add(Dec { m: 5, s: 1 })?.floor())
    }

    /// Integer part (truncation toward zero).
    fn trunc(self) -> i128 {
        match pow10(self.s) {
            Some(p) => self.m / p,
            None => 0,
        }
    }

    fn cmp_dec(self, o: Dec) -> Ordering {
        match Dec::align(self, o) {
            Some((a, b, _)) => a.cmp(&b),
            None => self
                .to_f64()
                .partial_cmp(&o.to_f64())
                .unwrap_or(Ordering::Equal),
        }
    }

    fn is_zero(self) -> bool {
        self.m == 0
    }
}

impl fmt::Display for Dec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digits = self.m.unsigned_abs().to_string();
        let s = self.s as usize;
        let sign = if self.m < 0 { "-" } else { "" };
        if s == 0 {
            return write!(f, "{sign}{digits}.0");
        }
        let padded = if digits.len() <= s {
            format!("{}{}", "0".repeat(s + 1 - digits.len()), digits)
        } else {
            digits
        };
        let (ip, fp) = padded.split_at(padded.len() - s);
        write!(f, "{sign}{ip}.{fp}")
    }
}

// ---- numerics ---------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Num {
    Int(i128),
    Dec(Dec),
    Float(f32),
    Double(f64),
}

impl Num {
    fn rank(self) -> u8 {
        match self {
            Num::Int(_) => 0,
            Num::Dec(_) => 1,
            Num::Float(_) => 2,
            Num::Double(_) => 3,
        }
    }

    fn to_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Dec(d) => d.to_f64(),
            Num::Float(f) => f64::from(f),
            Num::Double(d) => d,
        }
    }

    fn to_rank(self, r: u8) -> Num {
        match (self, r) {
            (Num::Int(i), 1) => Num::Dec(Dec::int(i)),
            (n, 2) if n.rank() < 2 => Num::Float(n.to_f64() as f32),
            (n, 3) if n.rank() < 3 => Num::Double(n.to_f64()),
            (n, _) => n,
        }
    }

    fn promote(a: Num, b: Num) -> (Num, Num) {
        let r = a.rank().max(b.rank());
        (a.to_rank(r), b.to_rank(r))
    }

    fn into_term(self) -> Term {
        match self {
            Num::Int(i) => typed(i.to_string(), "integer"),
            Num::Dec(d) => typed(d.to_string(), "decimal"),
            Num::Float(f) => typed(float_lexical(f), "float"),
            Num::Double(d) => typed(double_lexical(d), "double"),
        }
    }

    fn is_zero_or_nan(self) -> bool {
        match self {
            Num::Int(i) => i == 0,
            Num::Dec(d) => d.is_zero(),
            Num::Float(f) => f == 0.0 || f.is_nan(),
            Num::Double(d) => d == 0.0 || d.is_nan(),
        }
    }

    fn is_nan(self) -> bool {
        match self {
            Num::Float(f) => f.is_nan(),
            Num::Double(d) => d.is_nan(),
            _ => false,
        }
    }
}

fn num_cmp(a: Num, b: Num) -> Option<Ordering> {
    match Num::promote(a, b) {
        (Num::Int(x), Num::Int(y)) => Some(x.cmp(&y)),
        (Num::Dec(x), Num::Dec(y)) => Some(x.cmp_dec(y)),
        (Num::Float(x), Num::Float(y)) => x.partial_cmp(&y),
        (x, y) => x.to_f64().partial_cmp(&y.to_f64()),
    }
}

fn float_lexical(x: f32) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "INF".into() } else { "-INF".into() };
    }
    let s = format!("{x:E}");
    let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
    if m.contains('.') {
        format!("{m}E{e}")
    } else {
        format!("{m}.0E{e}")
    }
}

/// Integer-derived types with their inclusive bounds (`None` = unbounded).
fn integer_bounds(local: &str) -> Option<(Option<i128>, Option<i128>)> {
    Some(match local {
        "integer" => (None, None),
        "long" => (Some(i64::MIN.into()), Some(i64::MAX.into())),
        "int" => (Some(i32::MIN.into()), Some(i32::MAX.into())),
        "short" => (Some(i16::MIN.into()), Some(i16::MAX.into())),
        "byte" => (Some(i8::MIN.into()), Some(i8::MAX.into())),
        "nonNegativeInteger" => (Some(0), None),
        "positiveInteger" => (Some(1), None),
        "nonPositiveInteger" => (None, Some(0)),
        "negativeInteger" => (None, Some(-1)),
        "unsignedLong" => (Some(0), Some(u64::MAX.into())),
        "unsignedInt" => (Some(0), Some(u32::MAX.into())),
        "unsignedShort" => (Some(0), Some(u16::MAX.into())),
        "unsignedByte" => (Some(0), Some(u8::MAX.into())),
        _ => return None,
    })
}

fn in_bounds(v: i128, (lo, hi): (Option<i128>, Option<i128>)) -> bool {
    lo.is_none_or(|lo| v >= lo) && hi.is_none_or(|hi| v <= hi)
}

fn parse_integer(lex: &str) -> Option<i128> {
    let body = lex.strip_prefix(['+', '-']).unwrap_or(lex);
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    lex.parse().ok()
}

/// Parses the `xsd:double` / `xsd:float` lexical space.
fn parse_xsd_double(lex: &str) -> Option<f64> {
    match lex {
        "INF" | "+INF" => return Some(f64::INFINITY),
        "-INF" => return Some(f64::NEG_INFINITY),
        "NaN" => return Some(f64::NAN),
        _ => {}
    }
    let body = lex.strip_prefix(['+', '-']).unwrap_or(lex);
    let (mant, exp) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let (ip, fp) = mant.split_once('.').unwrap_or((mant, ""));
    if (ip.is_empty() && fp.is_empty())
        || !ip.bytes().all(|b| b.is_ascii_digit())
        || !fp.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    if let Some(e) = exp {
        let eb = e.strip_prefix(['+', '-']).unwrap_or(e);
        if eb.is_empty() || !eb.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    lex.parse().ok()
}

fn xsd_local(dt: &str) -> Option<&str> {
    dt.strip_prefix(XSD)
}

/// The numeric value of a literal: outer `None` when the datatype is not
/// numeric, inner `None` when the lexical form is ill-typed.
fn literal_num(l: &Literal) -> Option<Option<Num>> {
    let local = xsd_local(&l.datatype)?;
    let lex = l.lexical.as_str();
    if let Some(bounds) = integer_bounds(local) {
        return Some(
            parse_integer(lex)
                .filter(|&v| in_bounds(v, bounds))
                .map(Num::Int),
        );
    }
    match local {
        "decimal" => Some(Dec::parse(lex).map(Num::Dec)),
        "float" => Some(parse_xsd_double(lex).map(|d| {
            Num::Float(match lex {
                "INF" | "+INF" | "-INF" | "NaN" => d as f32,
                _ => lex.parse::<f32>().unwrap_or(d as f32),
            })
        })),
        "double" => Some(parse_xsd_double(lex).map(Num::Double)),
        _ => None,
    }
}

fn numeric(t: &Term) -> FResult<Num> {
    match t {
        Term::Literal(l) => match literal_num(l) {
            Some(Some(n)) => Ok(n),
            Some(None) => err(format!("ill-typed numeric literal {t}")),
            None => err(format!("not a numeric value: {t}")),
        },
        _ => err(format!("not a numeric value: {t}")),
    }
}

/// Whether `t` is a valid numeric literal (`isNumeric`).
pub fn is_numeric(t: &Term) -> bool {
    matches!(t, Term::Literal(l) if matches!(literal_num(l), Some(Some(_))))
}

// ---- date/time --------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
struct DateTimeV {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: Dec,
    /// Offset in minutes.
    tz: Option<i32>,
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

fn two_digits(s: &str) -> Option<u32> {
    if s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse().ok()
    } else {
        None
    }
}

fn parse_tz(s: &str) -> Option<Option<i32>> {
    match s {
        "" => Some(None),
        "Z" => Some(Some(0)),
        _ => {
            let sign = match s.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            if s.len() != 6 || s.as_bytes()[3] != b':' {
                return None;
            }
            let hh = two_digits(&s[1..3])? as i32;
            let mm = two_digits(&s[4..6])? as i32;
            if hh > 14 || mm > 59 || (hh == 14 && mm != 0) {
                return None;
            }
            Some(Some(sign * (hh * 60 + mm)))
        }
    }
}

/// Parses `xsd:dateTime` (or `xsd:date` when `date_only`).
fn parse_date_time(s: &str, date_only: bool) -> Option<DateTimeV> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s),
    };
    let ydash = body.find('-')?;
    let ys = &body[..ydash];
    if ys.len() < 4
        || (ys.len() > 4 && ys.starts_with('0'))
        || !ys.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let year: i64 = ys.parse().ok()?;
    let year = if neg { -year } else { year };
    let rest = &body[ydash + 1..];
    let month = two_digits(rest.get(..2)?)?;
    if rest.as_bytes().get(2) != Some(&b'-') {
        return None;
    }
    let day = two_digits(rest.get(3..5)?)?;
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let rest = &rest[5..];
    if date_only {
        return Some(DateTimeV {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: Dec::int(0),
            tz: parse_tz(rest)?,
        });
    }
    let rest = rest.strip_prefix('T')?;
    let hour = two_digits(rest.get(..2)?)?;
    let minute = two_digits(rest.get(3..5)?)?;
    if rest.as_bytes().get(2) != Some(&b':') || rest.as_bytes().get(5) != Some(&b':') {
        return None;
    }
    let sec_rest = rest.get(6..)?;
    let sec_len = sec_rest
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b'.')
        .count();
    let sec_str = &sec_rest[..sec_len];
    if sec_str.len() < 2
        || !sec_str.as_bytes()[..2].iter().all(u8::is_ascii_digit)
        || (sec_str.len() > 2 && (sec_str.as_bytes()[2] != b'.' || sec_str.len() == 3))
    {
        return None;
    }
    let second = Dec::parse(sec_str)?;
    let tz = parse_tz(&sec_rest[sec_len..])?;
    if minute > 59
        || second.cmp_dec(Dec::int(60)) != Ordering::Less
        || hour > 24
        || (hour == 24 && (minute != 0 || !second.is_zero()))
    {
        return None;
    }
    Some(DateTimeV {
        year,
        month,
        day,
        hour,
        minute,
        second,
        tz,
    })
}

impl DateTimeV {
    /// Seconds since the epoch; a value without timezone is read as UTC.
    fn instant(&self) -> Dec {
        let days = i128::from(days_from_civil(self.year, self.month, self.day));
        let mut secs = days * 86_400 + i128::from(self.hour) * 3600 + i128::from(self.minute) * 60;
        if let Some(tz) = self.tz {
            secs -= i128::from(tz) * 60;
        }
        Dec::int(secs).add(self.second).unwrap_or(Dec::int(secs))
    }
}

/// XPath comparison with the ±14h rule for a value without timezone;
/// `None` when indeterminate.
fn dt_cmp(a: &DateTimeV, b: &DateTimeV) -> Option<Ordering> {
    let (ia, ib) = (a.instant(), b.instant());
    match (a.tz.is_some(), b.tz.is_some()) {
        (true, true) | (false, false) => Some(ia.cmp_dec(ib)),
        (true, false) => {
            let w = Dec::int(14 * 3600);
            if ia.cmp_dec(ib.sub(w)?) == Ordering::Less {
                Some(Ordering::Less)
            } else if ia.cmp_dec(ib.add(w)?) == Ordering::Greater {
                Some(Ordering::Greater)
            } else {
                None
            }
        }
        (false, true) => dt_cmp(b, a).map(Ordering::reverse),
    }
}

// ---- literal values ---------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Val<'a> {
    Num(Num),
    Str(&'a str),
    Lang(&'a str, &'a str),
    Bool(bool),
    DateTime(DateTimeV),
    Date(DateTimeV),
    /// A known datatype with an invalid lexical form.
    Invalid,
    /// A datatype the operators do not know.
    Other,
}

fn parse_bool(lex: &str) -> Option<bool> {
    match lex {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn value(l: &Literal) -> Val<'_> {
    if let Some(n) = literal_num(l) {
        return n.map_or(Val::Invalid, Val::Num);
    }
    if l.datatype == RDF_LANG_STRING {
        return match &l.lang {
            Some(tag) => Val::Lang(&l.lexical, tag),
            None => Val::Invalid,
        };
    }
    match xsd_local(&l.datatype) {
        Some("string") => Val::Str(&l.lexical),
        Some("boolean") => parse_bool(&l.lexical).map_or(Val::Invalid, Val::Bool),
        Some("dateTime") => parse_date_time(&l.lexical, false).map_or(Val::Invalid, Val::DateTime),
        Some("date") => parse_date_time(&l.lexical, true).map_or(Val::Invalid, Val::Date),
        _ => Val::Other,
    }
}

// ---- term constructors ------------------------------------------------------

fn typed(lex: impl Into<String>, local: &str) -> Term {
    Term::Literal(Literal::typed(lex, xsd(local)))
}

fn bool_term(b: bool) -> Term {
    Term::Literal(Literal::boolean(b))
}

fn int_term(i: i128) -> Term {
    typed(i.to_string(), "integer")
}

fn str_term(s: impl Into<String>) -> Term {
    Term::string(s)
}

/// A string literal with the language tag of `lang`, or a simple literal.
fn str_like(s: impl Into<String>, lang: Option<&str>) -> Term {
    match lang {
        Some(tag) => Term::Literal(Literal::lang(s, tag)),
        None => Term::string(s),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ---- tests on values --------------------------------------------------------

/// Effective boolean value (§17.2.2).
pub fn ebv(t: &Term) -> FResult<bool> {
    let Term::Literal(l) = t else {
        return err(format!("no effective boolean value for {t}"));
    };
    // SPARQL 1.2 (W3C test `not-not`): only simple strings, numbers and
    // valid booleans have an EBV; language-tagged strings and ill-formed
    // numbers or booleans are errors.
    match value(l) {
        Val::Bool(b) => Ok(b),
        Val::Num(n) => Ok(!n.is_zero_or_nan()),
        Val::Str(s) => Ok(!s.is_empty()),
        _ => err(format!("no effective boolean value for {t}")),
    }
}

/// `sameTerm`.
pub fn same_term(a: &Term, b: &Term) -> bool {
    a == b
}

/// The `=` operator (§17.3 operator mapping and RDFterm-equal). Two triple
/// terms are equal when their components are (SPARQL 1.2 §17.4.1.7).
pub fn equal(a: &Term, b: &Term) -> FResult<bool> {
    if let (Term::Triple(x), Term::Triple(y)) = (a, b) {
        return Ok(equal(&x.subject, &y.subject)?
            && equal(&x.predicate, &y.predicate)?
            && equal(&x.object, &y.object)?);
    }
    let (Term::Literal(x), Term::Literal(y)) = (a, b) else {
        return Ok(a == b);
    };
    match (value(x), value(y)) {
        (Val::Num(p), Val::Num(q)) => Ok(num_cmp(p, q) == Some(Ordering::Equal)),
        (Val::Str(p), Val::Str(q)) => Ok(p == q),
        (Val::Lang(p, l1), Val::Lang(q, l2)) => Ok(p == q && l1.eq_ignore_ascii_case(l2)),
        (Val::Bool(p), Val::Bool(q)) => Ok(p == q),
        (Val::DateTime(p), Val::DateTime(q)) | (Val::Date(p), Val::Date(q)) => {
            match dt_cmp(&p, &q) {
                Some(o) => Ok(o == Ordering::Equal),
                None => err("indeterminate dateTime comparison"),
            }
        }
        (Val::Other | Val::Invalid, _) | (_, Val::Other | Val::Invalid) => {
            if x == y {
                Ok(true)
            } else {
                err(format!("cannot compare {a} and {b}"))
            }
        }
        _ => Ok(false),
    }
}

/// The `<` / `>` operators.
pub fn compare(a: &Term, b: &Term) -> FResult<Ordering> {
    let fail = || err(format!("cannot compare {a} and {b}"));
    let (Term::Literal(x), Term::Literal(y)) = (a, b) else {
        return fail();
    };
    match (value(x), value(y)) {
        (Val::Num(p), Val::Num(q)) => num_cmp(p, q).map_or_else(fail, Ok),
        (Val::Str(p), Val::Str(q)) => Ok(p.cmp(q)),
        (Val::Lang(p, l1), Val::Lang(q, l2)) if l1.eq_ignore_ascii_case(l2) => Ok(p.cmp(q)),
        (Val::Bool(p), Val::Bool(q)) => Ok(p.cmp(&q)),
        (Val::DateTime(p), Val::DateTime(q)) | (Val::Date(p), Val::Date(q)) => {
            dt_cmp(&p, &q).map_or_else(fail, Ok)
        }
        _ => fail(),
    }
}

/// Literal group for `ORDER BY`.
fn order_group(v: &Val<'_>) -> u8 {
    match v {
        Val::Num(_) => 0,
        Val::Str(_) => 1,
        Val::Lang(..) => 2,
        Val::Bool(_) => 3,
        Val::DateTime(_) => 4,
        Val::Date(_) => 5,
        Val::Invalid | Val::Other => 6,
    }
}

/// The total `ORDER BY` order (§15.1): unbound < blank < IRI < literal <
/// triple term; literals group by kind (numeric, string, language string,
/// boolean, dateTime, date, other) and order by value inside a group, with
/// the lexical form, datatype and language as tiebreaks; triple terms order
/// by subject, then predicate, then object.
pub fn order_cmp(a: Option<&Term>, b: Option<&Term>) -> Ordering {
    fn kind(t: Option<&Term>) -> u8 {
        match t {
            None => 0,
            Some(Term::Blank(_)) => 1,
            Some(Term::Iri(_)) => 2,
            Some(Term::Literal(_)) => 3,
            Some(Term::Triple(_)) => 4,
        }
    }
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        _ => return kind(a).cmp(&kind(b)),
    };
    match (a, b) {
        (Term::Blank(x), Term::Blank(y)) | (Term::Iri(x), Term::Iri(y)) => x.cmp(y),
        (Term::Literal(x), Term::Literal(y)) => {
            let (vx, vy) = (value(x), value(y));
            let by_value = order_group(&vx)
                .cmp(&order_group(&vy))
                .then_with(|| match (vx, vy) {
                    (Val::Num(p), Val::Num(q)) => match (p.is_nan(), q.is_nan()) {
                        (true, true) => Ordering::Equal,
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        _ => num_cmp(p, q).unwrap_or(Ordering::Equal),
                    },
                    (Val::Str(p), Val::Str(q)) => p.cmp(q),
                    (Val::Lang(p, l1), Val::Lang(q, l2)) => l1
                        .to_ascii_lowercase()
                        .cmp(&l2.to_ascii_lowercase())
                        .then_with(|| p.cmp(q)),
                    (Val::Bool(p), Val::Bool(q)) => p.cmp(&q),
                    (Val::DateTime(p), Val::DateTime(q)) | (Val::Date(p), Val::Date(q)) => {
                        p.instant().cmp_dec(q.instant())
                    }
                    _ => x.datatype.cmp(&y.datatype),
                });
            by_value
                .then_with(|| x.lexical.cmp(&y.lexical))
                .then_with(|| x.datatype.cmp(&y.datatype))
                .then_with(|| x.lang.cmp(&y.lang))
        }
        (Term::Triple(x), Term::Triple(y)) => order_cmp(Some(&x.subject), Some(&y.subject))
            .then_with(|| order_cmp(Some(&x.predicate), Some(&y.predicate)))
            .then_with(|| order_cmp(Some(&x.object), Some(&y.object))),
        _ => kind(Some(a)).cmp(&kind(Some(b))),
    }
}

// ---- arithmetic -------------------------------------------------------------

#[derive(Clone, Copy)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

fn arith(a: &Term, b: &Term, op: Op) -> FResult<Term> {
    let (x, y) = Num::promote(numeric(a)?, numeric(b)?);
    let overflow = || ExprError("numeric overflow".into());
    let n = match (x, y) {
        (Num::Int(p), Num::Int(q)) => match op {
            Op::Add => Num::Int(p.checked_add(q).ok_or_else(overflow)?),
            Op::Sub => Num::Int(p.checked_sub(q).ok_or_else(overflow)?),
            Op::Mul => Num::Int(p.checked_mul(q).ok_or_else(overflow)?),
            Op::Div => {
                if q == 0 {
                    return err("division by zero");
                }
                Num::Dec(Dec::int(p).div(Dec::int(q)).ok_or_else(overflow)?)
            }
        },
        (Num::Dec(p), Num::Dec(q)) => Num::Dec(
            match op {
                Op::Add => p.add(q),
                Op::Sub => p.sub(q),
                Op::Mul => p.mul(q),
                Op::Div => {
                    if q.is_zero() {
                        return err("division by zero");
                    }
                    p.div(q)
                }
            }
            .ok_or_else(overflow)?,
        ),
        (Num::Float(p), Num::Float(q)) => Num::Float(match op {
            Op::Add => p + q,
            Op::Sub => p - q,
            Op::Mul => p * q,
            Op::Div => p / q,
        }),
        (p, q) => {
            let (p, q) = (p.to_f64(), q.to_f64());
            Num::Double(match op {
                Op::Add => p + q,
                Op::Sub => p - q,
                Op::Mul => p * q,
                Op::Div => p / q,
            })
        }
    };
    Ok(n.into_term())
}

pub fn add(a: &Term, b: &Term) -> FResult<Term> {
    arith(a, b, Op::Add)
}

pub fn sub(a: &Term, b: &Term) -> FResult<Term> {
    arith(a, b, Op::Sub)
}

pub fn mul(a: &Term, b: &Term) -> FResult<Term> {
    arith(a, b, Op::Mul)
}

pub fn div(a: &Term, b: &Term) -> FResult<Term> {
    arith(a, b, Op::Div)
}

/// Unary `-`.
pub fn neg(a: &Term) -> FResult<Term> {
    Ok(match numeric(a)? {
        Num::Int(i) => Num::Int(
            i.checked_neg()
                .ok_or(ExprError("numeric overflow".into()))?,
        ),
        Num::Dec(d) => Num::Dec(d.neg().ok_or(ExprError("numeric overflow".into()))?),
        Num::Float(f) => Num::Float(-f),
        Num::Double(d) => Num::Double(-d),
    }
    .into_term())
}

/// Unary `+`.
pub fn pos(a: &Term) -> FResult<Term> {
    Ok(match numeric(a)? {
        Num::Int(i) => Num::Int(i),
        n => n,
    }
    .into_term())
}

/// `!`.
pub fn not(a: &Term) -> FResult<Term> {
    Ok(bool_term(!ebv(a)?))
}

// ---- casts ------------------------------------------------------------------

/// The XPath `xs:string` cast of a number (XPath F&O §19.1.2): decimals
/// without trailing zeros, floats and doubles in `[1e-6, 1e6)` without an
/// exponent. `None` keeps the canonical lexical form.
fn xpath_string(n: Num) -> Option<String> {
    let plain = |s: String| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    };
    match n {
        Num::Int(_) => None,
        Num::Dec(_) => match n.into_term() {
            Term::Literal(c) => Some(plain(c.lexical)),
            _ => None,
        },
        Num::Float(f) if f == 0.0 || (1e-6..1e6).contains(&f.abs()) => Some(plain(format!("{f}"))),
        Num::Double(f) if f == 0.0 || (1e-6..1e6).contains(&f.abs()) => Some(plain(format!("{f}"))),
        _ => None,
    }
}

/// XSD constructor casts (§17.5).
pub fn cast(datatype_iri: &str, a: &Term) -> FResult<Term> {
    let fail = || err(format!("cannot cast {a} to <{datatype_iri}>"));
    let Some(target) = xsd_local(datatype_iri) else {
        return fail();
    };
    let l = match a {
        Term::Iri(i) if target == "string" => return Ok(str_term(i.clone())),
        Term::Literal(l) => l,
        _ => return fail(),
    };
    let v = value(l);
    // A string source is read with whitespace trimmed, as XPath casting does.
    let src_str = match v {
        Val::Str(s) => Some(s.trim()),
        _ => None,
    };
    if matches!(v, Val::Invalid | Val::Lang(..)) {
        return fail();
    }
    if let Some(bounds) = integer_bounds(target) {
        let i = match (v, src_str) {
            (_, Some(s)) => parse_integer(s),
            (Val::Num(Num::Int(i)), _) => Some(i),
            (Val::Num(Num::Dec(d)), _) => Some(d.trunc()),
            (Val::Num(n @ (Num::Float(_) | Num::Double(_))), _) => {
                let f = n.to_f64().trunc();
                if f.is_finite() && f.abs() < 1e38 {
                    Some(f as i128)
                } else {
                    None
                }
            }
            (Val::Bool(b), _) => Some(i128::from(b)),
            _ => None,
        };
        return match i {
            Some(i) if in_bounds(i, bounds) => Ok(typed(i.to_string(), target)),
            _ => fail(),
        };
    }
    match target {
        "string" => Ok(str_term(match v {
            Val::Num(n) => xpath_string(n).unwrap_or_else(|| match n.into_term() {
                Term::Literal(c) => c.lexical,
                _ => l.lexical.clone(),
            }),
            Val::Bool(b) => b.to_string(),
            _ => l.lexical.clone(),
        })),
        "decimal" => {
            let d = match (v, src_str) {
                (_, Some(s)) => Dec::parse(s),
                (Val::Num(Num::Int(i)), _) => Some(Dec::int(i)),
                (Val::Num(Num::Dec(d)), _) => Some(d),
                (Val::Num(n), _) => Dec::from_f64(n.to_f64()),
                (Val::Bool(b), _) => Some(Dec::int(i128::from(b))),
                _ => None,
            };
            d.map_or_else(fail, |d| Ok(Num::Dec(d).into_term()))
        }
        "float" | "double" => {
            let f = match (v, src_str) {
                (_, Some(s)) => parse_xsd_double(s),
                (Val::Num(n), _) => Some(n.to_f64()),
                (Val::Bool(b), _) => Some(if b { 1.0 } else { 0.0 }),
                _ => None,
            };
            match f {
                Some(f) if target == "float" => {
                    let f32v = match src_str {
                        Some(s) if f.is_finite() => s.parse::<f32>().unwrap_or(f as f32),
                        _ => match v {
                            Val::Num(Num::Float(x)) => x,
                            _ => f as f32,
                        },
                    };
                    Ok(Num::Float(f32v).into_term())
                }
                Some(f) => Ok(Num::Double(f).into_term()),
                None => fail(),
            }
        }
        "boolean" => {
            let b = match (v, src_str) {
                (_, Some(s)) => parse_bool(s),
                (Val::Num(n), _) => Some(!n.is_zero_or_nan()),
                (Val::Bool(b), _) => Some(b),
                _ => None,
            };
            b.map_or_else(fail, |b| Ok(bool_term(b)))
        }
        "dateTime" => match (v, src_str) {
            (_, Some(s)) => match parse_date_time(s, false) {
                Some(_) => Ok(typed(s, "dateTime")),
                None => fail(),
            },
            (Val::DateTime(_), _) => Ok(typed(l.lexical.clone(), "dateTime")),
            (Val::Date(d), _) => Ok(typed(
                format!("{}T00:00:00{}", date_lexical(&d), tz_string(d.tz)),
                "dateTime",
            )),
            _ => fail(),
        },
        "date" => match (v, src_str) {
            (_, Some(s)) => match parse_date_time(s, true) {
                Some(_) => Ok(typed(s, "date")),
                None => fail(),
            },
            (Val::Date(_), _) => Ok(typed(l.lexical.clone(), "date")),
            (Val::DateTime(d), _) => Ok(typed(
                format!("{}{}", date_lexical(&d), tz_string(d.tz)),
                "date",
            )),
            _ => fail(),
        },
        _ => match (v, l.datatype == datatype_iri) {
            (_, true) => Ok(a.clone()),
            (Val::Str(s), _) => Ok(typed(s.trim(), target)),
            _ => fail(),
        },
    }
}

/// `YYYY-MM-DD` of a date/time value, without timezone.
fn date_lexical(d: &DateTimeV) -> String {
    let y = if d.year < 0 {
        format!("-{:04}", -d.year)
    } else {
        format!("{:04}", d.year)
    };
    format!("{y}-{:02}-{:02}", d.month, d.day)
}

// ---- argument helpers -------------------------------------------------------

/// A string literal (simple, `xsd:string` or language-tagged): lexical form
/// and language tag.
fn string_lit(t: &Term) -> FResult<(&str, Option<&str>)> {
    match t {
        Term::Literal(l) if l.datatype == xsd("string") => Ok((&l.lexical, None)),
        Term::Literal(l) if l.datatype == RDF_LANG_STRING && l.lang.is_some() => {
            Ok((&l.lexical, l.lang.as_deref()))
        }
        _ => err(format!("not a string literal: {t}")),
    }
}

/// A simple literal or `xsd:string`.
fn simple_lit(t: &Term) -> FResult<&str> {
    match t {
        Term::Literal(l) if l.datatype == xsd("string") => Ok(&l.lexical),
        _ => err(format!("not a simple literal: {t}")),
    }
}

/// Argument compatibility (§17.4.3.1.2).
fn compatible<'a>(a: &'a Term, b: &'a Term) -> FResult<(&'a str, Option<&'a str>, &'a str)> {
    let (x, la) = string_lit(a)?;
    let (y, lb) = string_lit(b)?;
    match (la, lb) {
        (_, None) => Ok((x, la, y)),
        (Some(p), Some(q)) if p.eq_ignore_ascii_case(q) => Ok((x, la, y)),
        _ => err(format!("incompatible string arguments {a} and {b}")),
    }
}

fn arity(f: &Function, args: &[Term], min: usize, max: usize) -> FResult<()> {
    if args.len() < min || args.len() > max {
        return err(format!(
            "{f} expects {} arguments, got {}",
            if min == max {
                min.to_string()
            } else {
                format!("{min}..{max}")
            },
            args.len()
        ));
    }
    Ok(())
}

fn int_arg(t: &Term) -> FResult<f64> {
    Ok(numeric(t)?.to_f64())
}

/// `fn:round` on a double: half toward positive infinity.
fn round_half_up(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

fn date_time_arg(t: &Term) -> FResult<DateTimeV> {
    if let Term::Literal(l) = t {
        match value(l) {
            Val::DateTime(d) | Val::Date(d) => return Ok(d),
            _ => {}
        }
    }
    err(format!("not an xsd:dateTime: {t}"))
}

fn build_regex(pattern: &str, flags: &str) -> FResult<regex::Regex> {
    let mut b_quote = false;
    let mut builder_flags = (false, false, false, false);
    for c in flags.chars() {
        match c {
            's' => builder_flags.0 = true,
            'm' => builder_flags.1 = true,
            'i' => builder_flags.2 = true,
            'x' => builder_flags.3 = true,
            'q' => b_quote = true,
            _ => return err(format!("invalid regex flag {c:?}")),
        }
    }
    let pat = if b_quote {
        regex::escape(pattern)
    } else if builder_flags.3 {
        // XPath `x`: whitespace in the pattern is removed (outside classes).
        let mut out = String::with_capacity(pattern.len());
        let mut in_class = false;
        let mut esc = false;
        for c in pattern.chars() {
            if esc {
                out.push(c);
                esc = false;
                continue;
            }
            match c {
                '\\' => {
                    esc = true;
                    out.push(c);
                }
                '[' => {
                    in_class = true;
                    out.push(c);
                }
                ']' => {
                    in_class = false;
                    out.push(c);
                }
                c if c.is_whitespace() && !in_class => {}
                c => out.push(c),
            }
        }
        out
    } else {
        pattern.to_string()
    };
    regex::RegexBuilder::new(&pat)
        .dot_matches_new_line(builder_flags.0)
        .multi_line(builder_flags.1)
        .case_insensitive(builder_flags.2)
        .build()
        .map_err(|e| ExprError(format!("invalid regex {pattern:?}: {e}")))
}

/// Translates an XPath `fn:replace` replacement string into the `regex`
/// crate's syntax.
fn translate_replacement(rep: &str, groups: usize) -> FResult<String> {
    let mut out = String::with_capacity(rep.len());
    let chars: Vec<char> = rep.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => match chars.get(i + 1) {
                Some('\\') => {
                    out.push('\\');
                    i += 2;
                }
                Some('$') => {
                    out.push_str("$$");
                    i += 2;
                }
                _ => return err("invalid replacement string"),
            },
            '$' => {
                let mut j = i + 1;
                let mut n: usize = match chars.get(j).and_then(|c| c.to_digit(10)) {
                    Some(d) => d as usize,
                    None => return err("invalid replacement string"),
                };
                j += 1;
                while let Some(d) = chars.get(j).and_then(|c| c.to_digit(10)) {
                    let m = n * 10 + d as usize;
                    if m >= groups {
                        break;
                    }
                    n = m;
                    j += 1;
                }
                if n < groups {
                    out.push_str(&format!("${{{n}}}"));
                }
                i = j;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn encode_for_uri(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn lang_matches(tag: &str, range: &str) -> bool {
    if range == "*" {
        return !tag.is_empty();
    }
    let (tag, range) = (tag.to_ascii_lowercase(), range.to_ascii_lowercase());
    tag == range || (tag.starts_with(&range) && tag.as_bytes().get(range.len()) == Some(&b'-'))
}

fn valid_lang_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .split('-')
            .all(|p| !p.is_empty() && p.len() <= 8 && p.bytes().all(|b| b.is_ascii_alphanumeric()))
        && tag.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
}

fn timezone_duration(tz: i32) -> String {
    if tz == 0 {
        return "PT0S".into();
    }
    let sign = if tz < 0 { "-" } else { "" };
    let (h, m) = (tz.abs() / 60, tz.abs() % 60);
    let mut s = format!("{sign}PT");
    if h != 0 {
        s.push_str(&format!("{h}H"));
    }
    if m != 0 {
        s.push_str(&format!("{m}M"));
    }
    s
}

fn tz_string(tz: Option<i32>) -> String {
    match tz {
        None => String::new(),
        Some(0) => "Z".into(),
        Some(t) => format!(
            "{}{:02}:{:02}",
            if t < 0 { '-' } else { '+' },
            t.abs() / 60,
            t.abs() % 60
        ),
    }
}

// ---- IRI resolution (RFC 3986 §5.2) -----------------------------------------

struct IriParts<'a> {
    scheme: Option<&'a str>,
    authority: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

fn split_iri(s: &str) -> IriParts<'_> {
    let (rest, fragment) = match s.find('#') {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (rest, query) = match rest.find('?') {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let (scheme, rest) = match rest.find(':') {
        Some(i)
            if i > 0
                && rest[..i]
                    .bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_alphabetic())
                && rest[..i]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
                && !rest[..i].contains('/') =>
        {
            (Some(&rest[..i]), &rest[i + 1..])
        }
        _ => (None, rest),
    };
    let (authority, path) = match rest.strip_prefix("//") {
        Some(r) => match r.find('/') {
            Some(i) => (Some(&r[..i]), &r[i..]),
            None => (Some(r), ""),
        },
        None => (None, rest),
    };
    IriParts {
        scheme,
        authority,
        path,
        query,
        fragment,
    }
}

fn remove_dot_segments(path: &str) -> String {
    fn pop_last(out: &mut String) {
        match out.rfind('/') {
            Some(i) => out.truncate(i),
            None => out.clear(),
        }
    }
    let mut out = String::new();
    let mut inp: &str = path;
    while !inp.is_empty() {
        if let Some(r) = inp.strip_prefix("../") {
            inp = r;
        } else if let Some(r) = inp.strip_prefix("./") {
            inp = r;
        } else if inp.starts_with("/./") {
            inp = &inp[2..];
        } else if inp == "/." {
            inp = "/";
        } else if inp.starts_with("/../") {
            inp = &inp[3..];
            pop_last(&mut out);
        } else if inp == "/.." {
            inp = "/";
            pop_last(&mut out);
        } else if inp == "." || inp == ".." {
            inp = "";
        } else {
            let start = usize::from(inp.starts_with('/'));
            let end = inp[start..].find('/').map_or(inp.len(), |i| i + start);
            out.push_str(&inp[..end]);
            inp = &inp[end..];
        }
    }
    out
}

fn resolve_iri(base: &str, rel: &str) -> String {
    let r = split_iri(rel);
    let b = split_iri(base);
    let (scheme, authority, path, query);
    if r.scheme.is_some() {
        scheme = r.scheme;
        authority = r.authority;
        path = remove_dot_segments(r.path);
        query = r.query;
    } else {
        scheme = b.scheme;
        if r.authority.is_some() {
            authority = r.authority;
            path = remove_dot_segments(r.path);
            query = r.query;
        } else {
            authority = b.authority;
            if r.path.is_empty() {
                path = b.path.to_string();
                query = r.query.or(b.query);
            } else {
                path = if r.path.starts_with('/') {
                    remove_dot_segments(r.path)
                } else {
                    let merged = if b.authority.is_some() && b.path.is_empty() {
                        format!("/{}", r.path)
                    } else {
                        match b.path.rfind('/') {
                            Some(i) => format!("{}{}", &b.path[..=i], r.path),
                            None => r.path.to_string(),
                        }
                    };
                    remove_dot_segments(&merged)
                };
                query = r.query;
            }
        }
    }
    let mut out = String::new();
    if let Some(s) = scheme {
        out.push_str(s);
        out.push(':');
    }
    if let Some(a) = authority {
        out.push_str("//");
        out.push_str(a);
    }
    out.push_str(&path);
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = r.fragment {
        out.push('#');
        out.push_str(f);
    }
    out
}

// ---- function dispatch ------------------------------------------------------

fn digest<D: sha2::Digest>(s: &str) -> Term {
    str_term(hex(&D::digest(s.as_bytes())))
}

/// Calls a SPARQL built-in function on evaluated arguments.
pub fn call(f: &Function, args: &[Term], ctx: &mut FnContext) -> FResult<Term> {
    use Function as F;
    match f {
        F::Str => {
            arity(f, args, 1, 1)?;
            match &args[0] {
                Term::Iri(i) => Ok(str_term(i.clone())),
                Term::Literal(l) => Ok(str_term(l.lexical.clone())),
                t => err(format!("STR of a blank node {t}")),
            }
        }
        F::Lang => {
            arity(f, args, 1, 1)?;
            match &args[0] {
                Term::Literal(l) => Ok(str_term(l.lang.clone().unwrap_or_default())),
                t => err(format!("LANG of a non-literal {t}")),
            }
        }
        F::LangMatches => {
            arity(f, args, 2, 2)?;
            let tag = simple_lit(&args[0])?;
            let range = simple_lit(&args[1])?;
            Ok(bool_term(lang_matches(tag, range)))
        }
        F::Datatype => {
            arity(f, args, 1, 1)?;
            match &args[0] {
                Term::Literal(l) => Ok(Term::Iri(l.datatype.clone())),
                t => err(format!("DATATYPE of a non-literal {t}")),
            }
        }
        F::Iri => {
            arity(f, args, 1, 1)?;
            let s = match &args[0] {
                Term::Iri(i) => return Ok(Term::Iri(i.clone())),
                t => simple_lit(t)?,
            };
            if s.chars().any(|c| {
                c.is_whitespace()
                    || matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\')
            }) {
                return err(format!("invalid IRI {s:?}"));
            }
            Ok(Term::Iri(match &ctx.base_iri {
                Some(base) => resolve_iri(base, s),
                None => s.to_string(),
            }))
        }
        F::BNode => {
            arity(f, args, 0, 1)?;
            match args.first() {
                None => Ok(ctx.fresh_bnode()),
                Some(t) => {
                    let s = simple_lit(t)?.to_string();
                    let key = (ctx.row.clone(), s);
                    if let Some(label) = ctx.row_bnodes.get(&key) {
                        return Ok(Term::Blank(label.clone()));
                    }
                    let Term::Blank(label) = ctx.fresh_bnode() else {
                        unreachable!()
                    };
                    ctx.row_bnodes.insert(key, label.clone());
                    Ok(Term::Blank(label))
                }
            }
        }
        F::Rand => {
            arity(f, args, 0, 0)?;
            let x = (ctx.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
            Ok(Num::Double(x).into_term())
        }
        F::Abs | F::Ceil | F::Floor | F::Round => {
            arity(f, args, 1, 1)?;
            let n = numeric(&args[0])?;
            let overflow = || ExprError("numeric overflow".into());
            let r = match (f, n) {
                (F::Abs, Num::Int(i)) => Num::Int(i.checked_abs().ok_or_else(overflow)?),
                (_, Num::Int(i)) => Num::Int(i),
                (F::Abs, Num::Dec(d)) => {
                    Num::Dec(Dec::new(d.m.checked_abs().ok_or_else(overflow)?, d.s))
                }
                (F::Ceil, Num::Dec(d)) => Num::Dec(d.ceil()),
                (F::Floor, Num::Dec(d)) => Num::Dec(d.floor()),
                (_, Num::Dec(d)) => Num::Dec(d.round().ok_or_else(overflow)?),
                (F::Abs, Num::Float(x)) => Num::Float(x.abs()),
                (F::Ceil, Num::Float(x)) => Num::Float(x.ceil()),
                (F::Floor, Num::Float(x)) => Num::Float(x.floor()),
                (_, Num::Float(x)) => Num::Float(round_half_up(f64::from(x)) as f32),
                (F::Abs, Num::Double(x)) => Num::Double(x.abs()),
                (F::Ceil, Num::Double(x)) => Num::Double(x.ceil()),
                (F::Floor, Num::Double(x)) => Num::Double(x.floor()),
                (_, Num::Double(x)) => Num::Double(round_half_up(x)),
            };
            Ok(r.into_term())
        }
        F::Concat => {
            let mut out = String::new();
            let mut lang: Option<Option<&str>> = None;
            for a in args {
                let (s, l) = string_lit(a)?;
                out.push_str(s);
                lang = match lang {
                    None => Some(l),
                    Some(Some(p)) if l.is_some_and(|q| q.eq_ignore_ascii_case(p)) => Some(Some(p)),
                    Some(_) => Some(None),
                };
            }
            Ok(str_like(out, lang.flatten()))
        }
        F::SubStr => {
            arity(f, args, 2, 3)?;
            let (s, lang) = string_lit(&args[0])?;
            let start = round_half_up(int_arg(&args[1])?);
            let end = match args.get(2) {
                Some(l) => start + round_half_up(int_arg(l)?),
                None => f64::INFINITY,
            };
            let out: String = s
                .chars()
                .enumerate()
                .filter(|(i, _)| {
                    let p = (*i + 1) as f64;
                    p >= start && p < end
                })
                .map(|(_, c)| c)
                .collect();
            Ok(str_like(out, lang))
        }
        F::StrLen => {
            arity(f, args, 1, 1)?;
            let (s, _) = string_lit(&args[0])?;
            Ok(int_term(s.chars().count() as i128))
        }
        F::Replace => {
            arity(f, args, 3, 4)?;
            let (s, lang) = string_lit(&args[0])?;
            let pattern = simple_lit(&args[1])?;
            let rep = simple_lit(&args[2])?;
            let flags = match args.get(3) {
                Some(t) => simple_lit(t)?,
                None => "",
            };
            let re = ctx.regex(pattern, flags)?;
            let rep = if flags.contains('q') {
                rep.replace('$', "$$")
            } else {
                translate_replacement(rep, re.captures_len())?
            };
            Ok(str_like(re.replace_all(s, rep.as_str()).into_owned(), lang))
        }
        F::UCase | F::LCase => {
            arity(f, args, 1, 1)?;
            let (s, lang) = string_lit(&args[0])?;
            let out = if matches!(f, F::UCase) {
                s.to_uppercase()
            } else {
                s.to_lowercase()
            };
            Ok(str_like(out, lang))
        }
        F::EncodeForUri => {
            arity(f, args, 1, 1)?;
            let (s, _) = string_lit(&args[0])?;
            Ok(str_term(encode_for_uri(s)))
        }
        F::Contains | F::StrStarts | F::StrEnds => {
            arity(f, args, 2, 2)?;
            let (a, _, b) = compatible(&args[0], &args[1])?;
            Ok(bool_term(match f {
                F::Contains => a.contains(b),
                F::StrStarts => a.starts_with(b),
                _ => a.ends_with(b),
            }))
        }
        F::StrBefore | F::StrAfter => {
            arity(f, args, 2, 2)?;
            let (a, lang, b) = compatible(&args[0], &args[1])?;
            Ok(match a.find(b) {
                None => str_term(""),
                Some(i) if matches!(f, F::StrBefore) => str_like(&a[..i], lang),
                Some(i) => str_like(&a[i + b.len()..], lang),
            })
        }
        F::Year | F::Month | F::Day | F::Hours | F::Minutes => {
            arity(f, args, 1, 1)?;
            let d = date_time_arg(&args[0])?;
            Ok(int_term(match f {
                F::Year => i128::from(d.year),
                F::Month => d.month.into(),
                F::Day => d.day.into(),
                F::Hours => d.hour.into(),
                _ => d.minute.into(),
            }))
        }
        F::Seconds => {
            arity(f, args, 1, 1)?;
            Ok(Num::Dec(date_time_arg(&args[0])?.second).into_term())
        }
        F::Timezone => {
            arity(f, args, 1, 1)?;
            match date_time_arg(&args[0])?.tz {
                Some(tz) => Ok(typed(timezone_duration(tz), "dayTimeDuration")),
                None => err("TIMEZONE of a value without timezone"),
            }
        }
        F::Tz => {
            arity(f, args, 1, 1)?;
            Ok(str_term(tz_string(date_time_arg(&args[0])?.tz)))
        }
        F::Now => {
            arity(f, args, 0, 0)?;
            Ok(ctx.now.clone())
        }
        F::Uuid => {
            arity(f, args, 0, 0)?;
            Ok(Term::Iri(format!("urn:uuid:{}", ctx.uuid())))
        }
        F::StrUuid => {
            arity(f, args, 0, 0)?;
            Ok(str_term(ctx.uuid()))
        }
        F::Md5 | F::Sha1 | F::Sha256 | F::Sha384 | F::Sha512 => {
            arity(f, args, 1, 1)?;
            let s = simple_lit(&args[0])?;
            Ok(match f {
                F::Md5 => digest::<md5::Md5>(s),
                F::Sha1 => digest::<sha1::Sha1>(s),
                F::Sha256 => digest::<sha2::Sha256>(s),
                F::Sha384 => digest::<sha2::Sha384>(s),
                _ => digest::<sha2::Sha512>(s),
            })
        }
        F::StrLang => {
            arity(f, args, 2, 2)?;
            let s = simple_lit(&args[0])?;
            let tag = simple_lit(&args[1])?;
            if !valid_lang_tag(tag) {
                return err(format!("invalid language tag {tag:?}"));
            }
            Ok(Term::Literal(Literal::lang(s, tag)))
        }
        F::StrDt => {
            arity(f, args, 2, 2)?;
            let s = simple_lit(&args[0])?;
            match &args[1] {
                Term::Iri(dt) if dt == RDF_LANG_STRING => {
                    err("STRDT cannot build an rdf:langString")
                }
                Term::Iri(dt) => Ok(Term::Literal(Literal::typed(s, dt.clone()))),
                t => err(format!("STRDT datatype is not an IRI: {t}")),
            }
        }
        F::IsIri => {
            arity(f, args, 1, 1)?;
            Ok(bool_term(matches!(args[0], Term::Iri(_))))
        }
        F::IsBlank => {
            arity(f, args, 1, 1)?;
            Ok(bool_term(matches!(args[0], Term::Blank(_))))
        }
        F::IsLiteral => {
            arity(f, args, 1, 1)?;
            Ok(bool_term(matches!(args[0], Term::Literal(_))))
        }
        F::IsNumeric => {
            arity(f, args, 1, 1)?;
            Ok(bool_term(is_numeric(&args[0])))
        }
        F::Regex => {
            arity(f, args, 2, 3)?;
            let (s, _) = string_lit(&args[0])?;
            let pattern = simple_lit(&args[1])?;
            let flags = match args.get(2) {
                Some(t) => simple_lit(t)?,
                None => "",
            };
            Ok(bool_term(ctx.regex(pattern, flags)?.is_match(s)))
        }
        F::Triple => {
            arity(f, args, 3, 3)?;
            if !matches!(args[0], Term::Iri(_) | Term::Blank(_)) {
                return err(format!(
                    "TRIPLE subject is not an IRI or blank node: {}",
                    args[0]
                ));
            }
            if !matches!(args[1], Term::Iri(_)) {
                return err(format!("TRIPLE predicate is not an IRI: {}", args[1]));
            }
            Ok(Term::triple(
                args[0].clone(),
                args[1].clone(),
                args[2].clone(),
            ))
        }
        F::Subject | F::Predicate | F::Object => {
            arity(f, args, 1, 1)?;
            let Term::Triple(t) = &args[0] else {
                return err(format!(
                    "{f} of a term that is not a triple term: {}",
                    args[0]
                ));
            };
            Ok(match f {
                F::Subject => t.subject.clone(),
                F::Predicate => t.predicate.clone(),
                _ => t.object.clone(),
            })
        }
        F::IsTriple => {
            arity(f, args, 1, 1)?;
            Ok(bool_term(matches!(args[0], Term::Triple(_))))
        }
        F::LangDir => {
            arity(f, args, 1, 1)?;
            match &args[0] {
                Term::Literal(_) => Ok(str_term("")),
                t => err(format!("LANGDIR of a non-literal {t}")),
            }
        }
        F::HasLang => {
            arity(f, args, 1, 1)?;
            // false for any term without a language tag, IRIs included
            Ok(bool_term(match &args[0] {
                Term::Literal(l) => l.lang.as_deref().is_some_and(|t| !t.is_empty()),
                _ => false,
            }))
        }
        F::HasLangDir => {
            arity(f, args, 1, 1)?;
            Ok(bool_term(false))
        }
        F::StrLangDir => err("base direction is not supported"),
        F::Custom(name) if name.as_str().starts_with(XSD) => {
            arity(f, args, 1, 1)?;
            cast(name.as_str(), &args[0])
        }
        F::Custom(name) => err(format!("unsupported function {name}")),
        // Functions behind optional spargebra features (e.g. `ADJUST` of
        // `sep-0002`, switched on when an application also embeds Oxigraph).
        #[allow(unreachable_patterns)]
        other => err(format!("unsupported function {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(i: i64) -> Term {
        Term::integer(i)
    }
    fn dec(s: &str) -> Term {
        Term::Literal(Literal::decimal(s))
    }
    fn dbl(s: &str) -> Term {
        typed(s, "double")
    }
    fn flt(s: &str) -> Term {
        typed(s, "float")
    }
    fn s(x: &str) -> Term {
        Term::string(x)
    }
    fn lang(x: &str, t: &str) -> Term {
        Term::Literal(Literal::lang(x, t))
    }
    fn dt(x: &str) -> Term {
        Term::Literal(Literal::date_time(x))
    }
    fn b(v: bool) -> Term {
        bool_term(v)
    }
    fn iri(x: &str) -> Term {
        Term::iri(x)
    }
    fn other(x: &str) -> Term {
        Term::Literal(Literal::typed(x, "http://example.org/dt"))
    }
    fn lex(t: &Term) -> String {
        t.as_literal().unwrap().lexical.clone()
    }
    fn dt_of(t: &Term) -> String {
        t.as_literal().unwrap().datatype.clone()
    }
    fn ctx() -> FnContext {
        FnContext::new("2026-11-24T10:00:00Z")
    }
    fn c(f: Function, args: &[Term]) -> FResult<Term> {
        call(&f, args, &mut ctx())
    }
    fn cs(f: Function, args: &[Term]) -> String {
        lex(&c(f, args).unwrap())
    }

    // ---- EBV ----

    #[test]
    fn ebv_table() {
        assert_eq!(ebv(&b(true)), Ok(true));
        assert_eq!(ebv(&b(false)), Ok(false));
        assert_eq!(ebv(&typed("1", "boolean")), Ok(true));
        // SPARQL 1.2: ill-formed booleans and language strings have no EBV
        assert!(ebv(&typed("maybe", "boolean")).is_err());
        assert_eq!(ebv(&s("")), Ok(false));
        assert_eq!(ebv(&s("a")), Ok(true));
        assert!(ebv(&lang("", "en")).is_err());
        assert!(ebv(&lang("x", "en")).is_err());
        assert_eq!(ebv(&int(0)), Ok(false));
        assert_eq!(ebv(&int(3)), Ok(true));
        assert_eq!(ebv(&dec("0.0")), Ok(false));
        assert_eq!(ebv(&dbl("NaN")), Ok(false));
        assert_eq!(ebv(&dbl("0.0E0")), Ok(false));
        assert_eq!(ebv(&dbl("-INF")), Ok(true));
        assert_eq!(ebv(&flt("1.5")), Ok(true));
        assert!(ebv(&typed("abc", "integer")).is_err());
        assert!(ebv(&iri("http://x")).is_err());
        assert!(ebv(&Term::Blank("b".into())).is_err());
        assert!(ebv(&dt("2020-01-01T00:00:00Z")).is_err());
        assert!(ebv(&other("x")).is_err());
    }

    #[test]
    fn not_operator() {
        assert_eq!(not(&s("")).unwrap(), b(true));
        assert_eq!(not(&int(2)).unwrap(), b(false));
        assert!(not(&iri("http://x")).is_err());
    }

    // ---- = and < ----

    #[test]
    fn equality() {
        assert_eq!(equal(&int(1), &dec("1.0")), Ok(true));
        assert_eq!(equal(&int(1), &dbl("1.0E0")), Ok(true));
        assert_eq!(equal(&typed("01", "integer"), &int(1)), Ok(true));
        assert_eq!(equal(&typed("1", "byte"), &int(1)), Ok(true));
        assert_eq!(equal(&dbl("NaN"), &dbl("NaN")), Ok(false));
        assert_eq!(equal(&s("a"), &s("a")), Ok(true));
        assert_eq!(equal(&s("a"), &s("b")), Ok(false));
        assert_eq!(equal(&lang("a", "en"), &lang("a", "EN")), Ok(true));
        assert_eq!(equal(&lang("a", "en"), &lang("a", "fr")), Ok(false));
        assert_eq!(equal(&lang("a", "en"), &s("a")), Ok(false));
        assert_eq!(equal(&s("1"), &int(1)), Ok(false));
        assert_eq!(equal(&b(true), &typed("1", "boolean")), Ok(true));
        assert_eq!(
            equal(
                &dt("2002-04-02T12:00:00-01:00"),
                &dt("2002-04-02T17:00:00+04:00")
            ),
            Ok(true)
        );
        assert_eq!(
            equal(&dt("2005-04-04T24:00:00Z"), &dt("2005-04-05T00:00:00Z")),
            Ok(true)
        );
        assert!(equal(&dt("2002-04-02T12:00:00"), &dt("2002-04-02T12:00:00Z")).is_err());
        // Unknown datatypes: same term or error.
        assert_eq!(equal(&other("a"), &other("a")), Ok(true));
        assert!(equal(&other("a"), &other("b")).is_err());
        assert!(equal(&other("a"), &s("a")).is_err());
        assert!(equal(&typed("x", "integer"), &int(1)).is_err());
        assert_eq!(equal(&iri("http://a"), &iri("http://a")), Ok(true));
        assert_eq!(equal(&iri("http://a"), &s("http://a")), Ok(false));
        assert_eq!(
            equal(&Term::Blank("x".into()), &Term::Blank("x".into())),
            Ok(true)
        );
    }

    #[test]
    fn same_term_is_syntactic() {
        assert!(same_term(&int(1), &int(1)));
        assert!(!same_term(&int(1), &dec("1.0")));
        assert!(!same_term(&typed("01", "integer"), &int(1)));
    }

    #[test]
    fn comparison() {
        use Ordering::*;
        assert_eq!(compare(&int(1), &dec("1.5")), Ok(Less));
        assert_eq!(compare(&dbl("2.0E0"), &int(1)), Ok(Greater));
        assert_eq!(compare(&flt("1.0"), &dec("1.0")), Ok(Equal));
        assert!(compare(&dbl("NaN"), &int(1)).is_err());
        assert_eq!(compare(&s("abc"), &s("abd")), Ok(Less));
        assert_eq!(compare(&lang("b", "en"), &lang("a", "en")), Ok(Greater));
        assert!(compare(&lang("b", "en"), &lang("a", "fr")).is_err());
        assert!(compare(&lang("b", "en"), &s("a")).is_err());
        assert_eq!(compare(&b(false), &b(true)), Ok(Less));
        assert_eq!(
            compare(&dt("2020-01-01T00:00:00Z"), &dt("2020-01-01T00:00:00.5Z")),
            Ok(Less)
        );
        assert_eq!(
            compare(&dt("2020-01-01T00:00:00Z"), &dt("2020-01-03T00:00:00")),
            Ok(Less)
        );
        assert!(compare(&dt("2020-01-01T00:00:00Z"), &dt("2020-01-01T05:00:00")).is_err());
        assert!(compare(&int(1), &s("1")).is_err());
        assert!(compare(&iri("http://a"), &iri("http://b")).is_err());
        assert!(compare(&other("a"), &other("b")).is_err());
    }

    #[test]
    fn order_by_total_order() {
        use Ordering::*;
        let blank = Term::Blank("a".into());
        let i = iri("http://a");
        assert_eq!(order_cmp(None, Some(&blank)), Less);
        assert_eq!(order_cmp(None, None), Equal);
        assert_eq!(order_cmp(Some(&blank), Some(&i)), Less);
        assert_eq!(order_cmp(Some(&i), Some(&s("a"))), Less);
        assert_eq!(order_cmp(Some(&int(2)), Some(&dec("10.5"))), Less);
        assert_eq!(order_cmp(Some(&int(10)), Some(&dbl("2.0E0"))), Greater);
        assert_eq!(order_cmp(Some(&int(1)), Some(&dec("1.0"))), Less); // tiebreak
        assert_eq!(order_cmp(Some(&s("b")), Some(&s("a"))), Greater);
        assert_eq!(order_cmp(Some(&dbl("NaN")), Some(&int(-5))), Less);
        assert_eq!(order_cmp(Some(&other("a")), Some(&other("a"))), Equal);
        // Sorting a mixed list is stable and total.
        let mut v = [
            s("b"),
            int(3),
            lang("x", "en"),
            dec("1.5"),
            iri("http://z"),
            other("q"),
            b(true),
            dt("2020-01-01T00:00:00Z"),
        ];
        v.sort_by(|a, b| order_cmp(Some(a), Some(b)));
        assert_eq!(v[0], iri("http://z"));
        assert_eq!(v[1], dec("1.5"));
        assert_eq!(v[2], int(3));
    }

    // ---- arithmetic ----

    #[test]
    fn promotion() {
        let r = add(&int(1), &int(2)).unwrap();
        assert_eq!((lex(&r), dt_of(&r)), ("3".into(), xsd("integer")));
        let r = add(&int(1), &dec("2.5")).unwrap();
        assert_eq!((lex(&r), dt_of(&r)), ("3.5".into(), xsd("decimal")));
        let r = add(&dec("1"), &flt("2")).unwrap();
        assert_eq!((lex(&r), dt_of(&r)), ("3.0E0".into(), xsd("float")));
        let r = mul(&flt("2"), &dbl("2")).unwrap();
        assert_eq!((lex(&r), dt_of(&r)), ("4.0E0".into(), xsd("double")));
        let r = add(&typed("1", "short"), &typed("2", "long")).unwrap();
        assert_eq!(dt_of(&r), xsd("integer"));
        assert!(add(&int(1), &s("1")).is_err());
        assert!(add(&typed("x", "integer"), &int(1)).is_err());
    }

    #[test]
    fn decimal_exact() {
        assert_eq!(lex(&add(&dec("0.1"), &dec("0.2")).unwrap()), "0.3");
        assert_eq!(
            equal(&add(&dec("0.1"), &dec("0.2")).unwrap(), &dec("0.3")),
            Ok(true)
        );
        assert_eq!(lex(&sub(&dec("1.0"), &dec("0.9")).unwrap()), "0.1");
        assert_eq!(lex(&mul(&dec("1.5"), &dec("2")).unwrap()), "3.0");
        assert_eq!(lex(&mul(&dec("-0.25"), &dec("0.2")).unwrap()), "-0.05");
        assert_eq!(lex(&div(&dec("1"), &dec("4")).unwrap()), "0.25");
        assert_eq!(
            lex(&div(&dec("1"), &dec("3")).unwrap()),
            "0.333333333333333333333333"
        );
        assert_eq!(lex(&dec_term("100.0")), "100.0");
    }

    fn dec_term(x: &str) -> Term {
        Num::Dec(Dec::parse(x).unwrap()).into_term()
    }

    #[test]
    fn division() {
        let r = div(&int(1), &int(2)).unwrap();
        assert_eq!((lex(&r), dt_of(&r)), ("0.5".into(), xsd("decimal")));
        let r = div(&int(4), &int(2)).unwrap();
        assert_eq!((lex(&r), dt_of(&r)), ("2.0".into(), xsd("decimal")));
        assert!(div(&int(1), &int(0)).is_err());
        assert!(div(&dec("1.5"), &dec("0.0")).is_err());
        assert_eq!(lex(&div(&dbl("1"), &dbl("0")).unwrap()), "INF");
        assert_eq!(lex(&div(&dbl("-1"), &int(0)).unwrap()), "-INF");
        assert_eq!(lex(&div(&dbl("0"), &dbl("0")).unwrap()), "NaN");
        assert_eq!(lex(&div(&flt("1"), &flt("0")).unwrap()), "INF");
    }

    #[test]
    fn unary_ops() {
        assert_eq!(lex(&neg(&int(3)).unwrap()), "-3");
        assert_eq!(lex(&neg(&dec("-1.5")).unwrap()), "1.5");
        assert_eq!(lex(&neg(&dbl("1")).unwrap()), "-1.0E0");
        assert_eq!(lex(&pos(&typed("+05", "integer")).unwrap()), "5");
        assert!(neg(&s("a")).is_err());
    }

    #[test]
    fn integer_overflow_errors() {
        let max = typed(i128::MAX.to_string(), "integer");
        assert!(add(&max, &int(1)).is_err());
        assert!(mul(&max, &int(2)).is_err());
    }

    #[test]
    fn canonical_forms() {
        assert_eq!(lex(&Num::Double(1.0).into_term()), "1.0E0");
        assert_eq!(lex(&Num::Double(1234.5).into_term()), "1.2345E3");
        assert_eq!(lex(&Num::Float(0.5).into_term()), "5.0E-1");
        assert_eq!(lex(&dec_term("-0.50")), "-0.5");
        assert_eq!(lex(&dec_term("+.5")), "0.5");
        assert_eq!(lex(&dec_term("007")), "7.0");
        assert!(Dec::parse(".").is_none());
        assert!(Dec::parse("1e3").is_none());
    }

    // ---- casts ----

    #[test]
    fn casts() {
        let i = xsd("integer");
        assert_eq!(cast(&i, &s(" 42 ")).unwrap(), int(42));
        assert_eq!(cast(&i, &dec("-3.9")).unwrap(), int(-3));
        assert_eq!(cast(&i, &dbl("2.5E0")).unwrap(), int(2));
        assert_eq!(cast(&i, &b(true)).unwrap(), int(1));
        assert!(cast(&i, &s("1.5")).is_err());
        assert!(cast(&i, &s("abc")).is_err());
        assert!(cast(&i, &dbl("INF")).is_err());
        assert!(cast(&i, &dbl("NaN")).is_err());
        assert!(cast(&i, &dt("2020-01-01T00:00:00Z")).is_err());
        assert!(cast(&i, &iri("http://x")).is_err());
        assert!(cast(&i, &lang("1", "en")).is_err());

        let d = xsd("decimal");
        assert_eq!(lex(&cast(&d, &s("1.50")).unwrap()), "1.5");
        assert_eq!(lex(&cast(&d, &int(3)).unwrap()), "3.0");
        assert_eq!(lex(&cast(&d, &dbl("1.25E0")).unwrap()), "1.25");
        assert!(cast(&d, &s("1e3")).is_err());
        assert!(cast(&d, &dbl("INF")).is_err());

        let db = xsd("double");
        assert_eq!(lex(&cast(&db, &s("1e3")).unwrap()), "1.0E3");
        assert_eq!(lex(&cast(&db, &s("INF")).unwrap()), "INF");
        assert_eq!(lex(&cast(&db, &int(2)).unwrap()), "2.0E0");
        assert!(cast(&db, &s("inf")).is_err());
        assert!(cast(&db, &s("1.2.3")).is_err());
        assert_eq!(
            dt_of(&cast(&xsd("float"), &s("1.5")).unwrap()),
            xsd("float")
        );

        let bo = xsd("boolean");
        assert_eq!(cast(&bo, &s("1")).unwrap(), b(true));
        assert_eq!(cast(&bo, &s("false")).unwrap(), b(false));
        assert_eq!(cast(&bo, &int(0)).unwrap(), b(false));
        assert_eq!(cast(&bo, &dbl("NaN")).unwrap(), b(false));
        assert_eq!(cast(&bo, &dec("0.1")).unwrap(), b(true));
        assert!(cast(&bo, &s("yes")).is_err());
        assert!(cast(&bo, &s("TRUE")).is_err());

        let st = xsd("string");
        // XPath casting: no exponent in [1e-6, 1e6), none for whole decimals
        assert_eq!(cast(&st, &dbl("1E0")).unwrap(), s("1"));
        assert_eq!(cast(&st, &dbl("1.5E7")).unwrap(), s("1.5E7"));
        assert_eq!(cast(&st, &dec("2.50")).unwrap(), s("2.5"));
        assert_eq!(cast(&st, &typed("01", "integer")).unwrap(), s("1"));
        assert_eq!(cast(&st, &iri("http://x")).unwrap(), s("http://x"));
        assert_eq!(cast(&st, &typed("1", "boolean")).unwrap(), s("true"));
        assert!(cast(&st, &Term::Blank("b".into())).is_err());

        let dtt = xsd("dateTime");
        assert_eq!(
            cast(&dtt, &s("2020-01-02T03:04:05Z")).unwrap(),
            dt("2020-01-02T03:04:05Z")
        );
        assert!(cast(&dtt, &s("2020-13-02T03:04:05Z")).is_err());
        assert!(cast(&dtt, &s("2020-02-30T00:00:00")).is_err());
        assert!(cast(&dtt, &int(1)).is_err());
        assert_eq!(
            cast(&dtt, &typed("2020-01-02Z", "date")).unwrap(),
            dt("2020-01-02T00:00:00Z")
        );
        assert_eq!(
            cast(&xsd("date"), &dt("2020-01-02T03:04:05-05:00")).unwrap(),
            typed("2020-01-02-05:00", "date")
        );
    }

    #[test]
    fn derived_integer_casts() {
        assert_eq!(cast(&xsd("byte"), &s("127")).unwrap(), typed("127", "byte"));
        assert!(cast(&xsd("byte"), &s("128")).is_err());
        assert!(cast(&xsd("unsignedInt"), &int(-1)).is_err());
        assert!(cast(&xsd("positiveInteger"), &int(0)).is_err());
        assert_eq!(
            cast(&xsd("negativeInteger"), &int(-1)).unwrap(),
            typed("-1", "negativeInteger")
        );
        assert!(cast("http://example.org/dt", &s("1")).is_err());
        // Out-of-range derived literal is ill-typed, not numeric.
        assert!(!is_numeric(&typed("300", "byte")));
    }

    // ---- functions ----

    #[test]
    fn str_lang_datatype() {
        assert_eq!(c(Function::Str, &[iri("http://a")]).unwrap(), s("http://a"));
        assert_eq!(c(Function::Str, &[lang("x", "en")]).unwrap(), s("x"));
        assert!(c(Function::Str, &[Term::Blank("b".into())]).is_err());
        assert_eq!(
            c(Function::Lang, &[lang("x", "en-US")]).unwrap(),
            s("en-US")
        );
        assert_eq!(c(Function::Lang, &[int(1)]).unwrap(), s(""));
        assert!(c(Function::Lang, &[iri("http://a")]).is_err());
        assert_eq!(
            c(Function::Datatype, &[int(1)]).unwrap(),
            iri(&xsd("integer"))
        );
        assert_eq!(
            c(Function::Datatype, &[s("a")]).unwrap(),
            iri(&xsd("string"))
        );
        assert_eq!(
            c(Function::Datatype, &[lang("a", "en")]).unwrap(),
            iri(RDF_LANG_STRING)
        );
        assert!(c(Function::Datatype, &[iri("http://a")]).is_err());
    }

    #[test]
    fn langmatches() {
        let lm = |t: &str, r: &str| c(Function::LangMatches, &[s(t), s(r)]).unwrap();
        assert_eq!(lm("en", "en"), b(true));
        assert_eq!(lm("en-US", "en"), b(true));
        assert_eq!(lm("EN-us", "en"), b(true));
        assert_eq!(lm("en", "en-US"), b(false));
        assert_eq!(lm("fr", "en"), b(false));
        assert_eq!(lm("eng", "en"), b(false));
        assert_eq!(lm("fr", "*"), b(true));
        assert_eq!(lm("", "*"), b(false));
    }

    #[test]
    fn iri_function() {
        let mut cx = ctx();
        cx.base_iri = Some("http://example.org/a/b/c?q".into());
        let r = |x: &str, cx: &mut FnContext| call(&Function::Iri, &[s(x)], cx);
        assert_eq!(r("d", &mut cx).unwrap(), iri("http://example.org/a/b/d"));
        assert_eq!(r("../d", &mut cx).unwrap(), iri("http://example.org/a/d"));
        assert_eq!(r("/x", &mut cx).unwrap(), iri("http://example.org/x"));
        assert_eq!(
            r("#f", &mut cx).unwrap(),
            iri("http://example.org/a/b/c?q#f")
        );
        assert_eq!(r("//h/p", &mut cx).unwrap(), iri("http://h/p"));
        assert_eq!(r("urn:x:y", &mut cx).unwrap(), iri("urn:x:y"));
        assert_eq!(r("./", &mut cx).unwrap(), iri("http://example.org/a/b/"));
        assert!(r("a b", &mut cx).is_err());
        assert_eq!(
            call(&Function::Iri, &[iri("http://z")], &mut cx).unwrap(),
            iri("http://z")
        );
        assert!(call(&Function::Iri, &[lang("x", "en")], &mut cx).is_err());
        assert_eq!(c(Function::Iri, &[s("http://q")]).unwrap(), iri("http://q"));
    }

    #[test]
    fn bnode_function() {
        let mut cx = ctx();
        let a = call(&Function::BNode, &[], &mut cx).unwrap();
        let b2 = call(&Function::BNode, &[], &mut cx).unwrap();
        assert!(matches!(a, Term::Blank(_)));
        assert_ne!(a, b2);
        let x1 = call(&Function::BNode, &[s("x")], &mut cx).unwrap();
        let x2 = call(&Function::BNode, &[s("x")], &mut cx).unwrap();
        let y = call(&Function::BNode, &[s("y")], &mut cx).unwrap();
        assert_eq!(x1, x2);
        assert_ne!(x1, y);
        cx.next_row();
        let x3 = call(&Function::BNode, &[s("x")], &mut cx).unwrap();
        assert_ne!(x1, x3);
        assert!(call(&Function::BNode, &[int(1)], &mut cx).is_err());
    }

    #[test]
    fn rand_uuid_now() {
        let mut cx = ctx();
        for _ in 0..100 {
            let r = call(&Function::Rand, &[], &mut cx).unwrap();
            assert_eq!(dt_of(&r), xsd("double"));
            let v: f64 = lex(&r).parse().unwrap();
            assert!((0.0..1.0).contains(&v));
        }
        let Term::Iri(u) = call(&Function::Uuid, &[], &mut cx).unwrap() else {
            panic!()
        };
        assert!(u.starts_with("urn:uuid:"));
        let su = lex(&call(&Function::StrUuid, &[], &mut cx).unwrap());
        assert_eq!(su.len(), 36);
        assert_eq!(&su[14..15], "4");
        assert!(matches!(&su[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(su, lex(&call(&Function::StrUuid, &[], &mut cx).unwrap()));
        assert_eq!(
            call(&Function::Now, &[], &mut cx).unwrap(),
            dt("2026-11-24T10:00:00Z")
        );
    }

    #[test]
    fn numeric_functions() {
        assert_eq!(c(Function::Abs, &[int(-3)]).unwrap(), int(3));
        assert_eq!(lex(&c(Function::Abs, &[dec("-1.5")]).unwrap()), "1.5");
        assert_eq!(lex(&c(Function::Abs, &[dbl("-2")]).unwrap()), "2.0E0");
        assert_eq!(lex(&c(Function::Ceil, &[dec("1.1")]).unwrap()), "2.0");
        assert_eq!(lex(&c(Function::Ceil, &[dec("-1.1")]).unwrap()), "-1.0");
        assert_eq!(lex(&c(Function::Floor, &[dec("-1.1")]).unwrap()), "-2.0");
        assert_eq!(lex(&c(Function::Floor, &[dbl("1.9")]).unwrap()), "1.0E0");
        assert_eq!(lex(&c(Function::Round, &[dec("2.5")]).unwrap()), "3.0");
        assert_eq!(lex(&c(Function::Round, &[dec("-2.5")]).unwrap()), "-2.0");
        assert_eq!(lex(&c(Function::Round, &[dec("-2.51")]).unwrap()), "-3.0");
        assert_eq!(lex(&c(Function::Round, &[dbl("2.5")]).unwrap()), "3.0E0");
        assert_eq!(lex(&c(Function::Round, &[dbl("-2.5")]).unwrap()), "-2.0E0");
        assert_eq!(lex(&c(Function::Round, &[flt("0.4")]).unwrap()), "0.0E0");
        assert_eq!(c(Function::Round, &[int(7)]).unwrap(), int(7));
        assert_eq!(
            dt_of(&c(Function::Ceil, &[dec("1.1")]).unwrap()),
            xsd("decimal")
        );
        assert!(c(Function::Abs, &[s("1")]).is_err());
        assert_eq!(c(Function::IsNumeric, &[int(1)]).unwrap(), b(true));
        assert_eq!(
            c(Function::IsNumeric, &[typed("x", "integer")]).unwrap(),
            b(false)
        );
        assert_eq!(c(Function::IsNumeric, &[s("1")]).unwrap(), b(false));
    }

    #[test]
    fn concat() {
        assert_eq!(c(Function::Concat, &[]).unwrap(), s(""));
        assert_eq!(c(Function::Concat, &[s("a"), s("b")]).unwrap(), s("ab"));
        assert_eq!(
            c(Function::Concat, &[lang("a", "en"), lang("b", "en")]).unwrap(),
            lang("ab", "en")
        );
        assert_eq!(
            c(Function::Concat, &[lang("a", "en"), lang("b", "fr")]).unwrap(),
            s("ab")
        );
        assert_eq!(
            c(Function::Concat, &[lang("a", "en"), s("b")]).unwrap(),
            s("ab")
        );
        assert!(c(Function::Concat, &[s("a"), int(1)]).is_err());
    }

    #[test]
    fn substr_strlen() {
        let sub = |x: Term, a: &[Term]| {
            let mut v = vec![x];
            v.extend_from_slice(a);
            c(Function::SubStr, &v).unwrap()
        };
        assert_eq!(sub(s("motor car"), &[int(6)]), s(" car"));
        assert_eq!(sub(s("metadata"), &[int(4), int(3)]), s("ada"));
        assert_eq!(sub(s("12345"), &[dbl("1.5"), dbl("2.6")]), s("234"));
        assert_eq!(sub(s("12345"), &[int(0), int(3)]), s("12"));
        assert_eq!(sub(s("12345"), &[int(5), int(-3)]), s(""));
        assert_eq!(sub(s("12345"), &[int(-3), int(5)]), s("1"));
        assert_eq!(sub(s("12345"), &[dbl("NaN"), int(3)]), s(""));
        assert_eq!(sub(s("12345"), &[int(-42), dbl("INF")]), s("12345"));
        // Code points, not bytes or UTF-16 units.
        assert_eq!(sub(s("日本語テキスト"), &[int(2), int(2)]), s("本語"));
        assert_eq!(sub(s("a😀b"), &[int(2), int(1)]), s("😀"));
        assert_eq!(sub(lang("foobar", "en"), &[int(4)]), lang("bar", "en"));
        assert!(c(Function::SubStr, &[s("abc"), s("1")]).is_err());
        assert_eq!(c(Function::StrLen, &[s("chat")]).unwrap(), int(4));
        assert_eq!(c(Function::StrLen, &[s("😀日本")]).unwrap(), int(3));
        assert_eq!(c(Function::StrLen, &[lang("chat", "fr")]).unwrap(), int(4));
        assert!(c(Function::StrLen, &[int(1)]).is_err());
    }

    #[test]
    fn case_and_encode() {
        assert_eq!(c(Function::UCase, &[s("foo")]).unwrap(), s("FOO"));
        assert_eq!(
            c(Function::UCase, &[lang("foo", "en")]).unwrap(),
            lang("FOO", "en")
        );
        assert_eq!(c(Function::LCase, &[s("BAR")]).unwrap(), s("bar"));
        assert!(c(Function::LCase, &[iri("http://A")]).is_err());
        assert_eq!(
            c(Function::EncodeForUri, &[s("Los Angeles")]).unwrap(),
            s("Los%20Angeles")
        );
        assert_eq!(
            c(Function::EncodeForUri, &[lang("~bébé/?", "fr")]).unwrap(),
            s("~b%C3%A9b%C3%A9%2F%3F")
        );
    }

    #[test]
    fn contains_starts_ends() {
        let t = |f: Function, a: Term, b2: Term| c(f, &[a, b2]);
        assert_eq!(
            t(Function::Contains, s("foobar"), s("bar")).unwrap(),
            b(true)
        );
        assert_eq!(t(Function::Contains, s("foobar"), s("")).unwrap(), b(true));
        assert_eq!(
            t(Function::StrStarts, s("foobar"), s("foo")).unwrap(),
            b(true)
        );
        assert_eq!(
            t(Function::StrEnds, s("foobar"), s("foo")).unwrap(),
            b(false)
        );
        assert_eq!(
            t(Function::Contains, lang("foobar", "en"), s("bar")).unwrap(),
            b(true)
        );
        assert_eq!(
            t(Function::Contains, lang("foobar", "en"), lang("bar", "en")).unwrap(),
            b(true)
        );
        assert!(t(Function::Contains, lang("foobar", "en"), lang("bar", "fr")).is_err());
        assert!(t(Function::Contains, s("foobar"), lang("bar", "en")).is_err());
        assert!(t(Function::StrStarts, s("1"), int(1)).is_err());
    }

    #[test]
    fn strbefore_strafter() {
        let sb = |a: Term, b2: Term| c(Function::StrBefore, &[a, b2]);
        let sa = |a: Term, b2: Term| c(Function::StrAfter, &[a, b2]);
        assert_eq!(sb(s("abc"), s("b")).unwrap(), s("a"));
        assert_eq!(sb(lang("abc", "en"), s("bc")).unwrap(), lang("a", "en"));
        assert_eq!(sb(lang("abc", "en"), lang("b", "cy")).ok(), None);
        assert_eq!(sb(s("abc"), s("xyz")).unwrap(), s(""));
        assert_eq!(sb(lang("abc", "en"), lang("z", "en")).unwrap(), s(""));
        assert_eq!(sb(lang("abc", "en"), s("")).unwrap(), lang("", "en"));
        assert_eq!(
            sb(lang("abc", "en"), lang("", "en")).unwrap(),
            lang("", "en")
        );
        assert!(sb(s("abc"), lang("b", "en")).is_err());
        assert_eq!(sa(s("abc"), s("b")).unwrap(), s("c"));
        assert_eq!(sa(lang("abc", "en"), s("ab")).unwrap(), lang("c", "en"));
        assert_eq!(sa(lang("abc", "en"), s("")).unwrap(), lang("abc", "en"));
        assert_eq!(sa(s("abc"), s("xyz")).unwrap(), s(""));
        assert_eq!(sa(s("ab😀cd"), s("😀")).unwrap(), s("cd"));
    }

    #[test]
    fn replace() {
        let r = |a: &[Term]| c(Function::Replace, a);
        assert_eq!(r(&[s("abcd"), s("b"), s("Z")]).unwrap(), s("aZcd"));
        assert_eq!(r(&[s("abab"), s("B"), s("Z"), s("i")]).unwrap(), s("aZaZ"));
        assert_eq!(r(&[s("banana"), s("ana"), s("*")]).unwrap(), s("b*na"));
        assert_eq!(
            r(&[s("abracadabra"), s("a(.)"), s("a$1$1")]).unwrap(),
            s("abbraccaddabbra")
        );
        assert_eq!(
            r(&[s("darted"), s("^(.*?)d(.*)$"), s("$1c$2")]).unwrap(),
            s("carted")
        );
        assert_eq!(r(&[s("a.b"), s("."), s("$"), s("q")]).unwrap(), s("a$b"));
        assert_eq!(r(&[s("ab"), s("b"), s("\\$x")]).unwrap(), s("a$x"));
        assert_eq!(
            r(&[lang("abc", "en"), s("b"), s("Z")]).unwrap(),
            lang("aZc", "en")
        );
        assert!(r(&[s("abc"), s("b"), s("$x")]).is_err());
        assert!(r(&[s("abc"), s("("), s("x")]).is_err());
        assert!(r(&[s("abc"), s("b"), s("x"), s("k")]).is_err());
        assert!(r(&[s("abc"), lang("b", "en"), s("x")]).is_err());
    }

    #[test]
    fn regex_flags() {
        let r = |a: &[Term]| c(Function::Regex, a).unwrap();
        assert_eq!(r(&[s("Alice"), s("^ali")]), b(false));
        assert_eq!(r(&[s("Alice"), s("^ali"), s("i")]), b(true));
        assert_eq!(r(&[s("a\nb"), s("a.b")]), b(false));
        assert_eq!(r(&[s("a\nb"), s("a.b"), s("s")]), b(true));
        assert_eq!(r(&[s("x\nb"), s("^b$")]), b(false));
        assert_eq!(r(&[s("x\nb"), s("^b$"), s("m")]), b(true));
        assert_eq!(r(&[s("helloworld"), s("hello world"), s("x")]), b(true));
        assert_eq!(r(&[s("a.c"), s("."), s("q")]), b(true));
        assert_eq!(r(&[s("abc"), s("."), s("q")]), b(false));
        assert_eq!(r(&[s("A.C"), s("a.c"), s("qi")]), b(true));
        assert_eq!(r(&[lang("chat", "fr"), s("ch")]), b(true));
        assert!(c(Function::Regex, &[s("a"), s("a"), s("z")]).is_err());
        assert!(c(Function::Regex, &[int(1), s("1")]).is_err());
        assert!(c(Function::Regex, &[s("a"), s("[")]).is_err());
    }

    #[test]
    fn date_functions() {
        let d = dt("2011-01-10T14:45:13.815-05:00");
        assert_eq!(
            c(Function::Year, std::slice::from_ref(&d)).unwrap(),
            int(2011)
        );
        assert_eq!(
            c(Function::Month, std::slice::from_ref(&d)).unwrap(),
            int(1)
        );
        assert_eq!(c(Function::Day, std::slice::from_ref(&d)).unwrap(), int(10));
        assert_eq!(
            c(Function::Hours, std::slice::from_ref(&d)).unwrap(),
            int(14)
        );
        assert_eq!(
            c(Function::Minutes, std::slice::from_ref(&d)).unwrap(),
            int(45)
        );
        let sec = c(Function::Seconds, std::slice::from_ref(&d)).unwrap();
        assert_eq!((lex(&sec), dt_of(&sec)), ("13.815".into(), xsd("decimal")));
        let tz = c(Function::Timezone, std::slice::from_ref(&d)).unwrap();
        assert_eq!(
            (lex(&tz), dt_of(&tz)),
            ("-PT5H".into(), xsd("dayTimeDuration"))
        );
        assert_eq!(
            cs(Function::Timezone, &[dt("2011-01-10T14:45:13Z")]),
            "PT0S"
        );
        assert_eq!(
            cs(Function::Timezone, &[dt("2011-01-10T14:45:13+05:30")]),
            "PT5H30M"
        );
        assert!(c(Function::Timezone, &[dt("2011-01-10T14:45:13")]).is_err());
        assert_eq!(c(Function::Tz, &[d]).unwrap(), s("-05:00"));
        assert_eq!(
            c(Function::Tz, &[dt("2011-01-10T14:45:13Z")]).unwrap(),
            s("Z")
        );
        assert_eq!(
            c(Function::Tz, &[dt("2011-01-10T14:45:13")]).unwrap(),
            s("")
        );
        assert_eq!(
            c(Function::Year, &[dt("-0044-03-15T00:00:00")]).unwrap(),
            int(-44)
        );
        assert!(c(Function::Year, &[s("2011-01-10T14:45:13Z")]).is_err());
        assert!(c(Function::Year, &[dt("not a date")]).is_err());
    }

    #[test]
    fn hashes() {
        assert_eq!(
            cs(Function::Md5, &[s("abc")]),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(
            cs(Function::Sha1, &[s("abc")]),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            cs(Function::Sha256, &[s("abc")]),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(cs(Function::Sha384, &[s("abc")]).len(), 96);
        assert_eq!(cs(Function::Sha512, &[s("abc")]).len(), 128);
        assert!(c(Function::Md5, &[lang("abc", "en")]).is_err());
        assert!(c(Function::Md5, &[int(1)]).is_err());
    }

    #[test]
    fn strlang_strdt() {
        assert_eq!(
            c(Function::StrLang, &[s("chat"), s("fr")]).unwrap(),
            lang("chat", "fr")
        );
        assert!(c(Function::StrLang, &[lang("chat", "en"), s("fr")]).is_err());
        assert!(c(Function::StrLang, &[s("chat"), s("")]).is_err());
        assert!(c(Function::StrLang, &[s("chat"), s("not a tag")]).is_err());
        assert_eq!(
            c(Function::StrDt, &[s("123"), iri(&xsd("integer"))]).unwrap(),
            int(123)
        );
        assert_eq!(
            c(Function::StrDt, &[s("x"), iri(&xsd("string"))]).unwrap(),
            s("x")
        );
        assert!(c(Function::StrDt, &[lang("1", "en"), iri(&xsd("integer"))]).is_err());
        assert!(c(Function::StrDt, &[s("1"), s("http://x")]).is_err());
        assert!(c(Function::StrDt, &[s("1"), iri(RDF_LANG_STRING)]).is_err());
    }

    #[test]
    fn type_tests() {
        let bl = Term::Blank("b".into());
        assert_eq!(c(Function::IsIri, &[iri("http://a")]).unwrap(), b(true));
        assert_eq!(c(Function::IsIri, &[s("a")]).unwrap(), b(false));
        assert_eq!(
            c(Function::IsBlank, std::slice::from_ref(&bl)).unwrap(),
            b(true)
        );
        assert_eq!(c(Function::IsLiteral, &[bl]).unwrap(), b(false));
        assert_eq!(c(Function::IsLiteral, &[int(1)]).unwrap(), b(true));
    }

    #[test]
    fn triple_terms() {
        let t = c(
            Function::Triple,
            &[iri("http://s"), iri("http://p"), int(1)],
        )
        .unwrap();
        assert_eq!(t, Term::triple(iri("http://s"), iri("http://p"), int(1)));
        assert_eq!(
            t.to_string(),
            "<<( <http://s> <http://p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> )>>"
        );
        assert_eq!(
            c(Function::Subject, std::slice::from_ref(&t)).unwrap(),
            iri("http://s")
        );
        assert_eq!(
            c(Function::Predicate, std::slice::from_ref(&t)).unwrap(),
            iri("http://p")
        );
        assert_eq!(
            c(Function::Object, std::slice::from_ref(&t)).unwrap(),
            int(1)
        );
        assert_eq!(
            c(Function::IsTriple, std::slice::from_ref(&t)).unwrap(),
            b(true)
        );
        assert_eq!(c(Function::IsTriple, &[int(1)]).unwrap(), b(false));
        // literal or triple-term subject, non-IRI predicate: errors
        assert!(c(Function::Triple, &[int(1), iri("http://p"), int(1)]).is_err());
        assert!(c(Function::Triple, &[t.clone(), iri("http://p"), int(1)]).is_err());
        assert!(c(Function::Triple, &[iri("http://s"), s("p"), int(1)]).is_err());
        assert!(c(Function::Subject, &[iri("http://s")]).is_err());
        assert!(c(Function::Predicate, &[iri("http://s")]).is_err());
        assert!(c(Function::Object, &[iri("http://s")]).is_err());
        assert!(ebv(&t).is_err());
        // `=` compares components by value; sameTerm is structural
        let t2 = Term::triple(iri("http://s"), iri("http://p"), dbl("1.0E0"));
        assert!(equal(&t, &t2).unwrap());
        assert!(!same_term(&t, &t2));
        assert!(!equal(&t, &iri("http://s")).unwrap());
        // ORDER BY: literals < triple terms, then by components
        let lower = Term::triple(iri("http://s"), iri("http://p"), int(-5));
        assert_eq!(order_cmp(Some(&s("z")), Some(&t)), Ordering::Less);
        assert_eq!(order_cmp(Some(&lower), Some(&t)), Ordering::Less);
        let nested = Term::triple(iri("http://s"), iri("http://p"), t.clone());
        assert_eq!(order_cmp(Some(&t), Some(&nested)), Ordering::Less);
    }

    #[test]
    fn rdf12_and_unsupported() {
        assert_eq!(c(Function::HasLang, &[lang("a", "en")]).unwrap(), b(true));
        assert_eq!(c(Function::HasLang, &[s("a")]).unwrap(), b(false));
        assert_eq!(
            c(Function::HasLangDir, &[lang("a", "en")]).unwrap(),
            b(false)
        );
        assert_eq!(c(Function::LangDir, &[lang("a", "en")]).unwrap(), s(""));
        assert!(c(Function::StrLangDir, &[s("a"), s("en"), s("ltr")]).is_err());
        let custom = Function::Custom(oxrdf::NamedNode::new_unchecked("http://ex/f"));
        let e = c(custom, &[]).unwrap_err();
        assert!(e.0.contains("http://ex/f"), "{e}");
    }

    #[test]
    fn arity_checked() {
        assert!(c(Function::Str, &[]).is_err());
        assert!(c(Function::Rand, &[int(1)]).is_err());
        assert!(c(Function::SubStr, &[s("a")]).is_err());
    }

    #[test]
    fn date_parsing() {
        assert!(parse_date_time("2020-02-29T00:00:00", false).is_some());
        assert!(parse_date_time("2021-02-29T00:00:00", false).is_none());
        assert!(parse_date_time("2020-01-01T24:00:00", false).is_some());
        assert!(parse_date_time("2020-01-01T24:00:01", false).is_none());
        assert!(parse_date_time("2020-01-01T10:00:00+15:00", false).is_none());
        assert!(parse_date_time("2020-01-01T10:00:00.", false).is_none());
        assert!(parse_date_time("2020-01-01T10:00", false).is_none());
        assert!(parse_date_time("12020-01-01T10:00:00Z", false).is_some());
        assert!(parse_date_time("02020-01-01T10:00:00Z", false).is_none());
    }
}
