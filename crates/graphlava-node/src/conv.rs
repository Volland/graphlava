//! Conversions between JavaScript values and graphlava types.
//!
//! The JS layer (`bindings/node/lib/terms.js`) maps host values (`string`,
//! `boolean`, `bigint`, `number`, `Date`, tagged objects) to the tagged
//! [`TermIn`] form and back from the tagged term objects built here, so the
//! XSD rules live in one place. This module checks IDs (unsigned 64-bit
//! `bigint`), builds [`CoreOp`]s and renders results as [`Out`] trees that
//! become JS values on the JavaScript thread.

use std::ffi::CString;

use graphlava::hedge::Hyperedge;
use graphlava::{
    CoreOp, DeleteMode, Durability, HedgeTypeDecl, Id, Lang, Literal, Match, OpResult,
    PredicateDecl, QueryResult, RoleSpec, Term, TermRef, Triple,
};
use napi::bindgen_prelude::{
    BigInt, BigUint64Array, Float64Array, Null, ToNapiValue, TypeName, ValidateNapiValue,
};
use napi::{ValueType, check_status, sys};
use napi_derive::napi;

use crate::error::{Fail, R};

// ---- inputs -----------------------------------------------------------------------

/// A term as the JS layer hands it over: `k` is `iri`, `blank`, `lit` or `id`.
#[napi(object)]
#[derive(Debug)]
pub struct TermIn {
    pub k: String,
    pub v: Option<String>,
    pub dt: Option<String>,
    pub lang: Option<String>,
    pub id: Option<BigInt>,
}

/// A role declaration of `declareHedgeType`.
#[napi(object)]
#[derive(Debug)]
pub struct RoleSpecIn {
    pub role: TermIn,
    pub many: Option<bool>,
    pub required: Option<bool>,
}

/// A `(role or attribute, value)` pair of `createHedge`.
#[napi(object)]
#[derive(Debug)]
pub struct PairIn {
    pub key: TermIn,
    pub value: TermIn,
}

/// One core operation, tagged by `op` (field names as in the JS API).
#[napi(object)]
#[derive(Debug)]
pub struct OpIn {
    pub op: String,
    pub s: Option<TermIn>,
    pub p: Option<TermIn>,
    pub o: Option<TermIn>,
    pub graph: Option<TermIn>,
    pub id: Option<BigInt>,
    pub mode: Option<String>,
    pub expected: Option<TermIn>,
    pub item: Option<BigInt>,
    pub from: Option<TermIn>,
    pub to: Option<TermIn>,
    pub name: Option<TermIn>,
    pub child: Option<TermIn>,
    pub parent: Option<TermIn>,
    pub recursive: Option<bool>,
    #[napi(js_name = "type")]
    pub ty: Option<TermIn>,
    pub role_specs: Option<Vec<RoleSpecIn>>,
    pub attr_decls: Option<Vec<TermIn>>,
    pub roles: Option<Vec<PairIn>>,
    pub attrs: Option<Vec<PairIn>>,
    pub hedge: Option<BigInt>,
    pub role: Option<TermIn>,
    pub filler: Option<TermIn>,
    pub node: Option<TermIn>,
    pub multi: Option<bool>,
    pub functional: Option<bool>,
}

/// A core read, tagged by `f`.
#[napi(object)]
#[derive(Debug)]
pub struct ReadIn {
    pub f: String,
    pub id: Option<BigInt>,
    pub s: Option<BigInt>,
    pub p: Option<BigInt>,
    pub o: Option<BigInt>,
    pub term: Option<TermIn>,
    pub direct: Option<bool>,
    pub ids: Option<Vec<BigInt>>,
}

/// Settings of `open`; durations in milliseconds.
#[napi(object)]
#[derive(Debug, Default)]
pub struct OpenIn {
    pub cache_bytes: Option<f64>,
    pub group_window_ms: Option<f64>,
    pub group_bytes: Option<f64>,
    pub write_timeout_ms: Option<f64>,
    pub cascade_limit: Option<f64>,
    pub graph_depth_limit: Option<u32>,
    pub max_relaxed_run: Option<f64>,
}

/// An administration request, tagged by `f`.
#[napi(object)]
#[derive(Debug)]
pub struct AdminIn {
    pub f: String,
    pub window_ms: Option<f64>,
    pub perm: Option<String>,
    pub threshold: Option<f64>,
    pub p: Option<TermIn>,
}

// @tg: implements:: [[openspec:node-binding#Node ID mapping]]
/// An ID from a JS `bigint`: the full unsigned 64-bit value, nothing else.
pub(crate) fn id_of(b: &BigInt, what: &str) -> R<Id> {
    let (neg, v, lossless) = b.get_u64();
    if (neg && v != 0) || !lossless {
        return Err(Fail::invalid(format!(
            "{what} must be an ID: a bigint in 0n..2n**64n"
        )));
    }
    Ok(Id(v))
}

fn need<T>(v: Option<T>, op: &str, field: &str) -> R<T> {
    v.ok_or_else(|| Fail::invalid(format!("{op}: missing field `{field}`")))
}

pub(crate) fn term_ref(t: TermIn, what: &str) -> R<TermRef> {
    let v = |t: &TermIn| {
        t.v.clone()
            .ok_or_else(|| Fail::invalid(format!("{what}: term without a value")))
    };
    Ok(match t.k.as_str() {
        "id" => TermRef::Id(id_of(
            t.id.as_ref()
                .ok_or_else(|| Fail::invalid(format!("{what}: ID term without an ID")))?,
            what,
        )?),
        "iri" => TermRef::Term(Term::iri(v(&t)?)),
        "blank" => TermRef::Term(Term::Blank(v(&t)?)),
        "lit" => {
            let lex = v(&t)?;
            TermRef::Term(Term::lit(match (t.lang, t.dt) {
                (Some(lang), _) => Literal::lang(lex, lang),
                (None, Some(dt)) => Literal::typed(lex, dt),
                (None, None) => Literal::string(lex),
            }))
        }
        k => return Err(Fail::invalid(format!("{what}: unknown term kind `{k}`"))),
    })
}

pub(crate) fn term(t: TermIn, what: &str) -> R<Term> {
    match term_ref(t, what)? {
        TermRef::Term(t) => Ok(t),
        TermRef::Id(_) => Err(Fail::invalid(format!("{what}: expected a term, not an ID"))),
    }
}

pub(crate) fn durability(d: &str) -> R<Durability> {
    match d {
        "sync" => Ok(Durability::Sync),
        "ordered" => Ok(Durability::Ordered),
        "relaxed" => Ok(Durability::Relaxed),
        _ => Err(Fail::invalid(format!(
            "unknown durability `{d}`: use \"sync\", \"ordered\" or \"relaxed\""
        ))),
    }
}

pub(crate) fn lang(l: &str) -> R<Lang> {
    match l {
        "sparql" => Ok(Lang::Sparql),
        "datalog" => Ok(Lang::Datalog),
        _ => Err(Fail::invalid(format!("unknown query language `{l}`"))),
    }
}

fn delete_mode(m: Option<String>) -> R<DeleteMode> {
    match m.as_deref() {
        None | Some("cascade") => Ok(DeleteMode::Cascade),
        Some("keep") | Some("keepAnnotations") => Ok(DeleteMode::KeepAnnotations),
        Some("detach") => Ok(DeleteMode::Detach),
        Some(m) => Err(Fail::invalid(format!(
            "unknown delete mode `{m}`: use \"cascade\", \"keep\" or \"detach\""
        ))),
    }
}

// @tg: implements:: [[openspec:node-binding#Node API parity]]
/// Builds a core operation; a missing field is `INVALID` and names the field.
pub(crate) fn core_op(o: OpIn) -> R<CoreOp> {
    let op = o.op.clone();
    let op = op.as_str();
    let t = |v: Option<TermIn>, f: &str| -> R<TermRef> { term_ref(need(v, op, f)?, f) };
    let opt = |v: Option<TermIn>, f: &str| -> R<Option<TermRef>> {
        v.map(|v| term_ref(v, f)).transpose()
    };
    let idf = |v: Option<BigInt>, f: &str| -> R<Id> { id_of(&need(v, op, f)?, f) };
    let pairs = |v: Option<Vec<PairIn>>| -> R<Vec<(TermRef, TermRef)>> {
        v.unwrap_or_default()
            .into_iter()
            .map(|p| Ok((term_ref(p.key, "role")?, term_ref(p.value, "filler")?)))
            .collect()
    };
    Ok(match op {
        "insert" => CoreOp::Insert {
            s: t(o.s, "s")?,
            p: t(o.p, "p")?,
            o: t(o.o, "o")?,
            graph: opt(o.graph, "graph")?,
        },
        "set" => CoreOp::Set {
            s: t(o.s, "s")?,
            p: t(o.p, "p")?,
            o: t(o.o, "o")?,
        },
        "update" => CoreOp::Update {
            id: idf(o.id, "id")?,
            o: t(o.o, "o")?,
            expected: opt(o.expected, "expected")?,
        },
        "annotate" => CoreOp::Annotate {
            id: idf(o.id, "id")?,
            p: t(o.p, "p")?,
            o: t(o.o, "o")?,
        },
        "delete" => CoreOp::Delete {
            id: idf(o.id, "id")?,
            mode: delete_mode(o.mode)?,
        },
        "remove" => CoreOp::Remove {
            s: t(o.s, "s")?,
            p: t(o.p, "p")?,
            o: t(o.o, "o")?,
            mode: delete_mode(o.mode)?,
            graph: opt(o.graph, "graph")?,
        },
        "statementId" => CoreOp::StatementId {
            s: t(o.s, "s")?,
            p: t(o.p, "p")?,
            o: t(o.o, "o")?,
        },
        "insertUnasserted" => CoreOp::InsertUnasserted {
            s: t(o.s, "s")?,
            p: t(o.p, "p")?,
            o: t(o.o, "o")?,
        },
        "declarePredicate" => CoreOp::DeclarePredicate {
            p: t(o.p, "p")?,
            decl: PredicateDecl {
                multi: o.multi.unwrap_or(false),
                functional: o.functional.unwrap_or(false),
            },
        },
        "createGraph" => CoreOp::CreateGraph {
            name: t(o.name, "name")?,
        },
        "addMember" => CoreOp::AddMember {
            item: idf(o.item, "item")?,
            graph: t(o.graph, "graph")?,
        },
        "removeMember" => CoreOp::RemoveMember {
            item: idf(o.item, "item")?,
            graph: t(o.graph, "graph")?,
        },
        "moveMember" => CoreOp::MoveMember {
            item: idf(o.item, "item")?,
            from: t(o.from, "from")?,
            to: t(o.to, "to")?,
        },
        "membershipId" => CoreOp::MembershipId {
            item: idf(o.item, "item")?,
            graph: t(o.graph, "graph")?,
        },
        "nest" => CoreOp::Nest {
            child: t(o.child, "child")?,
            parent: t(o.parent, "parent")?,
        },
        "unnest" => CoreOp::Unnest {
            child: t(o.child, "child")?,
            parent: t(o.parent, "parent")?,
        },
        "dropGraph" => CoreOp::DropGraph {
            graph: t(o.graph, "graph")?,
            recursive: o.recursive.unwrap_or(false),
        },
        "declareHedgeType" => {
            let specs = need(o.role_specs, op, "roles")?;
            let mut roles = Vec::with_capacity(specs.len());
            for r in specs {
                roles.push(RoleSpec {
                    role: term_ref(r.role, "role")?,
                    many: r.many.unwrap_or(false),
                    required: r.required.unwrap_or(true),
                    kind: None,
                });
            }
            let attrs = o
                .attr_decls
                .unwrap_or_default()
                .into_iter()
                .map(|a| term_ref(a, "attrs"))
                .collect::<R<Vec<_>>>()?;
            CoreOp::DeclareHedgeType {
                ty: t(o.ty, "type")?,
                decl: HedgeTypeDecl { roles, attrs },
            }
        }
        "createHedge" => CoreOp::CreateHedge {
            ty: t(o.ty, "type")?,
            graph: opt(o.graph, "graph")?,
            roles: pairs(Some(need(o.roles, op, "roles")?))?,
            attrs: pairs(o.attrs)?,
        },
        "updateRole" => CoreOp::UpdateRole {
            hedge: idf(o.hedge, "hedge")?,
            role: t(o.role, "role")?,
            filler: t(o.filler, "filler")?,
        },
        "roleId" => CoreOp::RoleId {
            hedge: idf(o.hedge, "hedge")?,
            role: t(o.role, "role")?,
            node: t(o.node, "node")?,
        },
        other => return Err(Fail::invalid(format!("unknown core operation `{other}`"))),
    })
}

// ---- outputs ----------------------------------------------------------------------

/// A result tree, built on any thread and turned into JS values on the
/// JavaScript thread.
#[derive(Debug)]
pub enum Out {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    /// An ID, as a `bigint`.
    Id(u64),
    Arr(Vec<Out>),
    Obj(Vec<(&'static str, Out)>),
    /// A column of IDs, as a `BigUint64Array`.
    Ids(Vec<u64>),
    /// Samples, as a `Float64Array` (benchmarks).
    F64s(Vec<f64>),
    Db(crate::db::NativeDb),
    Txn(crate::txn::NativeTxn),
    Snap(crate::snapshot::NativeSnapshot),
    Rows(crate::snapshot::NativeRows),
}

impl TypeName for Out {
    fn type_name() -> &'static str {
        "unknown"
    }
    fn value_type() -> ValueType {
        ValueType::Unknown
    }
}

impl ValidateNapiValue for Out {}

impl ToNapiValue for Out {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> napi::Result<sys::napi_value> {
        // SAFETY (every call below): `env` is the valid env napi passed to this
        // conversion, on the JavaScript thread; the values passed on are owned.
        unsafe {
            match val {
                Out::Undefined => <()>::to_napi_value(env, ()),
                Out::Null => Null::to_napi_value(env, Null),
                Out::Bool(b) => bool::to_napi_value(env, b),
                Out::Num(n) => f64::to_napi_value(env, n),
                Out::Str(s) => String::to_napi_value(env, s),
                Out::Id(i) => BigInt::to_napi_value(env, BigInt::from(i)),
                Out::Arr(v) => Vec::<Out>::to_napi_value(env, v),
                Out::Obj(fields) => {
                    let mut obj = std::ptr::null_mut();
                    check_status!(sys::napi_create_object(env, &mut obj))?;
                    for (k, v) in fields {
                        let key = CString::new(k).expect("field names have no NUL");
                        let jv = Out::to_napi_value(env, v)?;
                        check_status!(sys::napi_set_named_property(env, obj, key.as_ptr(), jv))?;
                    }
                    Ok(obj)
                }
                Out::Ids(v) => BigUint64Array::to_napi_value(env, BigUint64Array::new(v)),
                Out::F64s(v) => Float64Array::to_napi_value(env, Float64Array::new(v)),
                Out::Db(d) => crate::db::NativeDb::to_napi_value(env, d),
                Out::Txn(t) => crate::txn::NativeTxn::to_napi_value(env, t),
                Out::Snap(s) => crate::snapshot::NativeSnapshot::to_napi_value(env, s),
                Out::Rows(r) => crate::snapshot::NativeRows::to_napi_value(env, r),
            }
        }
    }
}

pub(crate) fn id_out(id: Id) -> Out {
    Out::Id(id.0)
}

pub(crate) fn opt_id(id: Option<Id>) -> Out {
    id.map_or(Out::Null, id_out)
}

pub(crate) fn ids_out(ids: &[Id]) -> Out {
    Out::Arr(ids.iter().copied().map(id_out).collect())
}

pub(crate) fn match_out(m: &Match) -> Out {
    Out::Obj(vec![
        ("s", id_out(m.s)),
        ("p", id_out(m.p)),
        ("o", id_out(m.o)),
        ("id", opt_id(m.id)),
    ])
}

pub(crate) fn matches_out(ms: &[Match]) -> Out {
    Out::Arr(ms.iter().map(match_out).collect())
}

pub(crate) fn triple_out(t: &Triple) -> Out {
    Out::Obj(vec![
        ("s", id_out(t.s)),
        ("p", id_out(t.p)),
        ("o", id_out(t.o)),
    ])
}

pub(crate) fn hedge_out(h: &Hyperedge) -> Out {
    Out::Obj(vec![
        ("id", id_out(h.id)),
        ("type", id_out(h.ty)),
        ("graph", id_out(h.graph)),
        (
            "roles",
            Out::Arr(
                h.roles
                    .iter()
                    .map(|(r, f)| Out::Arr(vec![id_out(*r), id_out(*f)]))
                    .collect(),
            ),
        ),
    ])
}

pub(crate) fn op_result(r: &OpResult) -> Out {
    match r {
        OpResult::Id(id) => opt_id(*id),
        OpResult::Deleted(ids) => ids_out(ids),
        OpResult::Removed(b) | OpResult::Changed(b) => Out::Bool(*b),
        OpResult::Done => Out::Undefined,
    }
}

/// A tagged term object for the JS layer: `{k, v, dt?, lang?}` or a triple.
pub(crate) fn term_out(t: &Term) -> Out {
    match t {
        Term::Iri(i) => Out::Obj(vec![
            ("k", Out::Str("iri".into())),
            ("v", Out::Str(i.clone())),
        ]),
        Term::Blank(b) => Out::Obj(vec![
            ("k", Out::Str("blank".into())),
            ("v", Out::Str(b.clone())),
        ]),
        Term::Literal(l) => {
            let mut f = vec![
                ("k", Out::Str("lit".into())),
                ("v", Out::Str(l.lexical.clone())),
                ("dt", Out::Str(l.datatype.clone())),
            ];
            if let Some(lang) = &l.lang {
                f.push(("lang", Out::Str(lang.clone())));
            }
            Out::Obj(f)
        }
        Term::Triple(t) => Out::Obj(vec![
            ("k", Out::Str("triple".into())),
            ("s", term_out(&t.subject)),
            ("p", term_out(&t.predicate)),
            ("o", term_out(&t.object)),
        ]),
        other => Out::Obj(vec![
            ("k", Out::Str("unknown".into())),
            ("v", Out::Str(other.to_string())),
        ]),
    }
}

/// A decoded query result: `{vars, rows}` with tagged terms, `undefined` for
/// unbound. ASK answers in one `ask` column, CONSTRUCT in `s`, `p`, `o`.
pub(crate) fn query_result_out(q: &QueryResult) -> Out {
    let term = |t: &Option<Term>| t.as_ref().map_or(Out::Undefined, term_out);
    let (vars, rows): (Vec<String>, Vec<Out>) = match q {
        QueryResult::Solutions { vars, rows } => (
            vars.clone(),
            rows.iter()
                .map(|r| Out::Arr(r.iter().map(term).collect()))
                .collect(),
        ),
        QueryResult::Boolean(b) => (
            vec!["ask".into()],
            vec![Out::Arr(vec![term_out(&Term::lit(Literal::boolean(*b)))])],
        ),
        QueryResult::Graph(ts) => (
            vec!["s".into(), "p".into(), "o".into()],
            ts.iter()
                .map(|t| Out::Arr(t.iter().map(term_out).collect()))
                .collect(),
        ),
    };
    Out::Obj(vec![
        ("vars", Out::Arr(vars.into_iter().map(Out::Str).collect())),
        ("rows", Out::Arr(rows)),
    ])
}
