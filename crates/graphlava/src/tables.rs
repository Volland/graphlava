//! The table inventory of the v1 encoding ([[04-physical-layout#Tables]],
//! [[03-dictionary#Layout]]). Keys are big-endian, values little-endian.

use crate::error::{Result, corrupted};
use crate::id::Id;
use crate::storage::Width::{Fixed, Var};
use crate::storage::{KvRead, KvWrite, TableDef};

macro_rules! table {
    ($c:ident, $n:literal, $k:expr, $v:expr) => {
        pub static $c: TableDef = TableDef {
            name: $n,
            key: $k,
            value: $v,
        };
    };
}

// base tables
table!(SPO, "spo", Fixed(24), Fixed(8));
table!(POS, "pos", Fixed(24), Fixed(8));
table!(OSP, "osp", Fixed(24), Fixed(8));
table!(STMT, "stmt", Fixed(8), Fixed(26));
table!(PROPS, "props", Fixed(8), Var);
table!(PLIST, "plist", Fixed(16), Fixed(0));
table!(META, "meta", Var, Var);
// multi predicates
table!(SPO_M, "spo_m", Fixed(32), Fixed(0));
table!(POS_M, "pos_m", Fixed(32), Fixed(0));
table!(OSP_M, "osp_m", Fixed(32), Fixed(0));
// on-demand
table!(PSO, "pso", Fixed(24), Fixed(8));
table!(SOP, "sop", Fixed(24), Fixed(8));
table!(OPS, "ops", Fixed(24), Fixed(8));
table!(PSO_M, "pso_m", Fixed(32), Fixed(0));
table!(SOP_M, "sop_m", Fixed(32), Fixed(0));
table!(OPS_M, "ops_m", Fixed(32), Fixed(0));
table!(PROPIDX, "propidx", Fixed(32), Fixed(0));
table!(PROPS_EXT, "props_ext", Fixed(24), Fixed(0));
// unasserted statements ([[adr:0032]]): (s, p, o) -> id
table!(UNASSERTED, "unasserted", Fixed(24), Fixed(8));
// allocation
table!(GCHUNK, "gchunk", Fixed(8), Fixed(16));
table!(KIND_CHUNK, "kind_chunk", Fixed(16), Fixed(0));
// dictionary
table!(NS_FWD, "ns_fwd", Var, Fixed(4));
table!(NS_REV, "ns_rev", Fixed(4), Var);
table!(DT_FWD, "dt_fwd", Var, Fixed(4));
table!(DT_REV, "dt_rev", Fixed(4), Var);
table!(LANG_FWD, "lang_fwd", Var, Fixed(2));
table!(LANG_REV, "lang_rev", Fixed(2), Var);
table!(DICT_FWD, "dict_fwd", Var, Fixed(8));
table!(DICT_REV, "dict_rev", Fixed(8), Var);
// graphs ([[09-graphs]]): chunk lists per graph, extra memberships and
// tombstones, the nesting DAG with its closure, and graph names
table!(GRAPH_CHUNKS, "graph_chunks", Fixed(24), Fixed(0));
table!(GMEM, "gmem", Fixed(16), Fixed(9));
table!(GMEM_REV, "gmem_rev", Fixed(16), Fixed(0));
table!(GPARENT, "gparent", Fixed(16), Fixed(0));
table!(GCHILD, "gchild", Fixed(16), Fixed(0));
table!(IN_STAR, "in_star", Fixed(16), Fixed(0));
table!(IN_STAR_REV, "in_star_rev", Fixed(16), Fixed(0));
table!(GNAME, "gname", Fixed(8), Fixed(8));
table!(GNAME_REV, "gname_rev", Fixed(8), Fixed(8));
// hyperedges ([[08-hyperedges]])
table!(HEDGE, "hedge", Fixed(8), Var);
table!(INCIDENCE, "incidence", Fixed(32), Fixed(0));
table!(HEDGE_PROMO, "hedge_promo", Fixed(24), Fixed(8));
// reifier aliases ([[adr:0032]]): alias IRI -> statement, and back
table!(ALIAS, "alias", Fixed(8), Fixed(8));
table!(ALIAS_REV, "alias_rev", Fixed(16), Fixed(0));
// sparse commit clock for the sweep safety window: commit (BE) -> unix micros (LE)
table!(TXCLOCK, "txclock", Fixed(8), Fixed(8));

/// A permutation of `(s, p, o)` stored by an ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perm {
    Spo,
    Pos,
    Osp,
    Pso,
    Sop,
    Ops,
}

impl Perm {
    pub const BASE: [Perm; 3] = [Perm::Spo, Perm::Pos, Perm::Osp];
    pub const ON_DEMAND: [Perm; 3] = [Perm::Pso, Perm::Sop, Perm::Ops];

    pub fn table(self, multi: bool) -> &'static TableDef {
        match (self, multi) {
            (Perm::Spo, false) => &SPO,
            (Perm::Pos, false) => &POS,
            (Perm::Osp, false) => &OSP,
            (Perm::Pso, false) => &PSO,
            (Perm::Sop, false) => &SOP,
            (Perm::Ops, false) => &OPS,
            (Perm::Spo, true) => &SPO_M,
            (Perm::Pos, true) => &POS_M,
            (Perm::Osp, true) => &OSP_M,
            (Perm::Pso, true) => &PSO_M,
            (Perm::Sop, true) => &SOP_M,
            (Perm::Ops, true) => &OPS_M,
        }
    }

    pub fn name(self) -> &'static str {
        self.table(false).name
    }

    pub fn from_name(n: &str) -> Option<Perm> {
        [
            Perm::Spo,
            Perm::Pos,
            Perm::Osp,
            Perm::Pso,
            Perm::Sop,
            Perm::Ops,
        ]
        .into_iter()
        .find(|p| p.name() == n)
    }

    /// Puts `(s, p, o)` in this permutation's order.
    pub fn arrange(self, s: Id, p: Id, o: Id) -> [Id; 3] {
        match self {
            Perm::Spo => [s, p, o],
            Perm::Pos => [p, o, s],
            Perm::Osp => [o, s, p],
            Perm::Pso => [p, s, o],
            Perm::Sop => [s, o, p],
            Perm::Ops => [o, p, s],
        }
    }

    /// Inverse of [`Perm::arrange`].
    pub fn spo(self, k: [Id; 3]) -> (Id, Id, Id) {
        match self {
            Perm::Spo => (k[0], k[1], k[2]),
            Perm::Pos => (k[2], k[0], k[1]),
            Perm::Osp => (k[1], k[2], k[0]),
            Perm::Pso => (k[1], k[0], k[2]),
            Perm::Sop => (k[0], k[2], k[1]),
            Perm::Ops => (k[2], k[1], k[0]),
        }
    }
}

pub fn key(ids: &[Id]) -> Vec<u8> {
    let mut k = Vec::with_capacity(ids.len() * 8);
    for i in ids {
        k.extend_from_slice(&i.be());
    }
    k
}

pub fn unkey3(k: &[u8]) -> [Id; 3] {
    [
        Id::from_be(&k[0..]),
        Id::from_be(&k[8..]),
        Id::from_be(&k[16..]),
    ]
}

// ---- meta helpers ----------------------------------------------------------

pub fn meta_get(kv: &dyn KvRead, k: &str) -> Result<Option<Vec<u8>>> {
    kv.get(&META, k.as_bytes())
}

pub fn meta_u64(kv: &dyn KvRead, k: &str) -> Result<Option<u64>> {
    match meta_get(kv, k)? {
        None => Ok(None),
        Some(v) if v.len() == 8 => Ok(Some(u64::from_le_bytes(v.try_into().expect("8")))),
        Some(_) => Err(corrupted(format!("meta {k} is not a u64"))),
    }
}

pub fn meta_put_u64(kv: &mut dyn KvWrite, k: &str, v: u64) -> Result<()> {
    kv.put(&META, k.as_bytes(), &v.to_le_bytes())
}

pub fn meta_put(kv: &mut dyn KvWrite, k: &str, v: &[u8]) -> Result<()> {
    kv.put(&META, k.as_bytes(), v)
}

/// Meta keys of the file format.
pub mod mk {
    pub const FORMAT: &str = "format_version";
    pub const FEATURES: &str = "features";
    pub const COMMIT: &str = "commit";
    pub const NEXT_CHUNK: &str = "next_chunk";
    pub const CTR_IRI: &str = "ctr:iri";
    pub const CTR_BLANK: &str = "ctr:blank";
    pub const CTR_LIT: &str = "ctr:lit";
    pub const NS_COUNT: &str = "ns_count";
    pub const DT_COUNT: &str = "dt_count";
    pub const LANG_COUNT: &str = "lang_count";
    pub const OWNED_THRESHOLD: &str = "owned_threshold";
    pub const NS_CAP: &str = "ns_cap";
    pub const SWEEP_CURSOR: &str = "sweep:cursor";
    pub const SWEEP_LAST_SIZE: &str = "sweep:last_size";
    /// IDs of the system IRIs interned with the file (`meta:in`, `rdf:type`).
    pub const SYS_IN: &str = "sys:in";
    pub const SYS_TYPE: &str = "sys:type";
    pub const DEPTH_LIMIT: &str = "graph_depth_limit";
}

/// Format version of this build ([[adr:0018]]).
pub const FORMAT_VERSION: u64 = 1;

/// Feature flags: a file that uses a feature sets its bit; a build refuses a
/// file with a bit it does not know (rule `tx-recovery-format`).
pub mod feature {
    pub const ON_DEMAND_ORDERINGS: u64 = 1 << 0;
    pub const VALUE_INDEX: u64 = 1 << 1;
    pub const PROPS_SPILL: u64 = 1 << 2;
    pub const UNASSERTED: u64 = 1 << 3;
    pub const MULTI: u64 = 1 << 4;
    pub const MEMBERSHIP: u64 = 1 << 5;
    pub const NESTING: u64 = 1 << 6;
    pub const HYPEREDGES: u64 = 1 << 7;
    pub const ALIASES: u64 = 1 << 8;
    pub const KNOWN: u64 = ON_DEMAND_ORDERINGS
        | VALUE_INDEX
        | PROPS_SPILL
        | UNASSERTED
        | MULTI
        | MEMBERSHIP
        | NESTING
        | HYPEREDGES
        | ALIASES;
}

pub fn set_feature(kv: &mut dyn KvWrite, bit: u64) -> Result<()> {
    let f = meta_u64(kv.as_read(), mk::FEATURES)?.unwrap_or(0);
    if f & bit == 0 {
        meta_put_u64(kv, mk::FEATURES, f | bit)?;
    }
    Ok(())
}
