//! 64-bit term IDs ([[02-id-encoding]], [[adr:0011]], [[adr:0021]]).
//!
//! Layout, matching `MetaCore.Id` in `formal/` exactly (checked by the property
//! test `id_layout_matches_model`):
//! - non-literal: `kind << 60 | payload`, kind in `1..=7`, bit 63 clear;
//! - chunked kinds: payload = `chunk << 20 | offset`;
//! - inline double: `0b10 << 62 | payload` (62-bit payload);
//! - other inline literal: `0b11 << 62 | subtype << 58 | payload` (58 bits).

// @tg: implements:: [[openspec:term-identity#Stable term identity]]
// @tg: implements:: [[openspec:term-identity#Order-preserving inline literals]]

use std::fmt;

/// Weight of the 4-bit kind tag (`MetaCore.Id.K`).
pub const K: u64 = 1 << 60;
/// IDs per chunk (`MetaCore.Id.C`).
pub const CHUNK: u64 = 1 << 20;
/// Number of chunks per chunked kind (`MetaCore.Id.NC`).
pub const N_CHUNKS: u64 = 1 << 40;
/// Bias of signed 58-bit payloads (`MetaCore.Id.B`).
pub const B57: i64 = 1 << 57;
/// Bias of the inline-double payload (`MetaCore.Id.D`).
pub const D61: u64 = 1 << 61;
/// Bias of the 57-bit signed date and dateTime instants.
pub const B56: i64 = 1 << 56;
/// Owned dictionary literal flag (bit 59).
pub const OWNED_BIT: u64 = 1 << 59;

/// The kind of a non-literal ID, or the class of a literal one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Null,
    Iri,
    Blank,
    DictLiteral,
    Statement,
    Graph,
    Hyperedge,
    System,
    InlineLiteral,
}

impl Kind {
    /// The 4-bit tag of a non-literal kind.
    pub const fn tag(self) -> u64 {
        match self {
            Kind::Null => 0,
            Kind::Iri => 1,
            Kind::Blank => 2,
            Kind::DictLiteral => 3,
            Kind::Statement => 4,
            Kind::Graph => 5,
            Kind::Hyperedge => 6,
            Kind::System => 7,
            Kind::InlineLiteral => 8,
        }
    }
}

/// Inline literal subtypes of class `11`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Subtype {
    Integer = 0,
    Decimal = 1,
    Boolean = 2,
    DateTime = 3,
    Date = 4,
    String = 5,
    LangString = 6,
}

/// A 64-bit term ID.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Id(pub u64);

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({:#018x})", self.0)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#018x}", self.0)
    }
}

/// The null / unbound ID; never allocated.
pub const NULL: Id = Id(0);
/// The system graph (rule `id-graph-bootstrap`): graph chunk 0, offset 0.
pub const SYSTEM_GRAPH: Id = Id(0x5000_0000_0000_0000);
/// The default graph created with the file ([[adr:0038]]): graph chunk 0, offset 1.
pub const DEFAULT_GRAPH: Id = Id(0x5000_0000_0000_0001);

impl Id {
    /// `MetaCore.Id.mk`: `kind * 2^60 + payload`.
    pub const fn new(kind: Kind, payload: u64) -> Id {
        debug_assert!(payload < K);
        Id((kind.tag() << 60) | payload)
    }

    /// `MetaCore.Id.chunked`.
    pub const fn chunked(kind: Kind, chunk: u64, offset: u64) -> Id {
        debug_assert!(chunk < N_CHUNKS && offset < CHUNK);
        Id::new(kind, chunk * CHUNK + offset)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    pub const fn is_inline(self) -> bool {
        self.0 >> 63 == 1
    }

    /// The kind, read from the ID alone (rule `id-kind-tag`).
    pub const fn kind(self) -> Kind {
        if self.is_inline() {
            return Kind::InlineLiteral;
        }
        match self.0 >> 60 {
            0 => Kind::Null,
            1 => Kind::Iri,
            2 => Kind::Blank,
            3 => Kind::DictLiteral,
            4 => Kind::Statement,
            5 => Kind::Graph,
            6 => Kind::Hyperedge,
            _ => Kind::System,
        }
    }

    pub const fn payload(self) -> u64 {
        self.0 % K
    }

    /// `MetaCore.Id.chunkOf`.
    pub const fn chunk(self) -> u64 {
        self.payload() / CHUNK
    }

    /// `MetaCore.Id.offsetOf`.
    pub const fn offset(self) -> u64 {
        self.payload() % CHUNK
    }

    pub const fn is_literal(self) -> bool {
        self.is_inline() || matches!(self.kind(), Kind::DictLiteral)
    }

    pub const fn is_owned_literal(self) -> bool {
        matches!(self.kind(), Kind::DictLiteral) && self.0 & OWNED_BIT != 0
    }

    /// Kinds that may appear as a statement subject (rule `dm-term-positions`).
    pub const fn can_be_subject(self) -> bool {
        matches!(
            self.kind(),
            Kind::Iri
                | Kind::Blank
                | Kind::Statement
                | Kind::Graph
                | Kind::Hyperedge
                | Kind::System
        )
    }

    /// Big-endian bytes, the key form (rule `pl-key-be`).
    pub const fn be(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }

    pub fn from_be(b: &[u8]) -> Id {
        Id(u64::from_be_bytes(b[..8].try_into().expect("8 bytes")))
    }

    /// Little-endian bytes, the record form (rule `pl-value-le`).
    pub const fn le_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    pub fn from_le(b: &[u8]) -> Id {
        Id(u64::from_le_bytes(b[..8].try_into().expect("8 bytes")))
    }

    /// Class `11` subtype of an inline literal that is not a double.
    pub fn subtype(self) -> Option<Subtype> {
        if self.0 >> 62 != 0b11 {
            return None;
        }
        Some(match (self.0 >> 58) & 0xF {
            0 => Subtype::Integer,
            1 => Subtype::Decimal,
            2 => Subtype::Boolean,
            3 => Subtype::DateTime,
            4 => Subtype::Date,
            5 => Subtype::String,
            6 => Subtype::LangString,
            _ => return None,
        })
    }

    pub const fn is_inline_double(self) -> bool {
        self.0 >> 62 == 0b10
    }
}

/// `MetaCore.Id.lit`: class `11`, subtype, 58-bit payload.
pub const fn lit(sub: Subtype, payload: u64) -> Id {
    debug_assert!(payload < (1 << 58));
    Id((0b11 << 62) | ((sub as u64) << 58) | payload)
}

const P58: u64 = (1 << 58) - 1;

// ---- integers (rule `id-inline-range`) -------------------------------------

pub const INT_MIN: i64 = -B57;
pub const INT_MAX: i64 = B57 - 1;

/// `MetaCore.Id.encInt`.
pub fn enc_int(x: i64) -> Option<Id> {
    if !(INT_MIN..=INT_MAX).contains(&x) {
        return None;
    }
    let id = lit(Subtype::Integer, (x + B57) as u64);
    // Assumption "Kind tag and ID injectivity" in [[20-proofs]]: equals `MetaCore.Id.encInt`.
    debug_assert_eq!(dec_int(id), x);
    Some(id)
}

/// `MetaCore.Id.decInt`.
pub fn dec_int(id: Id) -> i64 {
    (id.0 & P58) as i64 - B57
}

// ---- decimals: value × 10^9 as a signed 58-bit integer ----------------------

pub const DEC_SCALE: i128 = 1_000_000_000;

pub fn enc_decimal(scaled: i128) -> Option<Id> {
    if scaled < INT_MIN as i128 || scaled > INT_MAX as i128 {
        return None;
    }
    Some(lit(Subtype::Decimal, (scaled as i64 + B57) as u64))
}

pub fn dec_decimal(id: Id) -> i128 {
    ((id.0 & P58) as i64 - B57) as i128
}

// ---- booleans --------------------------------------------------------------

pub const fn enc_bool(b: bool) -> Id {
    lit(Subtype::Boolean, b as u64)
}

pub const fn dec_bool(id: Id) -> bool {
    id.0 & 1 == 1
}

// ---- dateTime and date: signed 57-bit instant, then a has-timezone bit -----

pub fn enc_instant(sub: Subtype, t: i64, tz: bool) -> Option<Id> {
    if !(-B56..B56).contains(&t) {
        return None;
    }
    Some(lit(sub, (((t + B56) as u64) << 1) | tz as u64))
}

pub fn dec_instant(id: Id) -> (i64, bool) {
    let p = id.0 & P58;
    (((p >> 1) as i64) - B56, p & 1 == 1)
}

// ---- strings: up to 7 bytes, big-endian, zero padded, in bits 57..2 --------

/// Packs `bs` big-endian into `width` bytes, zero padded on the right.
fn pack(bs: &[u8], width: usize) -> u64 {
    let mut v = 0u64;
    for i in 0..width {
        v = (v << 8) | *bs.get(i).unwrap_or(&0) as u64;
    }
    v
}

fn unpack(v: u64, width: usize) -> Vec<u8> {
    let mut out: Vec<u8> = (0..width)
        .map(|i| (v >> (8 * (width - 1 - i))) as u8)
        .collect();
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

/// Inline string (subtype `0x5`). Strings ending in a NUL byte are not inline,
/// because zero padding would make `s` and `s ++ [0]` collide (PO-ID-STRING).
pub fn enc_string(s: &str) -> Option<Id> {
    let b = s.as_bytes();
    if b.len() > 7 || b.last() == Some(&0) {
        return None;
    }
    let id = lit(Subtype::String, pack(b, 7) << 2);
    // Assumption of "Inline string order" in [[20-proofs]] (`packN 7`, no trailing 0x00).
    debug_assert!(dec_string(id).as_bytes() == b);
    Some(id)
}

pub fn dec_string(id: Id) -> String {
    let bytes = unpack((id.0 & P58) >> 2, 7);
    String::from_utf8(bytes).unwrap_or_default()
}

/// Inline language string (subtype `0x6`): 5 bytes then a 16-bit language ID.
pub fn enc_lang_string(s: &str, lang: u16) -> Option<Id> {
    let b = s.as_bytes();
    if b.len() > 5 || b.last() == Some(&0) || lang == 0 {
        return None;
    }
    Some(lit(
        Subtype::LangString,
        ((pack(b, 5) << 16) | lang as u64) << 2,
    ))
}

pub fn dec_lang_string(id: Id) -> (String, u16) {
    let p = (id.0 & P58) >> 2;
    let text = String::from_utf8(unpack(p >> 16, 5)).unwrap_or_default();
    (text, (p & 0xFFFF) as u16)
}

// ---- doubles (rule `id-double-inline`) -------------------------------------

/// The 61-bit magnitude field of a non-negative double: rebiased exponent
/// (`e - 767`) in 9 bits followed by the 52 mantissa bits; 0 for zero and
/// `511 << 52` for infinity. `None` outside the inline range (NaN, subnormals,
/// unbiased exponents outside [-255, 254]).
pub fn double_magnitude(abs_bits: u64) -> Option<u64> {
    let e = abs_bits >> 52;
    let m = abs_bits & ((1 << 52) - 1);
    match (e, m) {
        (0, 0) => Some(0),
        (2047, 0) => Some(511 << 52),
        (768..=1277, _) => Some(((e - 767) << 52) | m),
        _ => None,
    }
}

fn magnitude_bits(mag: u64) -> u64 {
    let r = mag >> 52;
    let m = mag & ((1 << 52) - 1);
    match r {
        0 => 0,
        511 => 2047 << 52,
        _ => ((r + 767) << 52) | m,
    }
}

/// `MetaCore.Id.encDbl` applied to the magnitude field of `x`.
pub fn enc_double(x: f64) -> Option<Id> {
    let bits = x.to_bits();
    let neg = bits >> 63 == 1;
    let mag = double_magnitude(bits & !(1 << 63))?;
    // Assumption of "Inline double order" in [[20-proofs]]: rebiased exponent in 0..=511, 1..=510 for normals.
    debug_assert!(
        mag < D61 && (mag >> 52 == 0 || mag >> 52 == 511 || (1..=510).contains(&(mag >> 52)))
    );
    let payload = if neg { D61 - 1 - mag } else { D61 + mag };
    Some(Id((0b10 << 62) | payload))
}

pub fn dec_double(id: Id) -> f64 {
    let payload = id.0 & ((1 << 62) - 1);
    let (neg, mag) = if payload >= D61 {
        (false, payload - D61)
    } else {
        (true, D61 - 1 - payload)
    };
    f64::from_bits(magnitude_bits(mag) | ((neg as u64) << 63))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // Arithmetic definitions copied from formal/MetaCore/Id.lean, on u128 so
    // nothing wraps; the Rust bit operations must agree with them.
    const QK: u128 = 1 << 60;
    const QQ: u128 = 1 << 62;
    const QS: u128 = 1 << 58;
    const QB: i128 = 1 << 57;
    const QD: u128 = 1 << 61;
    fn model_mk(k: u128, p: u128) -> u128 {
        k * QK + p
    }
    fn model_lit(sub: u128, p: u128) -> u128 {
        3 * QQ + sub * QS + p
    }
    fn model_enc_int(x: i128) -> u128 {
        model_lit(0, (x + QB) as u128)
    }
    fn model_enc_dbl(neg: bool, m: u128) -> u128 {
        2 * QQ + if neg { QD - 1 - m } else { QD + m }
    }

    proptest! {
        // @tg: verifies:: [[openspec:term-identity#Stable term identity#Kind is readable from the ID]] {level: property}
        #[test]
        fn id_layout_matches_model(kind in 1u64..8, payload in 0u64..K, x in INT_MIN..=INT_MAX, bits in any::<u64>()) {
            let k = match kind { 1 => Kind::Iri, 2 => Kind::Blank, 3 => Kind::DictLiteral, 4 => Kind::Statement, 5 => Kind::Graph, 6 => Kind::Hyperedge, _ => Kind::System };
            let id = Id::new(k, payload);
            prop_assert_eq!(id.0 as u128, model_mk(kind as u128, payload as u128));
            prop_assert_eq!(id.kind(), k);
            prop_assert_eq!(id.payload(), payload);
            prop_assert_eq!(enc_int(x).unwrap().0 as u128, model_enc_int(x as i128));
            if let Some(d) = enc_double(f64::from_bits(bits)) {
                let neg = bits >> 63 == 1;
                let m = double_magnitude(bits & !(1 << 63)).unwrap() as u128;
                prop_assert!(m < QD);
                prop_assert_eq!(d.0 as u128, model_enc_dbl(neg, m));
            }
        }

        #[test]
        fn chunk_split_roundtrip(c in 0u64..N_CHUNKS, o in 0u64..CHUNK) {
            let id = Id::chunked(Kind::Statement, c, o);
            prop_assert_eq!((id.chunk(), id.offset(), id.kind()), (c, o, Kind::Statement));
        }

        #[test]
        fn int_roundtrip_and_order(a in INT_MIN..=INT_MAX, b in INT_MIN..=INT_MAX) {
            prop_assert_eq!(dec_int(enc_int(a).unwrap()), a);
            prop_assert_eq!(enc_int(a).unwrap() < enc_int(b).unwrap(), a < b);
        }

        #[test]
        fn decimal_order(a in INT_MIN..=INT_MAX, b in INT_MIN..=INT_MAX) {
            let (ea, eb) = (enc_decimal(a as i128).unwrap(), enc_decimal(b as i128).unwrap());
            prop_assert_eq!(dec_decimal(ea), a as i128);
            prop_assert_eq!(ea < eb, a < b);
        }

        // Assumption of PO-ID-DOUBLE in tg.md/20-proofs.md.
        #[test]
        fn double_magnitude_order(a in any::<u64>(), b in any::<u64>()) {
            let (x, y) = (f64::from_bits(a), f64::from_bits(b));
            if let (Some(ex), Some(ey)) = (enc_double(x), enc_double(y)) {
                prop_assert_eq!(dec_double(ex).to_bits(), x.to_bits());
                // total order: value order, with -0.0 < +0.0
                let lt = x < y || (x == 0.0 && y == 0.0 && x.is_sign_negative() && y.is_sign_positive());
                prop_assert_eq!(ex < ey, lt);
            }
        }

        // Assumption of PO-ID-STRING.
        #[test]
        fn string_pack_order(a in proptest::collection::vec(1u8..=255, 0..=7), b in proptest::collection::vec(any::<u8>(), 0..=7)) {
            let mut b = b;
            while b.last() == Some(&0) { b.pop(); }
            let (sa, sb) = (String::from_utf8_lossy(&a).into_owned(), String::from_utf8_lossy(&b).into_owned());
            if let (Some(ia), Some(ib)) = (enc_string(&sa), enc_string(&sb)) {
                prop_assert_eq!(dec_string(ia), sa.clone());
                prop_assert_eq!(ia < ib, sa.as_bytes() < sb.as_bytes());
                prop_assert_eq!(ia == ib, sa == sb);
            }
        }

        #[test]
        fn instant_order(a in -B56..B56, b in -B56..B56, ta: bool, tb: bool) {
            let (ia, ib) = (enc_instant(Subtype::DateTime, a, ta).unwrap(), enc_instant(Subtype::DateTime, b, tb).unwrap());
            prop_assert_eq!(dec_instant(ia), (a, ta));
            if a != b { prop_assert_eq!(ia < ib, a < b); }
        }

        #[test]
        fn key_bytes_order_eq_u64_order(a in any::<u64>(), b in any::<u64>()) {
            prop_assert_eq!(Id(a).be() < Id(b).be(), a < b);
        }
    }

    #[test]
    fn numeric_runs_adjacent() {
        // double < integer < decimal (rule `id-numeric-adjacent`)
        let d = enc_double(f64::INFINITY).unwrap();
        let i = enc_int(INT_MIN).unwrap();
        let i2 = enc_int(INT_MAX).unwrap();
        let dec = enc_decimal(INT_MIN as i128).unwrap();
        assert!(d < i && i2 < dec);
        assert!(Id::new(Kind::System, K - 1) < enc_double(f64::NEG_INFINITY).unwrap());
    }

    #[test]
    fn double_range_edges() {
        assert!(enc_double(0.1).is_some());
        assert!(enc_double(1e-30).is_some());
        assert!(enc_double(5e76).is_some());
        assert!(enc_double(6e76).is_none());
        assert!(enc_double(1e-78).is_none());
        assert!(enc_double(f64::NAN).is_none());
        assert!(enc_double(f64::MIN_POSITIVE / 2.0).is_none());
        assert!(enc_double(-0.0).unwrap() < enc_double(0.0).unwrap());
    }

    #[test]
    fn system_graph_id() {
        assert_eq!(Id::chunked(Kind::Graph, 0, 0), SYSTEM_GRAPH);
        assert_eq!(Id::chunked(Kind::Graph, 0, 1), DEFAULT_GRAPH);
    }
}
