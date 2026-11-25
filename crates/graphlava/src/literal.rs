//! Canonical literal forms and the inline routing of literals
//! ([[02-id-encoding#Canonical forms]], rules `id-canonical`, `id-inline-range`).
//!
//! The folding table here is part of the file format
//! (rule `id-canonical-contract`): changing what a lexical form folds to needs
//! a format version bump.

// @tg: implements:: [[openspec:term-identity#Canonical literal forms]]

use crate::id::{self, Id, Subtype};
use crate::term::{Literal, RDF_LANG_STRING, XSD};

/// The canonical form of a literal, before any dictionary access.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Canon {
    /// Fits inline; the ID is the literal.
    Inline(Id),
    /// A language string that is inline if its tag gets a language ID.
    LangCandidate { text: String, tag: String },
    /// Stored in the dictionary with this canonical lexical form.
    Dict {
        lexical: String,
        datatype: String,
        lang: Option<String>,
    },
}

fn xsd_local(dt: &str) -> Option<&str> {
    dt.strip_prefix(XSD)
}

/// Integer-derived types: (name, min, max) with `None` for unbounded.
fn integer_bounds(local: &str) -> Option<(Option<i128>, Option<i128>)> {
    Some(match local {
        "integer" => (None, None),
        "long" => (Some(i64::MIN as i128), Some(i64::MAX as i128)),
        "int" => (Some(i32::MIN as i128), Some(i32::MAX as i128)),
        "short" => (Some(i16::MIN as i128), Some(i16::MAX as i128)),
        "byte" => (Some(i8::MIN as i128), Some(i8::MAX as i128)),
        "nonNegativeInteger" => (Some(0), None),
        "positiveInteger" => (Some(1), None),
        "nonPositiveInteger" => (None, Some(0)),
        "negativeInteger" => (None, Some(-1)),
        "unsignedLong" => (Some(0), Some(u64::MAX as i128)),
        "unsignedInt" => (Some(0), Some(u32::MAX as i128)),
        "unsignedShort" => (Some(0), Some(u16::MAX as i128)),
        "unsignedByte" => (Some(0), Some(u8::MAX as i128)),
        _ => return None,
    })
}

/// Splits `[+-]digits` into sign and digits without leading zeros.
fn parse_integer_lex(s: &str) -> Option<(bool, String)> {
    let s = s.trim();
    let (neg, digits) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let d = digits.trim_start_matches('0');
    let d = if d.is_empty() { "0" } else { d };
    Some((neg && d != "0", d.to_string()))
}

/// Splits a decimal into sign, integer digits and fractional digits, both
/// without redundant zeros.
fn parse_decimal_lex(s: &str) -> Option<(bool, String, String)> {
    let s = s.trim();
    let (neg, body) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let (ip, fp) = match body.split_once('.') {
        Some((a, b)) => (a, b),
        None => (body, ""),
    };
    if ip.is_empty() && fp.is_empty() {
        return None;
    }
    if !ip.bytes().all(|b| b.is_ascii_digit()) || !fp.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ip = ip.trim_start_matches('0');
    let fp = fp.trim_end_matches('0');
    let zero = ip.is_empty() && fp.is_empty();
    Some((neg && !zero, ip.to_string(), fp.to_string()))
}

fn decimal_lexical(neg: bool, ip: &str, fp: &str) -> String {
    let ip = if ip.is_empty() { "0" } else { ip };
    let sign = if neg { "-" } else { "" };
    if fp.is_empty() {
        format!("{sign}{ip}")
    } else {
        format!("{sign}{ip}.{fp}")
    }
}

/// XSD canonical double lexical form: `1.5E-1`, `0.0E0`, `INF`, `NaN`.
pub fn double_lexical(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "INF".into() } else { "-INF".into() };
    }
    let s = format!("{x:E}");
    let (m, e) = s.split_once('E').expect("E format");
    let m = if m.contains('.') {
        m.to_string()
    } else {
        format!("{m}.0")
    };
    format!("{m}E{e}")
}

fn float_lexical(x: f32) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "INF".into() } else { "-INF".into() };
    }
    let s = format!("{x:E}");
    let (m, e) = s.split_once('E').expect("E format");
    let m = if m.contains('.') {
        m.to_string()
    } else {
        format!("{m}.0")
    };
    format!("{m}E{e}")
}

fn parse_double_lex(s: &str) -> Option<f64> {
    let s = s.trim();
    match s {
        "INF" | "+INF" => return Some(f64::INFINITY),
        "-INF" => return Some(f64::NEG_INFINITY),
        "NaN" => return Some(f64::NAN),
        _ => {}
    }
    // XSD allows digits, '.', sign and exponent only (no "inf", "nan", "infinity").
    if s.is_empty()
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'+' | b'-' | b'e' | b'E'))
    {
        return None;
    }
    s.parse::<f64>().ok()
}

// ---- calendar ---------------------------------------------------------------

/// Days since 1970-01-01 of a proleptic Gregorian date (year 0 = 1 BCE, XSD 1.1).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    }
}

fn num(s: &str, len: usize) -> Option<i64> {
    if s.len() != len || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Parses `[-]YYYY-MM-DD` and returns (year, month, day, rest).
fn parse_date_part(s: &str) -> Option<(i64, u32, u32, &str)> {
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
    let y: i64 = ys.parse().ok()?;
    let y = if neg { -y } else { y };
    let rest = &body[ydash + 1..];
    let m = num(rest.get(..2)?, 2)? as u32;
    if rest.as_bytes().get(2) != Some(&b'-') {
        return None;
    }
    let d = num(rest.get(3..5)?, 2)? as u32;
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    Some((y, m, d, &rest[5..]))
}

/// Parses an optional timezone; returns `Some(None)` when absent and the
/// offset in minutes when present.
fn parse_tz(s: &str) -> Option<Option<i64>> {
    match s {
        "" => Some(None),
        "Z" => Some(Some(0)),
        _ => {
            let sign = match s.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hh = num(s.get(1..3)?, 2)?;
            if s.as_bytes().get(3) != Some(&b':') {
                return None;
            }
            let mm = num(s.get(4..6)?, 2)?;
            if s.len() != 6 || hh > 14 || mm > 59 || (hh == 14 && mm != 0) {
                return None;
            }
            Some(Some(sign * (hh * 60 + mm)))
        }
    }
}

/// `xsd:dateTime` to UTC microseconds and a has-timezone flag. `None` when the
/// value is not valid or has sub-microsecond precision.
pub(crate) fn parse_date_time(s: &str) -> Option<(i64, bool)> {
    let (y, m, d, rest) = parse_date_part(s.trim())?;
    let rest = rest.strip_prefix('T')?;
    let hh = num(rest.get(..2)?, 2)?;
    let mi = num(rest.get(3..5)?, 2)?;
    let ss = num(rest.get(6..8)?, 2)?;
    if rest.as_bytes().get(2) != Some(&b':') || rest.as_bytes().get(5) != Some(&b':') {
        return None;
    }
    let mut tail = &rest[8..];
    let mut micros = 0i64;
    if let Some(f) = tail.strip_prefix('.') {
        let n = f.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        let digits = &f[..n];
        let (head, extra) = digits.split_at(n.min(6));
        if extra.bytes().any(|b| b != b'0') {
            return None; // finer than a microsecond: not inline
        }
        micros = format!("{head:0<6}").parse().ok()?;
        tail = &f[n..];
    }
    let tz = parse_tz(tail)?;
    if mi > 59 || ss > 59 || hh > 24 || (hh == 24 && (mi != 0 || ss != 0 || micros != 0)) {
        return None;
    }
    let days = days_from_civil(y, m, d) as i128;
    let mut t = days * 86_400_000_000
        + (hh as i128 * 3600 + mi as i128 * 60 + ss as i128) * 1_000_000
        + micros as i128;
    if let Some(off) = tz {
        t -= off as i128 * 60_000_000;
    }
    i64::try_from(t).ok().map(|t| (t, tz.is_some()))
}

/// `xsd:date` to days of the written date and a has-timezone flag.
pub(crate) fn parse_date(s: &str) -> Option<(i64, bool)> {
    let (y, m, d, rest) = parse_date_part(s.trim())?;
    let tz = parse_tz(rest)?;
    Some((days_from_civil(y, m, d), tz.is_some()))
}

fn year_str(y: i64) -> String {
    if y < 0 {
        format!("-{:04}", -y)
    } else {
        format!("{y:04}")
    }
}

pub(crate) fn date_time_lexical(t: i64, tz: bool) -> String {
    let days = t.div_euclid(86_400_000_000);
    let rem = t.rem_euclid(86_400_000_000);
    let (y, m, d) = civil_from_days(days);
    let secs = rem / 1_000_000;
    let us = rem % 1_000_000;
    let mut s = format!(
        "{}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        year_str(y),
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    );
    if us != 0 {
        let f = format!("{us:06}");
        s.push('.');
        s.push_str(f.trim_end_matches('0'));
    }
    if tz {
        s.push('Z');
    }
    s
}

pub(crate) fn date_lexical(days: i64, tz: bool) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{}-{m:02}-{d:02}{}", year_str(y), if tz { "Z" } else { "" })
}

/// Canonicalises a literal (rule `id-canonical`).
pub(crate) fn canonicalize(l: &Literal) -> Canon {
    let dict = |lexical: String, datatype: &str| Canon::Dict {
        lexical,
        datatype: datatype.to_string(),
        lang: None,
    };
    if let Some(tag) = &l.lang {
        let tag = tag.to_ascii_lowercase();
        if id::enc_lang_string(&l.lexical, 1).is_some() {
            return Canon::LangCandidate {
                text: l.lexical.clone(),
                tag,
            };
        }
        return Canon::Dict {
            lexical: l.lexical.clone(),
            datatype: RDF_LANG_STRING.to_string(),
            lang: Some(tag),
        };
    }
    let Some(local) = xsd_local(&l.datatype) else {
        return dict(l.lexical.clone(), &l.datatype);
    };
    if let Some((min, max)) = integer_bounds(local) {
        let Some((neg, digits)) = parse_integer_lex(&l.lexical) else {
            return dict(l.lexical.clone(), &l.datatype); // ill-typed: kept as written
        };
        let lex = if neg {
            format!("-{digits}")
        } else {
            digits.clone()
        };
        let v = lex.parse::<i128>().ok();
        let in_type = match v {
            Some(v) => min.is_none_or(|m| v >= m) && max.is_none_or(|m| v <= m),
            None => {
                min.is_none() && max.is_none() || (neg && min.is_none()) || (!neg && max.is_none())
            }
        };
        if !in_type {
            return dict(l.lexical.clone(), &l.datatype);
        }
        if let Some(i) = v.and_then(|v| i64::try_from(v).ok()).and_then(id::enc_int) {
            return Canon::Inline(i); // every in-range integer type folds to the integer run
        }
        return dict(lex, &l.datatype);
    }
    match local {
        "string" => {
            if let Some(i) = id::enc_string(&l.lexical) {
                return Canon::Inline(i);
            }
            dict(l.lexical.clone(), &l.datatype)
        }
        "boolean" => match l.lexical.trim() {
            "true" | "1" => Canon::Inline(id::enc_bool(true)),
            "false" | "0" => Canon::Inline(id::enc_bool(false)),
            _ => dict(l.lexical.clone(), &l.datatype),
        },
        "decimal" => {
            let Some((neg, ip, fp)) = parse_decimal_lex(&l.lexical) else {
                return dict(l.lexical.clone(), &l.datatype);
            };
            if fp.len() <= 9 && ip.len() <= 18 {
                let frac: i128 = format!("{fp:0<9}").parse().unwrap_or(0);
                let int: i128 = if ip.is_empty() {
                    0
                } else {
                    ip.parse().unwrap_or(i128::MAX / 2)
                };
                let scaled = (int * id::DEC_SCALE + frac) * if neg { -1 } else { 1 };
                if let Some(i) = id::enc_decimal(scaled) {
                    return Canon::Inline(i);
                }
            }
            dict(decimal_lexical(neg, &ip, &fp), &l.datatype)
        }
        "double" => match parse_double_lex(&l.lexical) {
            Some(x) => match id::enc_double(x) {
                Some(i) => Canon::Inline(i),
                None => dict(
                    double_lexical(if x.is_nan() { f64::NAN } else { x }),
                    &l.datatype,
                ),
            },
            None => dict(l.lexical.clone(), &l.datatype),
        },
        "float" => match parse_double_lex(&l.lexical) {
            Some(x) => dict(float_lexical(x as f32), &l.datatype),
            None => dict(l.lexical.clone(), &l.datatype),
        },
        "dateTime" | "dateTimeStamp" => match parse_date_time(&l.lexical) {
            Some((t, tz)) if local == "dateTime" || tz => {
                match id::enc_instant(Subtype::DateTime, t, tz) {
                    Some(i) => Canon::Inline(i),
                    None => dict(l.lexical.trim().to_string(), &l.datatype),
                }
            }
            _ => dict(l.lexical.clone(), &l.datatype),
        },
        "date" => match parse_date(&l.lexical) {
            Some((d, tz)) => match id::enc_instant(Subtype::Date, d, tz) {
                Some(i) => Canon::Inline(i),
                None => dict(l.lexical.trim().to_string(), &l.datatype),
            },
            None => dict(l.lexical.clone(), &l.datatype),
        },
        _ => dict(l.lexical.clone(), &l.datatype),
    }
}

/// Decodes an inline literal (other than a language string, which needs the
/// language table) to its canonical lexical form and datatype.
pub(crate) fn decode_inline(id: Id) -> Option<Literal> {
    if id.is_inline_double() {
        return Some(Literal::typed(
            double_lexical(id::dec_double(id)),
            crate::term::xsd("double"),
        ));
    }
    let x = |local: &str| crate::term::xsd(local);
    Some(match id.subtype()? {
        Subtype::Integer => Literal::typed(id::dec_int(id).to_string(), x("integer")),
        Subtype::Decimal => {
            let v = id::dec_decimal(id);
            let neg = v < 0;
            let a = v.unsigned_abs();
            let ip = (a / id::DEC_SCALE as u128).to_string();
            let fp = format!("{:09}", a % id::DEC_SCALE as u128);
            let ip = if ip == "0" { String::new() } else { ip };
            Literal::typed(
                decimal_lexical(neg, &ip, fp.trim_end_matches('0')),
                x("decimal"),
            )
        }
        Subtype::Boolean => Literal::typed(id::dec_bool(id).to_string(), x("boolean")),
        Subtype::DateTime => {
            let (t, tz) = id::dec_instant(id);
            Literal::typed(date_time_lexical(t, tz), x("dateTime"))
        }
        Subtype::Date => {
            let (d, tz) = id::dec_instant(id);
            Literal::typed(date_lexical(d, tz), x("date"))
        }
        Subtype::String => Literal::string(id::dec_string(id)),
        Subtype::LangString => return None,
    })
}

/// The `propidx` sort key of a literal (rule `pl-sort-key`): its ID when
/// inline, otherwise the ID it would have if it fit, with the payload truncated
/// or clamped. Entries with equal keys are rechecked on decode.
pub(crate) fn sort_key(id: Id, lit: &Literal, lang_id: Option<u16>) -> u64 {
    if id.is_inline() {
        return id.raw();
    }
    let local = xsd_local(&lit.datatype).unwrap_or("");
    if let Some(tag_id) = lang_id {
        let b = lit.lexical.as_bytes();
        let mut head: Vec<u8> = b[..b.len().min(5)].to_vec();
        while head.last() == Some(&0) {
            head.pop();
        }
        let text = String::from_utf8_lossy(&head).into_owned();
        return id::enc_lang_string(&text, tag_id.max(1)).map_or(id.raw(), Id::raw);
    }
    if integer_bounds(local).is_some() {
        return match parse_integer_lex(&lit.lexical) {
            Some((neg, _)) => if neg {
                id::enc_int(id::INT_MIN)
            } else {
                id::enc_int(id::INT_MAX)
            }
            .map_or(id.raw(), Id::raw),
            None => id.raw(),
        };
    }
    match local {
        "string" => {
            let b = lit.lexical.as_bytes();
            let mut head: Vec<u8> = b[..b.len().min(7)].to_vec();
            while head.last() == Some(&0) {
                head.pop();
            }
            id::enc_string(&String::from_utf8_lossy(&head)).map_or(id.raw(), Id::raw)
        }
        "decimal" => match parse_decimal_lex(&lit.lexical) {
            Some((neg, ip, fp)) => {
                let fp9: String = format!("{fp:0<9}").chars().take(9).collect();
                let int: i128 = if ip.is_empty() {
                    0
                } else {
                    ip.parse().unwrap_or(i128::MAX / id::DEC_SCALE)
                };
                let scaled = int
                    .saturating_mul(id::DEC_SCALE)
                    .saturating_add(fp9.parse::<i128>().unwrap_or(0));
                let scaled = if neg { -scaled } else { scaled };
                let clamped = scaled.clamp(id::INT_MIN as i128, id::INT_MAX as i128);
                id::enc_decimal(clamped).map_or(id.raw(), Id::raw)
            }
            None => id.raw(),
        },
        "double" => match parse_double_lex(&lit.lexical) {
            Some(x) if x.is_nan() => id::enc_double(f64::INFINITY).map_or(id.raw(), Id::raw),
            Some(x) => {
                // Round the magnitude towards zero into the inline range.
                let a = x.abs();
                let max = f64::from_bits(((1277u64) << 52) | ((1 << 52) - 1));
                let min = f64::from_bits(768u64 << 52);
                let a = if a > max {
                    max
                } else if a < min {
                    0.0
                } else {
                    a
                };
                id::enc_double(if x.is_sign_negative() { -a } else { a }).map_or(id.raw(), Id::raw)
            }
            None => id.raw(),
        },
        "dateTime" | "dateTimeStamp" | "date" => {
            let neg = lit.lexical.trim_start().starts_with('-');
            let sub = if local == "date" {
                Subtype::Date
            } else {
                Subtype::DateTime
            };
            let t = if neg { -id::B56 } else { id::B56 - 1 };
            id::enc_instant(sub, t, true).map_or(id.raw(), Id::raw)
        }
        _ => id.raw(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::xsd;

    fn canon(lex: &str, dt: &str) -> Canon {
        canonicalize(&Literal::typed(lex, xsd(dt)))
    }

    // @tg: verifies:: [[openspec:term-identity#Canonical literal forms#Leading zeros are folded]] {level: unit}
    #[test]
    fn integer_folding_corpus() {
        let one = Canon::Inline(id::enc_int(1).unwrap());
        for lex in ["1", "01", "+1", "0001", " 1 "] {
            assert_eq!(canon(lex, "integer"), one, "{lex}");
        }
        assert_eq!(canon("5", "int"), canon("5", "integer"));
        assert_eq!(canon("-0", "integer"), canon("0", "integer"));
        // ill-typed values keep their lexical form
        assert!(matches!(canon("300", "byte"), Canon::Dict { .. }));
        assert!(matches!(canon("abc", "integer"), Canon::Dict { .. }));
        // out of range: dictionary with canonical digits
        let big = (1i128 << 60).to_string();
        assert_eq!(
            canon(&format!("+000{big}"), "integer"),
            Canon::Dict {
                lexical: big,
                datatype: xsd("integer"),
                lang: None
            }
        );
    }

    #[test]
    fn decimal_and_double_folding() {
        assert_eq!(canon("1.50", "decimal"), canon("01.5", "decimal"));
        assert_eq!(canon("-0.0", "decimal"), canon("0", "decimal"));
        assert_eq!(
            decode_inline(match canon("1.50", "decimal") {
                Canon::Inline(i) => i,
                _ => panic!(),
            })
            .unwrap()
            .lexical,
            "1.5"
        );
        assert_eq!(canon("1e0", "double"), canon("1.0E0", "double"));
        assert_ne!(canon("-0.0", "double"), canon("0.0", "double"));
        assert_eq!(
            canon("NaN", "double"),
            Canon::Dict {
                lexical: "NaN".into(),
                datatype: xsd("double"),
                lang: None
            }
        );
        assert_eq!(double_lexical(0.15), "1.5E-1");
        assert_eq!(double_lexical(0.0), "0.0E0");
    }

    #[test]
    fn booleans_and_strings() {
        assert_eq!(canon("1", "boolean"), canon("true", "boolean"));
        assert_eq!(
            canonicalize(&Literal::string("hi")),
            Canon::Inline(id::enc_string("hi").unwrap())
        );
        assert!(matches!(
            canonicalize(&Literal::string("a\0")),
            Canon::Dict { .. }
        ));
        assert!(
            matches!(canonicalize(&Literal::lang("hi", "EN-gb")), Canon::LangCandidate { ref tag, .. } if tag == "en-gb")
        );
    }

    #[test]
    fn date_times() {
        let a = canon("2026-11-24T10:00:00+02:00", "dateTime");
        let b = canon("2026-11-24T08:00:00Z", "dateTime");
        assert_eq!(a, b);
        let c = canon("2026-11-24T08:00:00", "dateTime");
        assert_ne!(b, c);
        let Canon::Inline(i) = b else { panic!() };
        assert_eq!(decode_inline(i).unwrap().lexical, "2026-11-24T08:00:00Z");
        assert_eq!(canon("2024-02-29", "date"), canon("2024-02-29", "date"));
        assert!(matches!(canon("2023-02-29", "date"), Canon::Dict { .. }));
        assert!(matches!(
            canon("2026-11-24T08:00:00.0000001Z", "dateTime"),
            Canon::Dict { .. }
        ));
        for d in [-1_000_000i64, -1, 0, 1, 20_000, 1_000_000] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
    }
}
