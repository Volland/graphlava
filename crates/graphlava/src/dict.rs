//! The term dictionary ([[03-dictionary]], [[adr:0016]], [[adr:0023]], [[adr:0033]]).
//!
//! Interning runs inside the caller's write transaction (rule
//! `dict-intern-in-txn`); read transactions only look up. No in-memory term
//! cache is kept yet, so an aborted request can never leave a cached entry
//! behind (rule `dict-cache`).

// @tg: implements:: [[openspec:term-identity#Dictionary reclamation without write cost]] {confidence: partial}

use xxhash_rust::xxh3::xxh3_128;

use crate::error::{Result, corrupted, invalid};
use crate::id::{self, Id, Kind, OWNED_BIT};
use crate::literal::{self, Canon};
use crate::storage::{KvRead, KvWrite};
use crate::tables::{self as tb, mk};
use crate::term::{Literal, RDF_LANG_STRING, Term, xsd};

/// Encoded keys longer than this are stored as a hash (rule `dict-key-hash`).
pub const HASH_KEY_LIMIT: usize = 256;
const KIND_IRI: u8 = 0x01;
const KIND_LIT: u8 = 0x03;
const KIND_HASHED: u8 = 0xFF;
const FLAG_OWNED: u8 = 0b10;

/// Per-file dictionary settings, fixed in `meta` at creation.
#[derive(Debug, Clone, Copy)]
pub struct DictConfig {
    /// Literals whose encoded key is longer than this are owned (rule `dict-owned-threshold`).
    pub owned_threshold: usize,
    /// Maximum namespace entries (rule `dict-ns-cap`).
    pub ns_cap: u32,
}

impl Default for DictConfig {
    fn default() -> Self {
        DictConfig {
            owned_threshold: 256,
            ns_cap: 65_536,
        }
    }
}

/// Splits an IRI at the last `#`, `/` or `:` that leaves a non-empty local name
/// (rule `dict-prefix-split`).
pub fn split_iri(iri: &str) -> (&str, &str) {
    let b = iri.as_bytes();
    for i in (0..b.len().saturating_sub(1)).rev() {
        if matches!(b[i], b'#' | b'/' | b':') {
            return (&iri[..=i], &iri[i + 1..]);
        }
    }
    ("", iri)
}

fn next_counter(kv: &mut dyn KvWrite, key: &str) -> Result<u64> {
    let v = tb::meta_u64(kv.as_read(), key)?.unwrap_or(1);
    tb::meta_put_u64(kv, key, v + 1)?;
    Ok(v)
}

/// A small append-only name table (namespaces, datatypes, language tags).
struct NameTable {
    fwd: &'static crate::storage::TableDef,
    rev: &'static crate::storage::TableDef,
    count: &'static str,
    width: usize,
}

const NS: NameTable = NameTable {
    fwd: &tb::NS_FWD,
    rev: &tb::NS_REV,
    count: mk::NS_COUNT,
    width: 4,
};
const DT: NameTable = NameTable {
    fwd: &tb::DT_FWD,
    rev: &tb::DT_REV,
    count: mk::DT_COUNT,
    width: 4,
};
const LANG: NameTable = NameTable {
    fwd: &tb::LANG_FWD,
    rev: &tb::LANG_REV,
    count: mk::LANG_COUNT,
    width: 2,
};

impl NameTable {
    fn decode(&self, v: &[u8]) -> u32 {
        let mut b = [0u8; 4];
        b[..self.width].copy_from_slice(&v[..self.width]);
        u32::from_le_bytes(b)
    }

    fn rev_key(&self, id: u32) -> Vec<u8> {
        id.to_be_bytes()[4 - self.width..].to_vec()
    }

    fn lookup(&self, kv: &dyn KvRead, name: &str) -> Result<Option<u32>> {
        Ok(kv.get(self.fwd, name.as_bytes())?.map(|v| self.decode(&v)))
    }

    /// Interns `name`; `None` when the table is at `cap`.
    fn intern(&self, kv: &mut dyn KvWrite, name: &str, cap: u32) -> Result<Option<u32>> {
        if let Some(id) = self.lookup(kv.as_read(), name)? {
            return Ok(Some(id));
        }
        let n = tb::meta_u64(kv.as_read(), self.count)?.unwrap_or(0);
        if n >= cap as u64 {
            return Ok(None);
        }
        let id = (n + 1) as u32;
        kv.put(self.fwd, name.as_bytes(), &id.to_le_bytes()[..self.width])?;
        kv.put(self.rev, &self.rev_key(id), name.as_bytes())?;
        tb::meta_put_u64(kv, self.count, n + 1)?;
        Ok(Some(id))
    }

    fn name(&self, kv: &dyn KvRead, id: u32) -> Result<String> {
        if id == 0 {
            return Ok(String::new());
        }
        let v = kv
            .get(self.rev, &self.rev_key(id))?
            .ok_or_else(|| corrupted(format!("missing name {id} in {}", self.rev.name)))?;
        String::from_utf8(v).map_err(|_| corrupted("name table holds invalid UTF-8"))
    }
}

/// The `dict_fwd` key of a term (Encoded term key table).
fn encoded_key(kind: u8, ns_or_dt: u32, lang: u16, body: &str) -> Vec<u8> {
    let mut k = Vec::with_capacity(7 + body.len());
    k.push(kind);
    k.extend_from_slice(&ns_or_dt.to_be_bytes());
    k.extend_from_slice(&lang.to_be_bytes());
    k.extend_from_slice(body.as_bytes());
    k
}

fn hashed(full: &[u8], disambiguator: Option<u8>) -> Vec<u8> {
    let mut k = vec![KIND_HASHED];
    k.extend_from_slice(&xxh3_128(full).to_be_bytes());
    if let Some(d) = disambiguator {
        k.push(d);
    }
    k
}

/// A decoded `dict_rev` record.
#[derive(Debug, Clone)]
pub struct Record {
    pub kind: u8,
    pub flags: u8,
    pub ns_or_dt: u32,
    pub lang: u16,
    pub created_tx: u64,
    pub hash: Option<u128>,
    pub body: String,
}

impl Record {
    fn encode(&self) -> Vec<u8> {
        let mut v = vec![self.kind, self.flags];
        v.extend_from_slice(&self.ns_or_dt.to_le_bytes());
        v.extend_from_slice(&self.lang.to_le_bytes());
        v.extend_from_slice(&self.created_tx.to_le_bytes());
        if let Some(h) = self.hash {
            v.extend_from_slice(&h.to_le_bytes());
        }
        v.extend_from_slice(self.body.as_bytes());
        v
    }

    pub fn decode(v: &[u8]) -> Result<Record> {
        if v.len() < 16 {
            return Err(corrupted("short dictionary record"));
        }
        let flags = v[1];
        let (hash, body) = if flags & FLAG_OWNED != 0 {
            if v.len() < 32 {
                return Err(corrupted("short owned record"));
            }
            (
                Some(u128::from_le_bytes(v[16..32].try_into().expect("16"))),
                &v[32..],
            )
        } else {
            (None, &v[16..])
        };
        Ok(Record {
            kind: v[0],
            flags,
            ns_or_dt: u32::from_le_bytes(v[2..6].try_into().expect("4")),
            lang: u16::from_le_bytes(v[6..8].try_into().expect("2")),
            created_tx: u64::from_le_bytes(v[8..16].try_into().expect("8")),
            hash,
            body: String::from_utf8(body.to_vec())
                .map_err(|_| corrupted("dictionary body is not UTF-8"))?,
        })
    }

    /// The full encoded key this record was interned under.
    pub fn full_key(&self) -> Vec<u8> {
        encoded_key(self.kind, self.ns_or_dt, self.lang, &self.body)
    }
}

/// The dictionary bound to one view.
#[derive(Debug, Clone, Copy)]
pub struct Dict {
    pub cfg: DictConfig,
}

/// What a literal resolves to before allocation.
enum LitPlan {
    Inline(Id),
    Interned {
        dt: Option<u32>,
        lang: u16,
        body: String,
    },
}

impl Dict {
    pub fn new(cfg: DictConfig) -> Self {
        Dict { cfg }
    }

    fn created_tx(kv: &dyn KvRead) -> Result<u64> {
        Ok(tb::meta_u64(kv, mk::COMMIT)?.unwrap_or(0))
    }

    /// Looks a full key up in `dict_fwd`, resolving hashed keys.
    fn fwd_lookup(&self, kv: &dyn KvRead, full: &[u8]) -> Result<Option<Id>> {
        if full.len() <= HASH_KEY_LIMIT {
            return Ok(kv.get(&tb::DICT_FWD, full)?.map(|v| Id::from_le(&v)));
        }
        for d in std::iter::once(None).chain((0..=255u8).map(Some)) {
            let k = hashed(full, d);
            match kv.get(&tb::DICT_FWD, &k)? {
                None => return Ok(None),
                Some(v) => {
                    let id = Id::from_le(&v);
                    let rec = self
                        .record(kv, id)?
                        .ok_or_else(|| corrupted("dict_fwd without dict_rev"))?;
                    if rec.full_key() == full {
                        return Ok(Some(id));
                    }
                }
            }
        }
        Err(corrupted("more than 256 hash collisions on one key"))
    }

    /// The key under which a new term with this full key is stored.
    fn fwd_slot(&self, kv: &dyn KvRead, full: &[u8]) -> Result<Vec<u8>> {
        if full.len() <= HASH_KEY_LIMIT {
            return Ok(full.to_vec());
        }
        for d in std::iter::once(None).chain((0..=255u8).map(Some)) {
            let k = hashed(full, d);
            if kv.get(&tb::DICT_FWD, &k)?.is_none() {
                return Ok(k);
            }
        }
        Err(corrupted("more than 256 hash collisions on one key"))
    }

    fn insert_new(&self, kv: &mut dyn KvWrite, kind: Kind, ctr: &str, rec: Record) -> Result<Id> {
        let full = rec.full_key();
        let slot = self.fwd_slot(kv.as_read(), &full)?;
        let n = next_counter(kv, ctr)?;
        let id = Id::new(kind, n);
        kv.put(&tb::DICT_FWD, &slot, &id.le_bytes())?;
        kv.put(&tb::DICT_REV, &id.be(), &rec.encode())?;
        Ok(id)
    }

    pub fn record(&self, kv: &dyn KvRead, id: Id) -> Result<Option<Record>> {
        kv.get(&tb::DICT_REV, &id.be())?
            .map(|v| Record::decode(&v))
            .transpose()
    }

    // ---- IRIs -----------------------------------------------------------

    fn iri_key(&self, kv: &dyn KvRead, iri: &str) -> Result<Option<Vec<u8>>> {
        let (ns, local) = split_iri(iri);
        if ns.is_empty() {
            return Ok(Some(encoded_key(KIND_IRI, 0, 0, iri)));
        }
        match NS.lookup(kv, ns)? {
            Some(n) => Ok(Some(encoded_key(KIND_IRI, n, 0, local))),
            // The namespace may be absent because the cap was hit: then the
            // IRI was stored whole under namespace 0.
            None => Ok(Some(encoded_key(KIND_IRI, 0, 0, iri))),
        }
    }

    pub fn lookup_iri(&self, kv: &dyn KvRead, iri: &str) -> Result<Option<Id>> {
        match self.iri_key(kv, iri)? {
            Some(k) => self.fwd_lookup(kv, &k),
            None => Ok(None),
        }
    }

    pub fn intern_iri(&self, kv: &mut dyn KvWrite, iri: &str) -> Result<Id> {
        if let Some(id) = self.lookup_iri(kv.as_read(), iri)? {
            return Ok(id);
        }
        let (ns, local) = split_iri(iri);
        let (ns_id, body) = match if ns.is_empty() {
            None
        } else {
            NS.intern(kv, ns, self.cfg.ns_cap)?
        } {
            Some(n) => (n, local.to_string()),
            None => (0, iri.to_string()),
        };
        let rec = Record {
            kind: KIND_IRI,
            flags: 0,
            ns_or_dt: ns_id,
            lang: 0,
            created_tx: Self::created_tx(kv.as_read())?,
            hash: None,
            body,
        };
        self.insert_new(kv, Kind::Iri, mk::CTR_IRI, rec)
    }

    // ---- literals ---------------------------------------------------------

    fn plan_literal(
        &self,
        kv: &mut dyn KvWrite,
        lit: &Literal,
        create: bool,
    ) -> Result<Option<LitPlan>> {
        let canon = literal::canonicalize(lit);
        let (body, datatype, tag) = match canon {
            Canon::Inline(id) => return Ok(Some(LitPlan::Inline(id))),
            Canon::LangCandidate { text, tag } => {
                let lang = if create {
                    LANG.intern(kv, &tag, u16::MAX as u32)?
                } else {
                    LANG.lookup(kv.as_read(), &tag)?
                };
                match lang {
                    Some(l) => {
                        return Ok(Some(LitPlan::Inline(
                            id::enc_lang_string(&text, l as u16).expect("checked by canonicalize"),
                        )));
                    }
                    None if !create => return Ok(None),
                    None => (text, RDF_LANG_STRING.to_string(), Some(tag)),
                }
            }
            Canon::Dict {
                lexical,
                datatype,
                lang,
            } => (lexical, datatype, lang),
        };
        let lang = match &tag {
            Some(t) => match if create {
                LANG.intern(kv, t, u16::MAX as u32)?
            } else {
                LANG.lookup(kv.as_read(), t)?
            } {
                Some(l) => l as u16,
                // Tag table full: the literal keeps its tag in the body.
                None => 0,
            },
            None => 0,
        };
        let body = if tag.is_some() && lang == 0 {
            format!("{}@{}", body, tag.as_deref().unwrap_or(""))
        } else {
            body
        };
        let dt = if create {
            DT.intern(kv, &datatype, u32::MAX)?
        } else {
            DT.lookup(kv.as_read(), &datatype)?
        };
        Ok(Some(LitPlan::Interned { dt, lang, body }))
    }

    /// Looks a literal up without creating anything. Owned literals are never
    /// found this way: they have no `dict_fwd` entry.
    pub fn lookup_literal(&self, kv: &dyn KvRead, lit: &Literal) -> Result<Option<Id>> {
        // plan_literal needs a writer only to intern; with create = false it
        // only reads, so a read-only shim is enough.
        let mut ro = ReadOnly(kv);
        match self.plan_literal(&mut ro, lit, false)? {
            None => Ok(None),
            Some(LitPlan::Inline(id)) => Ok(Some(id)),
            Some(LitPlan::Interned { dt: None, .. }) => Ok(None),
            Some(LitPlan::Interned {
                dt: Some(dt),
                lang,
                body,
                ..
            }) => self.fwd_lookup(kv, &encoded_key(KIND_LIT, dt, lang, &body)),
        }
    }

    /// Whether the literal would be owned (and so must only be created by the
    /// slot that holds it).
    pub fn is_owned_size(&self, lit: &Literal) -> bool {
        match literal::canonicalize(lit) {
            Canon::Inline(_) | Canon::LangCandidate { .. } => false,
            Canon::Dict { lexical, .. } => 7 + lexical.len() > self.cfg.owned_threshold,
        }
    }

    /// Interns a literal; above the owned threshold this creates a new owned
    /// literal for one slot (rule `dict-owned-lifetime`).
    pub fn intern_literal(&self, kv: &mut dyn KvWrite, lit: &Literal) -> Result<Id> {
        let Some(plan) = self.plan_literal(kv, lit, true)? else {
            return invalid("literal could not be planned");
        };
        let (dt, lang, body) = match plan {
            LitPlan::Inline(id) => return Ok(id),
            LitPlan::Interned { dt, lang, body, .. } => (dt.unwrap_or(0), lang, body),
        };
        let full = encoded_key(KIND_LIT, dt, lang, &body);
        let created_tx = Self::created_tx(kv.as_read())?;
        if full.len() > self.cfg.owned_threshold {
            let n = next_counter(kv, mk::CTR_LIT)?;
            if n >= OWNED_BIT {
                return invalid("dictionary literal counter exhausted");
            }
            let id = Id(Id::new(Kind::DictLiteral, n).raw() | OWNED_BIT);
            let rec = Record {
                kind: KIND_LIT,
                flags: FLAG_OWNED,
                ns_or_dt: dt,
                lang,
                created_tx,
                hash: Some(xxh3_128(&full)),
                body,
            };
            kv.put(&tb::DICT_REV, &id.be(), &rec.encode())?;
            return Ok(id);
        }
        if let Some(id) = self.fwd_lookup(kv.as_read(), &full)? {
            return Ok(id);
        }
        let rec = Record {
            kind: KIND_LIT,
            flags: 0,
            ns_or_dt: dt,
            lang,
            created_tx,
            hash: None,
            body,
        };
        self.insert_new(kv, Kind::DictLiteral, mk::CTR_LIT, rec)
    }

    /// Deletes an owned literal with its slot (rule `dict-owned-lifetime`).
    pub fn drop_owned(&self, kv: &mut dyn KvWrite, id: Id) -> Result<()> {
        if id.is_owned_literal() {
            kv.del(&tb::DICT_REV, &id.be())?;
        }
        Ok(())
    }

    /// Allocates a fresh blank node. Blank nodes have no dictionary entry: the
    /// label is derived from the ID (rule `dm-blank-scope`).
    pub fn new_blank(&self, kv: &mut dyn KvWrite) -> Result<Id> {
        Ok(Id::new(Kind::Blank, next_counter(kv, mk::CTR_BLANK)?))
    }

    // ---- terms ------------------------------------------------------------

    pub fn lookup(&self, kv: &dyn KvRead, t: &Term) -> Result<Option<Id>> {
        match t {
            Term::Iri(i) => self.lookup_iri(kv, i),
            Term::Literal(l) => self.lookup_literal(kv, l),
            Term::Blank(label) => Ok(parse_blank_label(label)),
            // A triple term's ID is its statement's, when one is stored.
            Term::Triple(t) => {
                let (Some(s), Some(p), Some(o)) = (
                    self.lookup(kv, &t.subject)?,
                    self.lookup(kv, &t.predicate)?,
                    self.lookup(kv, &t.object)?,
                ) else {
                    return Ok(None);
                };
                crate::store::stmt_lookup(kv, s, p, o)
            }
        }
    }

    /// Decodes any ID to its term (rule `api-rows-late`: callers decode only
    /// what they project).
    pub fn decode(&self, kv: &dyn KvRead, id: Id) -> Result<Term> {
        match id.kind() {
            Kind::InlineLiteral => {
                if let Some(l) = literal::decode_inline(id) {
                    return Ok(Term::Literal(l));
                }
                let (text, lang) = id::dec_lang_string(id);
                Ok(Term::Literal(Literal::lang(
                    text,
                    LANG.name(kv, lang as u32)?,
                )))
            }
            Kind::Blank => Ok(Term::Blank(format!("b{}", id.payload()))),
            Kind::Iri | Kind::DictLiteral => {
                let rec = self
                    .record(kv, id)?
                    .ok_or(crate::error::Error::NotFound(id.raw()))?;
                if rec.kind == KIND_IRI {
                    let ns = NS.name(kv, rec.ns_or_dt)?;
                    return Ok(Term::Iri(format!("{ns}{}", rec.body)));
                }
                let datatype = DT.name(kv, rec.ns_or_dt)?;
                if datatype == RDF_LANG_STRING {
                    if rec.lang == 0 {
                        let (text, tag) = rec.body.rsplit_once('@').unwrap_or((&rec.body, ""));
                        return Ok(Term::Literal(Literal::lang(text, tag)));
                    }
                    return Ok(Term::Literal(Literal::lang(
                        rec.body,
                        LANG.name(kv, rec.lang as u32)?,
                    )));
                }
                Ok(Term::Literal(Literal::typed(
                    rec.body,
                    if datatype.is_empty() {
                        xsd("string")
                    } else {
                        datatype
                    },
                )))
            }
            Kind::Graph => {
                // A graph decodes to its name ([[22-implementation-decisions#Graph names]]).
                if id == crate::id::DEFAULT_GRAPH {
                    return Ok(Term::Iri(crate::term::DEFAULT_GRAPH_IRI.into()));
                }
                if id == crate::id::SYSTEM_GRAPH {
                    return Ok(Term::Iri(crate::term::SYSTEM_GRAPH_IRI.into()));
                }
                match kv.get(&crate::tables::GNAME_REV, &id.be())? {
                    Some(v) => self.decode(kv, Id::from_le(&v)),
                    None => Err(crate::error::Error::NotFound(id.raw())),
                }
            }
            Kind::Statement | Kind::Hyperedge | Kind::System => Ok(Term::Iri(format!(
                "{}{}",
                crate::term::STMT_IRI_PREFIX,
                id.raw()
            ))),
            Kind::Null => Err(crate::error::Error::NotFound(0)),
        }
    }

    /// Literal equality including owned literals (rule `dict-owned-equality`).
    pub fn literal_eq(&self, kv: &dyn KvRead, a: Id, b: Id) -> Result<bool> {
        if a == b {
            return Ok(true);
        }
        if !(a.is_owned_literal() || b.is_owned_literal()) {
            return Ok(false);
        }
        if !(a.kind() == Kind::DictLiteral && b.kind() == Kind::DictLiteral) {
            return Ok(false);
        }
        let (ra, rb) = match (self.record(kv, a)?, self.record(kv, b)?) {
            (Some(x), Some(y)) => (x, y),
            _ => return Ok(false),
        };
        let ha = ra.hash.unwrap_or_else(|| xxh3_128(&ra.full_key()));
        let hb = rb.hash.unwrap_or_else(|| xxh3_128(&rb.full_key()));
        Ok(ha == hb && ra.full_key() == rb.full_key())
    }

    /// The language ID of a tag, if it has one.
    pub fn lang_id(&self, kv: &dyn KvRead, tag: &str) -> Result<Option<u16>> {
        Ok(LANG
            .lookup(kv, &tag.to_ascii_lowercase())?
            .map(|v| v as u16))
    }
}

fn parse_blank_label(label: &str) -> Option<Id> {
    let n: u64 = label.strip_prefix('b')?.parse().ok()?;
    (n > 0 && n < id::K).then(|| Id::new(Kind::Blank, n))
}

/// A writer view that refuses writes, for lookups through write-capable code.
struct ReadOnly<'a>(&'a dyn KvRead);

impl KvRead for ReadOnly<'_> {
    fn get(&self, t: &'static crate::storage::TableDef, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.0.get(t, key)
    }
    fn scan(
        &self,
        t: &'static crate::storage::TableDef,
        lo: std::ops::Bound<&[u8]>,
        hi: std::ops::Bound<&[u8]>,
        rev: bool,
        limit: usize,
    ) -> Result<Vec<crate::storage::Entry>> {
        self.0.scan(t, lo, hi, rev, limit)
    }
}

impl KvWrite for ReadOnly<'_> {
    fn as_read(&self) -> &dyn KvRead {
        self
    }
    fn put(&mut self, t: &'static crate::storage::TableDef, _: &[u8], _: &[u8]) -> Result<()> {
        invalid(format!("write to {} from a read-only lookup", t.name))
    }
    fn del(&mut self, t: &'static crate::storage::TableDef, _: &[u8]) -> Result<bool> {
        invalid(format!("delete in {} from a read-only lookup", t.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split() {
        assert_eq!(
            split_iri("http://ex.org/alice"),
            ("http://ex.org/", "alice")
        );
        assert_eq!(split_iri("http://ex.org/a#b"), ("http://ex.org/a#", "b"));
        assert_eq!(split_iri("urn:x:y"), ("urn:x:", "y"));
        assert_eq!(split_iri("http://ex.org/"), ("http://", "ex.org/"));
        assert_eq!(split_iri("abc"), ("", "abc"));
    }
}
