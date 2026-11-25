//! Python binding of graphlava: a PyO3 extension over the Rust API
//! ([[adr:0042]], [[16-api#Python binding]]). The value types, write
//! operations and exception classes live in the pure-Python part of the
//! package (`python/graphlava/`); this module does the core calls.
//!
//! Every call that touches the store, parses, plans, executes or waits on a
//! commit runs with the GIL released (`Python::detach`) and inside
//! `catch_unwind`, so a panic becomes `InternalError` instead of aborting the
//! interpreter. Handles record the PID that opened them: a call from a forked
//! child raises `ForkedHandleError`, and a child never flushes, closes or
//! drops an inherited handle.

// @tg: implements:: [[adr:0042]]
// @tg: implements:: [[openspec:python-binding#Python API parity]]
// @tg: implements:: [[openspec:python-binding#Python transactions as context managers]]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use graphlava::{
    CoreOp, Database, DeleteMode, Durability, Error, HedgeTypeDecl, Id, Lang, Literal, OpResult,
    OpenOptions, PredicateDecl, QueryResult, ReadTxn, Reader, RoleSpec, Term, TermRef,
    WriteRequest, WriteTxn,
};
use pyo3::exceptions::{PyStopIteration, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple, PyType};
use pyo3::{IntoPyObjectExt, PyClassInitializer};

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const BATCH: usize = 1024;

/// Dictionary decodes done for results (the `graphlava._debug` counter).
static DECODES: AtomicU64 = AtomicU64::new(0);

fn decode_counted(r: &dyn Reader, id: Id) -> graphlava::Result<Term> {
    DECODES.fetch_add(1, Ordering::Relaxed);
    r.decode(id)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic inside a core call is caught and reported; the state it
    // guarded is still consistent for the next caller.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- fork safety (D5) -------------------------------------------------------------

/// A value that is only dropped in the process that created it. After `fork`
/// a child's copy is leaked: dropping it would flush a batch, join a flusher
/// thread that does not exist in the child, or touch locks held by parent
/// threads at the time of the fork.
// @tg: implements:: [[openspec:python-binding#Single owner across fork]]
struct Owned<T> {
    v: Option<T>,
    pid: u32,
}

impl<T> Owned<T> {
    fn new(v: T) -> Owned<T> {
        Owned {
            v: Some(v),
            pid: std::process::id(),
        }
    }
    fn get(&self) -> &T {
        self.v.as_ref().expect("owned value present")
    }
    fn get_mut(&mut self) -> &mut T {
        self.v.as_mut().expect("owned value present")
    }
    fn take(mut self) -> T {
        self.v.take().expect("owned value present")
    }
}

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        if self.pid != std::process::id() {
            if let Some(v) = self.v.take() {
                std::mem::forget(v);
            }
        }
    }
}

/// The open database, shared by the handle and every transaction or result
/// that borrows it. The `Database` closes when the last reference goes.
struct Shared {
    db: Owned<Database>,
}

impl Shared {
    fn db(&self) -> &Database {
        self.db.get()
    }
    fn owner_pid(&self) -> u32 {
        self.db.pid
    }
}

fn check_pid(py: Python<'_>, owner: u32) -> PyResult<()> {
    let pid = std::process::id();
    if pid == owner {
        return Ok(());
    }
    Err(py_error(
        py,
        "ForkedHandleError",
        &format!(
            "this handle was opened by process {owner} and cannot be used in process {pid} \
             (after os.fork); open the file in the child only after the parent closed it"
        ),
        &[
            ("owner_pid", owner.into_py_any(py)?),
            ("pid", pid.into_py_any(py)?),
        ],
    ))
}

// ---- running core work (D2) ---------------------------------------------------------

/// Runs `f` with the GIL released and panics caught.
// @tg: implements:: [[openspec:python-binding#GIL released during core work]]
fn run<T: Send>(py: Python<'_>, f: impl FnOnce() -> graphlava::Result<T> + Send) -> PyResult<T> {
    match py.detach(|| catch_unwind(AssertUnwindSafe(f))) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(map_error(py, e)),
        Err(p) => Err(internal(py, &p)),
    }
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown panic".into())
}

fn internal(py: Python<'_>, p: &(dyn std::any::Any + Send)) -> PyErr {
    py_error(
        py,
        "InternalError",
        &format!("internal error (Rust panic): {}", panic_message(p)),
        &[],
    )
}

// ---- Python-side classes ------------------------------------------------------------

struct Classes {
    errors: Py<PyModule>,
    id: Py<PyType>,
    iri: Py<PyType>,
    bnode: Py<PyType>,
    literal: Py<PyType>,
    triple: Py<PyType>,
    match_: Py<PyType>,
    hyperedge: Py<PyType>,
    commit: Py<PyType>,
    sweep: Py<PyType>,
    update_report: Py<PyType>,
    decimal: Py<PyType>,
    datetime: Py<PyType>,
    date: Py<PyType>,
    int: Py<PyType>,
    arrow_table: Py<PyAny>,
}

static CLASSES: PyOnceLock<Classes> = PyOnceLock::new();

fn classes(py: Python<'_>) -> PyResult<&Classes> {
    CLASSES.get_or_try_init(py, || {
        let t = py.import("graphlava._types")?;
        let ty = |m: &Bound<'_, PyModule>, n: &str| -> PyResult<Py<PyType>> {
            Ok(m.getattr(n)?.cast_into::<PyType>()?.unbind())
        };
        let dec = py.import("decimal")?;
        let dt = py.import("datetime")?;
        let builtins = py.import("builtins")?;
        Ok(Classes {
            errors: py.import("graphlava._errors")?.unbind(),
            id: ty(&t, "Id")?,
            iri: ty(&t, "IRI")?,
            bnode: ty(&t, "BNode")?,
            literal: ty(&t, "Literal")?,
            triple: ty(&t, "Triple")?,
            match_: ty(&t, "Match")?,
            hyperedge: ty(&t, "Hyperedge")?,
            commit: ty(&t, "CommitResult")?,
            sweep: ty(&t, "SweepReport")?,
            update_report: ty(&t, "UpdateReport")?,
            decimal: ty(&dec, "Decimal")?,
            datetime: ty(&dt, "datetime")?,
            date: ty(&dt, "date")?,
            int: ty(&builtins, "int")?,
            arrow_table: py.import("graphlava._arrow")?.getattr("table")?.unbind(),
        })
    })
}

/// Builds `graphlava.<name>(message, **fields)` as a Python exception.
fn py_error(py: Python<'_>, name: &str, msg: &str, fields: &[(&str, Py<PyAny>)]) -> PyErr {
    let build = || -> PyResult<PyErr> {
        let c = classes(py)?;
        let cls = c.errors.bind(py).getattr(name)?;
        let kw = PyDict::new(py);
        for (k, v) in fields {
            kw.set_item(*k, v)?;
        }
        Ok(PyErr::from_value(cls.call((msg,), Some(&kw))?))
    };
    build().unwrap_or_else(|e| e)
}

fn invalid(py: Python<'_>, msg: impl AsRef<str>) -> PyErr {
    py_error(py, "InvalidError", msg.as_ref(), &[])
}

/// Maps every Rust `Error` variant to its exception class, with the
/// variant's fields as attributes.
// @tg: implements:: [[openspec:python-binding#Python errors]]
fn map_error(py: Python<'_>, e: Error) -> PyErr {
    let msg = e.to_string();
    let id = |v: u64| make_id(py, Id(v)).unwrap_or_else(|_| py.None());
    let int = |v: u64| v.into_py_any(py).unwrap_or_else(|_| py.None());
    let (name, fields): (&str, Vec<(&str, Py<PyAny>)>) = match &e {
        Error::Io(_) => ("IoError", vec![]),
        Error::Corrupted(_) => ("CorruptedError", vec![]),
        Error::Full => ("FullError", vec![]),
        Error::FormatMismatch(_) => ("FormatMismatchError", vec![]),
        Error::Locked => ("LockedError", vec![]),
        Error::Conflict(_) => ("ConflictError", vec![]),
        Error::NotFound(v) => ("NotFoundError", vec![("id", id(*v))]),
        Error::ReferencedByStatement { item, by } => (
            "ReferencedByStatementError",
            vec![("item", id(*item)), ("by", id(*by))],
        ),
        Error::FillsRole { item, hedge } => (
            "FillsRoleError",
            vec![("item", id(*item)), ("hedge", id(*hedge))],
        ),
        Error::GraphCycle { child, parent } => (
            "GraphCycleError",
            vec![("child", id(*child)), ("parent", id(*parent))],
        ),
        Error::GraphDepthExceeded { limit } => (
            "GraphDepthExceededError",
            vec![("limit", int(u64::from(*limit)))],
        ),
        Error::HyperedgeShapeMismatch(_) => ("HyperedgeShapeMismatchError", vec![]),
        Error::CascadeLimitExceeded { limit } => {
            ("CascadeLimitExceededError", vec![("limit", int(*limit))])
        }
        Error::WriteTimeout => {
            return py_error(
                py,
                "WriteTimeoutError",
                "write transaction exceeded its time limit (default 100 ms) and was aborted; \
                 use db.submit([...]) for writes that cannot finish within the limit",
                &[],
            );
        }
        Error::ReadOnly => ("ReadOnlyError", vec![]),
        Error::Invalid(_) => ("InvalidError", vec![]),
        _ => ("InternalError", vec![]),
    };
    py_error(py, name, &msg, &fields)
}

fn closed(py: Python<'_>, what: &str) -> PyErr {
    py_error(
        py,
        "TransactionClosedError",
        &format!("{what} is closed"),
        &[],
    )
}

// ---- term mapping (D4) ------------------------------------------------------------------

/// Where a Python value appears: in subject, predicate or graph position a
/// plain `int` is an ID; in object position it is an `xsd:integer`.
#[derive(Clone, Copy, PartialEq)]
enum Pos {
    Node,
    Object,
}

fn is_type(obj: &Bound<'_, PyAny>, ty: &Py<PyType>) -> bool {
    obj.get_type().as_ptr() == ty.as_ptr()
}

// @tg: implements:: [[openspec:python-binding#Python term mapping]]
fn to_tref(py: Python<'_>, c: &Classes, obj: &Bound<'_, PyAny>, pos: Pos) -> PyResult<TermRef> {
    if is_type(obj, &c.id) {
        return Ok(TermRef::Id(Id(obj.extract::<u64>()?)));
    }
    Ok(TermRef::Term(to_term(py, c, obj, pos)?))
}

fn to_term(py: Python<'_>, c: &Classes, obj: &Bound<'_, PyAny>, pos: Pos) -> PyResult<Term> {
    if is_type(obj, &c.iri) {
        return Ok(Term::Iri(obj.getattr("value")?.extract()?));
    }
    if obj.is_instance_of::<PyString>() {
        return Ok(Term::string(obj.extract::<String>()?));
    }
    if obj.is_instance_of::<PyBool>() {
        return Ok(Term::lit(Literal::boolean(obj.extract::<bool>()?)));
    }
    if obj.is_instance_of::<PyInt>() {
        if pos == Pos::Node || obj.is_instance(c.id.bind(py))? {
            return Err(invalid(
                py,
                "an int in subject, predicate or graph position is an ID; wrap terms in IRI(...)",
            ));
        }
        return Ok(match obj.extract::<i64>() {
            Ok(v) => Term::integer(v),
            Err(_) => Term::lit(Literal::typed(
                obj.str()?.to_string(),
                format!("{XSD}integer"),
            )),
        });
    }
    if obj.is_instance_of::<PyFloat>() {
        let v: f64 = obj.extract()?;
        if v.is_nan() {
            return Err(invalid(
                py,
                "NaN cannot be stored: it is not equal to itself",
            ));
        }
        return Ok(Term::double(v));
    }
    if is_type(obj, &c.literal) {
        let lex: String = obj.getattr("lexical")?.extract()?;
        let lang: Option<String> = obj.getattr("lang")?.extract()?;
        return Ok(match lang {
            Some(tag) => Term::lit(Literal::lang(lex, tag)),
            None => Term::lit(Literal::typed(
                lex,
                obj.getattr("datatype")?.extract::<String>()?,
            )),
        });
    }
    if is_type(obj, &c.bnode) {
        return Ok(Term::Blank(obj.getattr("label")?.extract()?));
    }
    if is_type(obj, &c.triple) {
        return Ok(Term::triple(
            to_term(py, c, &obj.getattr("s")?, Pos::Node)?,
            to_term(py, c, &obj.getattr("p")?, Pos::Node)?,
            to_term(py, c, &obj.getattr("o")?, Pos::Object)?,
        ));
    }
    if obj.is_instance(c.datetime.bind(py))? {
        if obj.call_method0("utcoffset")?.is_none() {
            return Err(invalid(
                py,
                "a naive datetime has no instant; give it a tzinfo (e.g. datetime.timezone.utc)",
            ));
        }
        let iso: String = obj.call_method0("isoformat")?.extract()?;
        return Ok(Term::lit(Literal::date_time(iso)));
    }
    if obj.is_instance(c.date.bind(py))? {
        let iso: String = obj.call_method0("isoformat")?.extract()?;
        return Ok(Term::lit(Literal::typed(iso, format!("{XSD}date"))));
    }
    if obj.is_instance(c.decimal.bind(py))? {
        if !obj.call_method0("is_finite")?.extract::<bool>()? {
            return Err(invalid(py, "only finite Decimal values can be stored"));
        }
        let lex: String = py
            .import("builtins")?
            .getattr("format")?
            .call1((obj, "f"))?
            .extract()?;
        return Ok(Term::lit(Literal::decimal(lex)));
    }
    if is_type(obj, &c.id) || obj.is_instance(c.id.bind(py))? {
        return Err(invalid(py, "an ID cannot be part of a triple term"));
    }
    Err(PyTypeError::new_err(format!(
        "cannot store a {} as a graphlava term",
        obj.get_type().name()?
    )))
}

/// An ID argument: any `int` (a plain one or an `Id`).
fn arg_id(obj: &Bound<'_, PyAny>) -> PyResult<Id> {
    obj.extract::<u64>()
        .map(Id)
        .map_err(|_| PyTypeError::new_err("an ID is an int in 0..2**64"))
}

fn make_id(py: Python<'_>, id: Id) -> PyResult<Py<PyAny>> {
    Ok(classes(py)?.id.bind(py).call1((id.raw(),))?.unbind())
}

fn opt_id(py: Python<'_>, id: Option<Id>) -> PyResult<Py<PyAny>> {
    match id {
        Some(i) => make_id(py, i),
        None => Ok(py.None()),
    }
}

fn ids_list(py: Python<'_>, ids: &[Id]) -> PyResult<Py<PyAny>> {
    let v = ids
        .iter()
        .map(|i| make_id(py, *i))
        .collect::<PyResult<Vec<_>>>()?;
    Ok(PyList::new(py, v)?.into_any().unbind())
}

fn literal_obj(py: Python<'_>, c: &Classes, l: &Literal) -> PyResult<Py<PyAny>> {
    let kw = PyDict::new(py);
    match &l.lang {
        Some(tag) => kw.set_item("lang", tag)?,
        None => kw.set_item("datatype", &l.datatype)?,
    }
    Ok(c.literal.bind(py).call((&l.lexical,), Some(&kw))?.unbind())
}

/// An `xsd:dateTime` lexical with a timezone (`Z` or `±HH:MM`) as an ISO
/// string Python 3.10 parses: six fractional digits and an explicit offset.
/// `None` for a dateTime without a timezone (it stays a `Literal`).
fn iso_aware(lex: &str) -> Option<String> {
    let (body, tz) = if let Some(b) = lex.strip_suffix('Z') {
        (b, "+00:00")
    } else if lex.len() >= 25 && matches!(lex.as_bytes()[lex.len() - 6], b'+' | b'-') {
        lex.split_at(lex.len() - 6)
    } else {
        return None;
    };
    let (main, frac) = match body.split_once('.') {
        Some((m, f)) if f.len() <= 6 && f.bytes().all(|b| b.is_ascii_digit()) => (m, f),
        Some(_) => return None,
        None => (body, ""),
    };
    if main.len() != 19 || main.starts_with('-') {
        return None;
    }
    Some(if frac.is_empty() {
        format!("{main}{tz}")
    } else {
        format!("{main}.{frac:0<6}{tz}")
    })
}

fn literal_to_py(py: Python<'_>, c: &Classes, l: &Literal) -> PyResult<Py<PyAny>> {
    if l.lang.is_some() {
        return literal_obj(py, c, l);
    }
    let Some(local) = l.datatype.strip_prefix(XSD) else {
        return literal_obj(py, c, l);
    };
    let lex = l.lexical.as_str();
    let native: Option<PyResult<Py<PyAny>>> = match local {
        "string" => Some(lex.into_py_any(py)),
        "integer" => Some(match lex.parse::<i64>() {
            Ok(v) => v.into_py_any(py),
            Err(_)
                if !lex.is_empty()
                    && lex
                        .trim_start_matches('-')
                        .bytes()
                        .all(|b| b.is_ascii_digit()) =>
            {
                c.int.bind(py).call1((lex,)).map(Bound::unbind)
            }
            Err(_) => return literal_obj(py, c, l),
        }),
        "boolean" => match lex {
            "true" | "1" => Some(true.into_py_any(py)),
            "false" | "0" => Some(false.into_py_any(py)),
            _ => None,
        },
        "double" => match lex {
            "INF" => Some(f64::INFINITY.into_py_any(py)),
            "-INF" => Some(f64::NEG_INFINITY.into_py_any(py)),
            "NaN" => Some(f64::NAN.into_py_any(py)),
            _ => lex.parse::<f64>().ok().map(|v| v.into_py_any(py)),
        },
        "decimal" => Some(c.decimal.bind(py).call1((lex,)).map(Bound::unbind)),
        "dateTime" => iso_aware(lex).map(|iso| {
            c.datetime
                .bind(py)
                .call_method1("fromisoformat", (iso,))
                .map(Bound::unbind)
        }),
        "date" if lex.len() == 10 && !lex.starts_with('-') => Some(
            c.date
                .bind(py)
                .call_method1("fromisoformat", (lex,))
                .map(Bound::unbind),
        ),
        _ => None,
    };
    match native {
        Some(Ok(v)) => Ok(v),
        // A value the Python type cannot hold (e.g. year 0) stays a Literal.
        Some(Err(_)) | None => literal_obj(py, c, l),
    }
}

fn term_to_py(py: Python<'_>, c: &Classes, t: &Term) -> PyResult<Py<PyAny>> {
    Ok(match t {
        Term::Iri(i) => c.iri.bind(py).call1((i,))?.unbind(),
        Term::Blank(b) => c.bnode.bind(py).call1((b,))?.unbind(),
        Term::Literal(l) => literal_to_py(py, c, l)?,
        Term::Triple(tt) => c
            .triple
            .bind(py)
            .call1((
                term_to_py(py, c, &tt.subject)?,
                term_to_py(py, c, &tt.predicate)?,
                term_to_py(py, c, &tt.object)?,
            ))?
            .unbind(),
        other => literal_obj(py, c, &Literal::string(other.to_string()))?,
    })
}

fn opt_term_to_py(py: Python<'_>, c: &Classes, t: &Option<Term>) -> PyResult<Py<PyAny>> {
    match t {
        Some(t) => term_to_py(py, c, t),
        None => Ok(py.None()),
    }
}

fn parse_durability(py: Python<'_>, d: Option<&Bound<'_, PyAny>>) -> PyResult<Durability> {
    let Some(d) = d.filter(|d| !d.is_none()) else {
        return Ok(Durability::Ordered);
    };
    let s: Option<String> = d.extract().ok();
    match s.as_deref() {
        Some("sync") => Ok(Durability::Sync),
        Some("ordered") => Ok(Durability::Ordered),
        Some("relaxed") => Ok(Durability::Relaxed),
        _ => {
            let _ = py;
            Err(PyValueError::new_err(format!(
                "durability must be one of 'sync', 'ordered' or 'relaxed' (or a graphlava.Durability), got {}",
                d.repr()?
            )))
        }
    }
}

fn parse_mode(d: &Bound<'_, PyAny>) -> PyResult<DeleteMode> {
    let s: Option<String> = d.extract().ok();
    match s.as_deref() {
        Some("cascade") => Ok(DeleteMode::Cascade),
        Some("keep_annotations") => Ok(DeleteMode::KeepAnnotations),
        Some("detach") => Ok(DeleteMode::Detach),
        _ => Err(PyValueError::new_err(format!(
            "mode must be one of 'cascade', 'keep_annotations' or 'detach', got {}",
            d.repr()?
        ))),
    }
}

// ---- operations -------------------------------------------------------------------------

/// Builds a `CoreOp` from one of the op classes of `graphlava._types`.
fn to_core_op(py: Python<'_>, c: &Classes, op: &Bound<'_, PyAny>) -> PyResult<CoreOp> {
    let kind: String = op.getattr("_op").and_then(|k| k.extract()).map_err(|_| {
        PyTypeError::new_err(format!(
            "not a graphlava operation: {}",
            op.repr().map(|r| r.to_string()).unwrap_or_default()
        ))
    })?;
    let node = |k: &str| -> PyResult<TermRef> { to_tref(py, c, &op.getattr(k)?, Pos::Node) };
    let obj = |k: &str| -> PyResult<TermRef> { to_tref(py, c, &op.getattr(k)?, Pos::Object) };
    let opt_node = |k: &str| -> PyResult<Option<TermRef>> {
        let v = op.getattr(k)?;
        if v.is_none() {
            Ok(None)
        } else {
            to_tref(py, c, &v, Pos::Node).map(Some)
        }
    };
    let id = |k: &str| -> PyResult<Id> { arg_id(&op.getattr(k)?) };
    let pairs = |k: &str, second: Pos| -> PyResult<Vec<(TermRef, TermRef)>> {
        let mut out = Vec::new();
        for item in op.getattr(k)?.try_iter()? {
            let item = item?;
            let (a, b): (Bound<'_, PyAny>, Bound<'_, PyAny>) = item.extract()?;
            out.push((to_tref(py, c, &a, Pos::Node)?, to_tref(py, c, &b, second)?));
        }
        Ok(out)
    };
    Ok(match kind.as_str() {
        "insert" => CoreOp::Insert {
            s: node("s")?,
            p: node("p")?,
            o: obj("o")?,
            graph: opt_node("graph")?,
        },
        "set" => CoreOp::Set {
            s: node("s")?,
            p: node("p")?,
            o: obj("o")?,
        },
        "update" => {
            let e = op.getattr("expected")?;
            CoreOp::Update {
                id: id("id")?,
                o: obj("o")?,
                expected: if e.is_none() {
                    None
                } else {
                    Some(to_tref(py, c, &e, Pos::Object)?)
                },
            }
        }
        "annotate" => CoreOp::Annotate {
            id: id("id")?,
            p: node("p")?,
            o: obj("o")?,
        },
        "delete" => CoreOp::Delete {
            id: id("id")?,
            mode: parse_mode(&op.getattr("mode")?)?,
        },
        "remove" => CoreOp::Remove {
            s: node("s")?,
            p: node("p")?,
            o: obj("o")?,
            mode: parse_mode(&op.getattr("mode")?)?,
            graph: opt_node("graph")?,
        },
        "statement_id" => CoreOp::StatementId {
            s: node("s")?,
            p: node("p")?,
            o: obj("o")?,
        },
        "insert_unasserted" => CoreOp::InsertUnasserted {
            s: node("s")?,
            p: node("p")?,
            o: obj("o")?,
        },
        "declare_predicate" => CoreOp::DeclarePredicate {
            p: node("p")?,
            decl: PredicateDecl {
                multi: op.getattr("multi")?.extract()?,
                functional: op.getattr("functional")?.extract()?,
            },
        },
        "create_graph" => CoreOp::CreateGraph {
            name: node("name")?,
        },
        "add_member" => CoreOp::AddMember {
            item: id("item")?,
            graph: node("graph")?,
        },
        "remove_member" => CoreOp::RemoveMember {
            item: id("item")?,
            graph: node("graph")?,
        },
        "move_member" => CoreOp::MoveMember {
            item: id("item")?,
            from: node("from_graph")?,
            to: node("to_graph")?,
        },
        "membership_id" => CoreOp::MembershipId {
            item: id("item")?,
            graph: node("graph")?,
        },
        "nest" => CoreOp::Nest {
            child: node("child")?,
            parent: node("parent")?,
        },
        "unnest" => CoreOp::Unnest {
            child: node("child")?,
            parent: node("parent")?,
        },
        "drop_graph" => CoreOp::DropGraph {
            graph: node("graph")?,
            recursive: op.getattr("recursive")?.extract()?,
        },
        "declare_hedge_type" => {
            let mut roles = Vec::new();
            for r in op.getattr("roles")?.try_iter()? {
                let r = r?;
                roles.push(RoleSpec {
                    role: to_tref(py, c, &r.getattr("role")?, Pos::Node)?,
                    many: r.getattr("many")?.extract()?,
                    required: r.getattr("required")?.extract()?,
                    kind: None,
                });
            }
            let mut attrs = Vec::new();
            for a in op.getattr("attrs")?.try_iter()? {
                attrs.push(to_tref(py, c, &a?, Pos::Node)?);
            }
            CoreOp::DeclareHedgeType {
                ty: node("type")?,
                decl: HedgeTypeDecl { roles, attrs },
            }
        }
        "create_hedge" => CoreOp::CreateHedge {
            ty: node("type")?,
            graph: opt_node("graph")?,
            roles: pairs("roles", Pos::Node)?,
            attrs: pairs("attrs", Pos::Object)?,
        },
        "update_role" => CoreOp::UpdateRole {
            hedge: id("hedge")?,
            role: node("role")?,
            filler: node("filler")?,
        },
        "role_id" => CoreOp::RoleId {
            hedge: id("hedge")?,
            role: node("role")?,
            node: node("node")?,
        },
        other => {
            return Err(PyTypeError::new_err(format!(
                "unknown graphlava operation {other:?}"
            )));
        }
    })
}

fn op_result(py: Python<'_>, r: &OpResult) -> PyResult<Py<PyAny>> {
    match r {
        OpResult::Id(i) => opt_id(py, *i),
        OpResult::Deleted(v) => ids_list(py, v),
        OpResult::Removed(b) | OpResult::Changed(b) => b.into_py_any(py),
        OpResult::Done => Ok(py.None()),
    }
}

fn commit_result(py: Python<'_>, results: &[OpResult], batch: u64) -> PyResult<Py<PyAny>> {
    let c = classes(py)?;
    let rs = results
        .iter()
        .map(|r| op_result(py, r))
        .collect::<PyResult<Vec<_>>>()?;
    Ok(c.commit
        .bind(py)
        .call1((PyList::new(py, rs)?, batch))?
        .unbind())
}

// ---- reads shared by ReadTxn and WriteTxn --------------------------------------------------

/// A node argument for a read: an `int` is an ID, anything else is a term
/// that is looked up (and may be absent).
enum NodeArg {
    Id(Id),
    Term(Term),
}

fn node_arg(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<NodeArg> {
    if obj.is_instance_of::<PyInt>() && !obj.is_instance_of::<PyBool>() {
        return Ok(NodeArg::Id(arg_id(obj)?));
    }
    let c = classes(py)?;
    Ok(NodeArg::Term(to_term(py, c, obj, Pos::Object)?))
}

fn opt_node_arg(py: Python<'_>, obj: Option<&Bound<'_, PyAny>>) -> PyResult<Option<NodeArg>> {
    match obj {
        Some(o) if !o.is_none() => node_arg(py, o).map(Some),
        _ => Ok(None),
    }
}

fn resolve(r: &dyn Reader, a: &NodeArg) -> graphlava::Result<Option<Id>> {
    match a {
        NodeArg::Id(i) => Ok(Some(*i)),
        NodeArg::Term(t) => r.lookup(t),
    }
}

/// Resolves optional pattern positions; `Err(())` when a given term is unknown.
fn resolve_opt(r: &dyn Reader, a: &Option<NodeArg>) -> graphlava::Result<Result<Option<Id>, ()>> {
    match a {
        None => Ok(Ok(None)),
        Some(a) => Ok(resolve(r, a)?.map(Some).ok_or(())),
    }
}

fn matches_to_py(py: Python<'_>, ms: &[graphlava::Match]) -> PyResult<Py<PyAny>> {
    let c = classes(py)?;
    let mut out = Vec::with_capacity(ms.len());
    for m in ms {
        out.push(c.match_.bind(py).call1((
            make_id(py, m.s)?,
            make_id(py, m.p)?,
            make_id(py, m.o)?,
            opt_id(py, m.id)?,
        ))?);
    }
    Ok(PyList::new(py, out)?.into_any().unbind())
}

fn query_result_to_py(py: Python<'_>, r: &QueryResult) -> PyResult<Py<PyAny>> {
    let c = classes(py)?;
    match r {
        QueryResult::Boolean(b) => b.into_py_any(py),
        QueryResult::Solutions { rows, .. } => {
            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let vals = row
                    .iter()
                    .map(|t| opt_term_to_py(py, c, t))
                    .collect::<PyResult<Vec<_>>>()?;
                out.push(PyTuple::new(py, vals)?);
            }
            Ok(PyList::new(py, out)?.into_any().unbind())
        }
        QueryResult::Graph(ts) => {
            let mut out = Vec::with_capacity(ts.len());
            for t in ts {
                let vals = t
                    .iter()
                    .map(|x| term_to_py(py, c, x))
                    .collect::<PyResult<Vec<_>>>()?;
                out.push(PyTuple::new(py, vals)?);
            }
            Ok(PyList::new(py, out)?.into_any().unbind())
        }
    }
}

// ---- transactions ------------------------------------------------------------------------

/// A read snapshot or an interactive write transaction. The `'static`
/// lifetimes are sound because each transaction is stored next to the
/// `Arc<Shared>` that owns the `Database` it borrows, is dropped before that
/// `Arc` (field order), and the `Database` is never moved or closed while
/// another `Arc` to it exists (see `PyDatabase::close`).
enum TxnState {
    Read(Owned<ReadTxn<'static>>, Arc<Shared>),
    Write(Owned<WriteTxn<'static>>, Arc<Shared>),
    Closed,
}

/// Extends a borrow of the database inside `shared` to `'static`.
///
/// SAFETY: the caller stores the result next to a clone of `shared` and
/// drops it first (see [`TxnState`]); the `Database` lives in the `Arc`'s heap
/// allocation and is only taken out by `close` when the `Arc` is unique.
unsafe fn extend(db: &Database) -> &'static Database {
    // SAFETY: see the function contract above.
    unsafe { &*(db as *const Database) }
}

/// The read methods of a transaction (`ReadTxn`; `WriteTxn` extends it and
/// sees its own writes).
#[pyclass(module = "graphlava._native", name = "ReadTxn", subclass, frozen)]
struct PyReadTxn {
    state: Mutex<TxnState>,
}

impl PyReadTxn {
    /// Runs `f` on the transaction's reader with the GIL released.
    fn with_reader<T: Send>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&dyn Reader) -> graphlava::Result<T> + Send,
    ) -> PyResult<T> {
        {
            let g = lock(&self.state);
            match &*g {
                TxnState::Read(_, s) | TxnState::Write(_, s) => check_pid(py, s.owner_pid())?,
                TxnState::Closed => return Err(closed(py, "the transaction")),
            }
        }
        run(py, || {
            let mut g = lock(&self.state);
            match &mut *g {
                TxnState::Read(t, _) => f(t.get()),
                TxnState::Write(w, _) => w.get_mut().read(|v| f(v)),
                TxnState::Closed => Err(Error::Invalid("the transaction is closed".into())),
            }
        })
    }
}

#[pymethods]
impl PyReadTxn {
    /// All inline properties of a node.
    fn props(&self, py: Python<'_>, node: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let a = node_arg(py, node)?;
        let ms = self.with_reader(py, |r| match resolve(r, &a)? {
            Some(n) => r.props(n),
            None => Ok(Vec::new()),
        })?;
        matches_to_py(py, &ms)
    }

    /// Statements with subject `s` (and predicate `p`), with its inline properties.
    #[pyo3(signature = (s, p=None))]
    fn out(
        &self,
        py: Python<'_>,
        s: &Bound<'_, PyAny>,
        p: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let (s, p) = (node_arg(py, s)?, opt_node_arg(py, p)?);
        let ms = self.with_reader(py, |r| {
            let (Some(s), Ok(p)) = (resolve(r, &s)?, resolve_opt(r, &p)?) else {
                return Ok(Vec::new());
            };
            r.out(s, p)
        })?;
        matches_to_py(py, &ms)
    }

    /// Statements with object `o` (and predicate `p`).
    #[pyo3(signature = (o, p=None))]
    fn inc(
        &self,
        py: Python<'_>,
        o: &Bound<'_, PyAny>,
        p: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let (o, p) = (node_arg(py, o)?, opt_node_arg(py, p)?);
        let ms = self.with_reader(py, |r| {
            let (Some(o), Ok(p)) = (resolve(r, &o)?, resolve_opt(r, &p)?) else {
                return Ok(Vec::new());
            };
            r.inc(o, p)
        })?;
        matches_to_py(py, &ms)
    }

    /// Every statement matching a triple pattern (`None` is a wildcard).
    #[pyo3(signature = (s=None, p=None, o=None))]
    fn find(
        &self,
        py: Python<'_>,
        s: Option<&Bound<'_, PyAny>>,
        p: Option<&Bound<'_, PyAny>>,
        o: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let (s, p, o) = (
            opt_node_arg(py, s)?,
            opt_node_arg(py, p)?,
            opt_node_arg(py, o)?,
        );
        let ms = self.with_reader(py, |r| {
            let (Ok(s), Ok(p), Ok(o)) = (
                resolve_opt(r, &s)?,
                resolve_opt(r, &p)?,
                resolve_opt(r, &o)?,
            ) else {
                return Ok(Vec::new());
            };
            r.find(s, p, o)
        })?;
        matches_to_py(py, &ms)
    }

    /// The `(s, p, o)` IDs of a statement, or `None`.
    fn stmt(&self, py: Python<'_>, id: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let id = arg_id(id)?;
        match self.with_reader(py, |r| r.stmt(id))? {
            Some(t) => Ok(PyTuple::new(
                py,
                [make_id(py, t.s)?, make_id(py, t.p)?, make_id(py, t.o)?],
            )?
            .into_any()
            .unbind()),
            None => Ok(py.None()),
        }
    }

    /// Whether a statement exists only as a reifier target.
    fn is_unasserted(&self, py: Python<'_>, id: &Bound<'_, PyAny>) -> PyResult<bool> {
        let id = arg_id(id)?;
        self.with_reader(py, |r| r.is_unasserted(id))
    }

    /// One hyperedge, or `None`.
    fn hedge(&self, py: Python<'_>, id: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let id = arg_id(id)?;
        let Some(h) = self.with_reader(py, |r| r.hedge(id))? else {
            return Ok(py.None());
        };
        let c = classes(py)?;
        let roles = h
            .roles
            .iter()
            .map(|(r, n)| PyTuple::new(py, [make_id(py, *r)?, make_id(py, *n)?]))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(c.hyperedge
            .bind(py)
            .call1((
                make_id(py, h.id)?,
                make_id(py, h.ty)?,
                make_id(py, h.graph)?,
                PyList::new(py, roles)?,
                h.promoted,
            ))?
            .unbind())
    }

    /// The ID of a term without interning it, or `None`.
    fn lookup(&self, py: Python<'_>, term: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let c = classes(py)?;
        let t = to_term(py, c, term, Pos::Object)?;
        let id = self.with_reader(py, |r| r.lookup(&t))?;
        opt_id(py, id)
    }

    /// Decodes an ID to its Python value or term.
    fn decode(&self, py: Python<'_>, id: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let id = arg_id(id)?;
        let t = self.with_reader(py, |r| decode_counted(r, id))?;
        term_to_py(py, classes(py)?, &t)
    }

    /// The graph named by an IRI, without creating it.
    fn graph(&self, py: Python<'_>, name: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let t = to_term(py, classes(py)?, name, Pos::Object)?;
        let g = self.with_reader(py, |r| r.graph(&t))?;
        opt_id(py, g)
    }

    /// The home graph of an item.
    fn home_graph(&self, py: Python<'_>, item: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let item = arg_id(item)?;
        let g = self.with_reader(py, |r| r.home_graph(item))?;
        make_id(py, g)
    }

    /// The graphs a statement or hyperedge is a direct member of.
    fn graphs_of(&self, py: Python<'_>, item: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let item = arg_id(item)?;
        let gs = self.with_reader(py, |r| r.graphs_of(item))?;
        ids_list(py, &gs)
    }

    /// The asserted statements in a scope, in ID order.
    #[pyo3(signature = (graph, direct=false))]
    fn scope_statements(
        &self,
        py: Python<'_>,
        graph: &Bound<'_, PyAny>,
        direct: bool,
    ) -> PyResult<Py<PyAny>> {
        let g = arg_id(graph)?;
        let ids = self.with_reader(py, |r| r.scope_statements(g, direct))?;
        ids_list(py, &ids)
    }

    /// A SPARQL query on this transaction's view, decoded eagerly: a list of
    /// row tuples, or a `bool` for `ASK`.
    fn sparql(&self, py: Python<'_>, query: &str) -> PyResult<Py<PyAny>> {
        let q = query.to_string();
        let r = self.with_reader(py, move |r| r.sparql(&q))?;
        query_result_to_py(py, &r)
    }

    /// A read-only Datalog program on this transaction's view (list of rows).
    fn datalog(&self, py: Python<'_>, program: &str) -> PyResult<Py<PyAny>> {
        let q = program.to_string();
        let r = self.with_reader(py, move |r| r.datalog(&q))?;
        query_result_to_py(py, &r)
    }

    /// Ends a read snapshot (a write transaction aborts).
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        let old = std::mem::replace(&mut *lock(&self.state), TxnState::Closed);
        py.detach(move || drop(old));
        Ok(())
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    #[pyo3(signature = (*_exc))]
    fn __exit__(&self, py: Python<'_>, _exc: &Bound<'_, PyTuple>) -> PyResult<bool> {
        self.close(py)?;
        Ok(false)
    }

    #[getter]
    fn closed(&self) -> bool {
        matches!(*lock(&self.state), TxnState::Closed)
    }
}

/// An interactive write transaction: `with db.write(Durability.SYNC) as w:`
/// commits on normal exit and aborts on an exception. It holds the single
/// writer for at most the configured limit (default 100 ms, rule
/// `api-write-in-process`); prefer `Database.submit`.
#[pyclass(module = "graphlava._native", name = "WriteTxn", extends = PyReadTxn, frozen)]
struct PyWriteTxn {}

impl PyWriteTxn {
    fn state<'a>(slf: &'a Bound<'_, Self>) -> &'a Mutex<TxnState> {
        &slf.as_super().get().state
    }

    /// Runs one core operation in the transaction with the GIL released. A
    /// panic aborts the transaction and closes it.
    fn call<T: Send>(
        slf: &Bound<'_, Self>,
        f: impl FnOnce(&mut WriteTxn<'static>) -> graphlava::Result<T> + Send,
    ) -> PyResult<T> {
        let py = slf.py();
        let state = Self::state(slf);
        {
            let g = lock(state);
            match &*g {
                TxnState::Write(_, s) => check_pid(py, s.owner_pid())?,
                _ => return Err(closed(py, "the write transaction")),
            }
        }
        let r = py.detach(|| {
            let mut g = lock(state);
            let TxnState::Write(w, _) = &mut *g else {
                return Ok(Err(Error::Invalid("the transaction is closed".into())));
            };
            let r = catch_unwind(AssertUnwindSafe(|| f(w.get_mut())));
            if r.is_err() {
                // the writer state after a panic is unknown: abort
                *g = TxnState::Closed;
            }
            r
        });
        match r {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(map_error(py, e)),
            Err(p) => Err(internal(py, &p)),
        }
    }

    fn run_op(slf: &Bound<'_, Self>, op: CoreOp) -> PyResult<Py<PyAny>> {
        let r = Self::call(slf, move |w| w.run(&op))?;
        op_result(slf.py(), &r)
    }

    fn terms3(
        slf: &Bound<'_, Self>,
        s: &Bound<'_, PyAny>,
        p: &Bound<'_, PyAny>,
        o: &Bound<'_, PyAny>,
    ) -> PyResult<(TermRef, TermRef, TermRef)> {
        let py = slf.py();
        let c = classes(py)?;
        Ok((
            to_tref(py, c, s, Pos::Node)?,
            to_tref(py, c, p, Pos::Node)?,
            to_tref(py, c, o, Pos::Object)?,
        ))
    }

    fn node(slf: &Bound<'_, Self>, x: &Bound<'_, PyAny>) -> PyResult<TermRef> {
        let py = slf.py();
        node_tref(py, x)
    }
}

/// A term in node position where a plain `int` is an ID.
fn node_tref(py: Python<'_>, x: &Bound<'_, PyAny>) -> PyResult<TermRef> {
    if x.is_instance_of::<PyInt>() && !x.is_instance_of::<PyBool>() {
        return Ok(TermRef::Id(arg_id(x)?));
    }
    to_tref(py, classes(py)?, x, Pos::Node)
}

#[pymethods]
impl PyWriteTxn {
    /// Inserts a statement; returns its ID, or `None` for an inline property.
    #[pyo3(signature = (s, p, o, graph=None))]
    fn insert(
        slf: &Bound<'_, Self>,
        s: &Bound<'_, PyAny>,
        p: &Bound<'_, PyAny>,
        o: &Bound<'_, PyAny>,
        graph: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let (s, p, o) = Self::terms3(slf, s, p, o)?;
        let graph = match graph {
            Some(g) if !g.is_none() => Some(Self::node(slf, g)?),
            _ => None,
        };
        Self::run_op(slf, CoreOp::Insert { s, p, o, graph })
    }

    /// Sets the single value of `(s, p)`, replacing any other.
    fn set(
        slf: &Bound<'_, Self>,
        s: &Bound<'_, PyAny>,
        p: &Bound<'_, PyAny>,
        o: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let (s, p, o) = Self::terms3(slf, s, p, o)?;
        Self::run_op(slf, CoreOp::Set { s, p, o })
    }

    /// Replaces the object of a statement in place, optionally compare-and-set.
    #[pyo3(signature = (id, o, expected=None))]
    fn update(
        slf: &Bound<'_, Self>,
        id: &Bound<'_, PyAny>,
        o: &Bound<'_, PyAny>,
        expected: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let c = classes(py)?;
        let op = CoreOp::Update {
            id: arg_id(id)?,
            o: to_tref(py, c, o, Pos::Object)?,
            expected: match expected {
                Some(e) if !e.is_none() => Some(to_tref(py, c, e, Pos::Object)?),
                _ => None,
            },
        };
        Self::run_op(slf, op)
    }

    /// Annotates a statement; returns the annotation's statement ID.
    fn annotate(
        slf: &Bound<'_, Self>,
        id: &Bound<'_, PyAny>,
        p: &Bound<'_, PyAny>,
        o: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let c = classes(py)?;
        let op = CoreOp::Annotate {
            id: arg_id(id)?,
            p: to_tref(py, c, p, Pos::Node)?,
            o: to_tref(py, c, o, Pos::Object)?,
        };
        Self::run_op(slf, op)
    }

    /// Deletes an item; returns every deleted ID.
    #[pyo3(signature = (id, mode="cascade"))]
    fn delete(slf: &Bound<'_, Self>, id: &Bound<'_, PyAny>, mode: &str) -> PyResult<Py<PyAny>> {
        let mode = parse_mode(&PyString::new(slf.py(), mode).into_any())?;
        Self::run_op(
            slf,
            CoreOp::Delete {
                id: arg_id(id)?,
                mode,
            },
        )
    }

    /// Removes a triple (inside `graph`, only its membership there).
    #[pyo3(signature = (s, p, o, mode="cascade", graph=None))]
    fn remove(
        slf: &Bound<'_, Self>,
        s: &Bound<'_, PyAny>,
        p: &Bound<'_, PyAny>,
        o: &Bound<'_, PyAny>,
        mode: &str,
        graph: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let (s, p, o) = Self::terms3(slf, s, p, o)?;
        let mode = parse_mode(&PyString::new(slf.py(), mode).into_any())?;
        let graph = match graph {
            Some(g) if !g.is_none() => Some(Self::node(slf, g)?),
            _ => None,
        };
        Self::run_op(
            slf,
            CoreOp::Remove {
                s,
                p,
                o,
                mode,
                graph,
            },
        )
    }

    /// The statement ID of an existing triple, promoting it if needed.
    fn statement_id(
        slf: &Bound<'_, Self>,
        s: &Bound<'_, PyAny>,
        p: &Bound<'_, PyAny>,
        o: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let (s, p, o) = Self::terms3(slf, s, p, o)?;
        Self::run_op(slf, CoreOp::StatementId { s, p, o })
    }

    /// A statement that exists only as a reifier target.
    fn insert_unasserted(
        slf: &Bound<'_, Self>,
        s: &Bound<'_, PyAny>,
        p: &Bound<'_, PyAny>,
        o: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let (s, p, o) = Self::terms3(slf, s, p, o)?;
        Self::run_op(slf, CoreOp::InsertUnasserted { s, p, o })
    }

    /// Declares a predicate `multi` and/or `functional`.
    #[pyo3(signature = (p, multi=false, functional=false))]
    fn declare_predicate(
        slf: &Bound<'_, Self>,
        p: &Bound<'_, PyAny>,
        multi: bool,
        functional: bool,
    ) -> PyResult<Py<PyAny>> {
        let p = Self::node(slf, p)?;
        Self::run_op(
            slf,
            CoreOp::DeclarePredicate {
                p,
                decl: PredicateDecl { multi, functional },
            },
        )
    }

    /// Creates a named graph (or returns the existing one).
    fn create_graph(slf: &Bound<'_, Self>, name: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let name = Self::node(slf, name)?;
        Self::run_op(slf, CoreOp::CreateGraph { name })
    }

    /// Adds a statement or hyperedge to a graph.
    fn add_member(
        slf: &Bound<'_, Self>,
        item: &Bound<'_, PyAny>,
        graph: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let op = CoreOp::AddMember {
            item: arg_id(item)?,
            graph: Self::node(slf, graph)?,
        };
        Self::run_op(slf, op)
    }

    /// Removes a membership; returns the deleted IDs.
    fn remove_member(
        slf: &Bound<'_, Self>,
        item: &Bound<'_, PyAny>,
        graph: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let op = CoreOp::RemoveMember {
            item: arg_id(item)?,
            graph: Self::node(slf, graph)?,
        };
        Self::run_op(slf, op)
    }

    /// Moves a membership from one graph to another.
    fn move_member(
        slf: &Bound<'_, Self>,
        item: &Bound<'_, PyAny>,
        from_graph: &Bound<'_, PyAny>,
        to_graph: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let op = CoreOp::MoveMember {
            item: arg_id(item)?,
            from: Self::node(slf, from_graph)?,
            to: Self::node(slf, to_graph)?,
        };
        Self::run_op(slf, op)
    }

    /// The statement ID of a membership, promoting it.
    fn membership_id(
        slf: &Bound<'_, Self>,
        item: &Bound<'_, PyAny>,
        graph: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let op = CoreOp::MembershipId {
            item: arg_id(item)?,
            graph: Self::node(slf, graph)?,
        };
        Self::run_op(slf, op)
    }

    /// Nests `child` under `parent`.
    fn nest(
        slf: &Bound<'_, Self>,
        child: &Bound<'_, PyAny>,
        parent: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let op = CoreOp::Nest {
            child: Self::node(slf, child)?,
            parent: Self::node(slf, parent)?,
        };
        Self::run_op(slf, op)
    }

    /// Removes a nesting edge.
    fn unnest(
        slf: &Bound<'_, Self>,
        child: &Bound<'_, PyAny>,
        parent: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let op = CoreOp::Unnest {
            child: Self::node(slf, child)?,
            parent: Self::node(slf, parent)?,
        };
        Self::run_op(slf, op)
    }

    /// Drops a graph; returns the deleted IDs.
    #[pyo3(signature = (graph, recursive=false))]
    fn drop_graph(
        slf: &Bound<'_, Self>,
        graph: &Bound<'_, PyAny>,
        recursive: bool,
    ) -> PyResult<Py<PyAny>> {
        let graph = Self::node(slf, graph)?;
        Self::run_op(slf, CoreOp::DropGraph { graph, recursive })
    }

    /// Runs one operation object (`graphlava.Insert(...)`, ...).
    fn run(slf: &Bound<'_, Self>, op: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let op = to_core_op(py, classes(py)?, op)?;
        Self::run_op(slf, op)
    }

    /// Interns a term; returns its ID.
    fn intern(slf: &Bound<'_, Self>, term: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let t = to_tref(py, classes(py)?, term, Pos::Object)?;
        let id = Self::call(slf, move |w| w.intern(t))?;
        make_id(py, id)
    }

    /// Runs a SPARQL update inside the transaction.
    fn sparql_update(slf: &Bound<'_, Self>, update: &str) -> PyResult<Py<PyAny>> {
        let u = update.to_string();
        let rep = Self::call(slf, move |w| w.sparql_update(&u))?;
        let py = slf.py();
        Ok(classes(py)?
            .update_report
            .bind(py)
            .call1((rep.inserted, rep.deleted, rep.updated_in_place))?
            .unbind())
    }

    /// Commits; returns once the durability level holds.
    fn commit(slf: &Bound<'_, Self>) -> PyResult<Py<PyAny>> {
        let py = slf.py();
        let state = Self::state(slf);
        let old = std::mem::replace(&mut *lock(state), TxnState::Closed);
        let (w, shared) = match old {
            TxnState::Write(w, s) => (w, s),
            other => {
                *lock(state) = other;
                return Err(closed(py, "the write transaction"));
            }
        };
        check_pid(py, shared.owner_pid())?;
        let t = run(py, move || {
            let r = w.take().commit();
            drop(shared);
            r
        })?;
        commit_result(py, &t.results, t.batch)
    }

    /// Aborts; nothing the transaction wrote is kept.
    fn abort(slf: &Bound<'_, Self>) -> PyResult<()> {
        let py = slf.py();
        let old = std::mem::replace(&mut *lock(Self::state(slf)), TxnState::Closed);
        if let TxnState::Write(w, s) = old {
            check_pid(py, s.owner_pid())?;
            py.detach(move || {
                w.take().abort();
                drop(s);
            });
        }
        Ok(())
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    #[pyo3(signature = (exc_type, _exc=None, _tb=None))]
    fn __exit__(
        slf: &Bound<'_, Self>,
        exc_type: Option<&Bound<'_, PyAny>>,
        _exc: Option<&Bound<'_, PyAny>>,
        _tb: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        match exc_type {
            Some(t) if !t.is_none() => Self::abort(slf)?,
            _ => {
                if matches!(*lock(Self::state(slf)), TxnState::Write(..)) {
                    Self::commit(slf)?;
                }
            }
        }
        Ok(false)
    }
}

// ---- rows ------------------------------------------------------------------------------

/// The snapshot a result decodes from, plus its ID rows.
struct RowsState {
    // Field order: the snapshot drops before the `Arc` that owns its database.
    snap: Option<Owned<ReadTxn<'static>>>,
    shared: Option<Arc<Shared>>,
    rows: graphlava::sparql::IdRows,
    cursor: usize,
}

impl RowsState {
    fn release(&mut self) {
        self.snap = None;
        self.shared = None;
    }
    fn remaining(&self) -> usize {
        if self.snap.is_none() {
            0
        } else {
            self.rows.rows.len() - self.cursor
        }
    }
    /// Decodes `n` rows from the cursor, column by column, and advances.
    fn decode_batch(&mut self, n: usize) -> graphlava::Result<Vec<Vec<Option<Term>>>> {
        let n = n.min(self.remaining());
        let width = self.rows.vars.len();
        let snap = match &self.snap {
            Some(s) => s.get(),
            None => return Ok(Vec::new()),
        };
        let mut cols: Vec<Vec<Option<Term>>> = Vec::with_capacity(width);
        for col in 0..width {
            let mut v = Vec::with_capacity(n);
            for row in &self.rows.rows[self.cursor..self.cursor + n] {
                let id = row[col];
                v.push(if id.is_null() {
                    None
                } else {
                    DECODES.fetch_add(1, Ordering::Relaxed);
                    Some(snap.decode_row_id(&self.rows, id)?)
                });
            }
            cols.push(v);
        }
        self.cursor += n;
        if self.cursor >= self.rows.rows.len() {
            self.release();
        }
        Ok(cols)
    }
}

/// A query result: a one-pass cursor over ID rows that decodes terms lazily,
/// column by column (rule `api-rows-late`). Iterate it for row tuples, or use
/// `iter_batches()` (up to 1024 rows per batch), `ids()` (no decoding) or
/// `to_arrow()`. Each consumes the rows it returns; an exhausted or closed
/// result yields nothing.
// @tg: implements:: [[openspec:python-binding#Python row batches]]
#[pyclass(module = "graphlava._native", name = "Rows", frozen)]
struct PyRows {
    vars: Vec<String>,
    owner_pid: u32,
    state: Mutex<RowsState>,
}

impl PyRows {
    fn batch(&self, py: Python<'_>, n: usize) -> PyResult<Vec<Vec<Option<Term>>>> {
        check_pid(py, self.owner_pid)?;
        run(py, || lock(&self.state).decode_batch(n))
    }

    fn rows_to_py(py: Python<'_>, cols: &[Vec<Option<Term>>]) -> PyResult<Vec<Py<PyAny>>> {
        let c = classes(py)?;
        let n = cols.first().map_or(0, Vec::len);
        let mut pycols = Vec::with_capacity(cols.len());
        for col in cols {
            pycols.push(
                col.iter()
                    .map(|t| opt_term_to_py(py, c, t))
                    .collect::<PyResult<Vec<_>>>()?,
            );
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let row: Vec<&Py<PyAny>> = pycols.iter().map(|c| &c[i]).collect();
            out.push(PyTuple::new(py, row)?.into_any().unbind());
        }
        Ok(out)
    }
}

#[pymethods]
impl PyRows {
    /// The projected variable names.
    #[getter]
    fn vars(&self) -> Vec<String> {
        self.vars.clone()
    }

    fn __iter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        if self.vars.is_empty() {
            // a zero-width result still has rows; yield empty tuples
            let mut g = lock(&self.state);
            if g.remaining() == 0 {
                return Err(PyStopIteration::new_err(()));
            }
            g.cursor += 1;
            if g.remaining() == 0 {
                g.release();
            }
            return Ok(PyTuple::empty(py).into_any().unbind());
        }
        let cols = self.batch(py, 1)?;
        if cols.first().is_none_or(Vec::is_empty) {
            return Err(PyStopIteration::new_err(()));
        }
        Ok(Self::rows_to_py(py, &cols)?.remove(0))
    }

    /// Batches of up to `size` row tuples (default 1024, the internal batch size).
    #[pyo3(signature = (size=BATCH))]
    fn iter_batches(slf: Py<Self>, size: usize) -> PyResult<PyBatches> {
        if size == 0 {
            return Err(PyValueError::new_err("size must be at least 1"));
        }
        Ok(PyBatches { rows: slf, size })
    }

    /// The remaining rows as tuples of IDs (`None` when unbound), without
    /// decoding any term.
    fn ids(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        check_pid(py, self.owner_pid)?;
        let mut g = lock(&self.state);
        if g.snap.is_none() {
            return Ok(PyList::empty(py).into_any().unbind());
        }
        let start = g.cursor;
        let mut out = Vec::with_capacity(g.rows.rows.len() - start);
        for row in &g.rows.rows[start..] {
            let vals: Vec<Option<u64>> = row
                .iter()
                .map(|i| (!i.is_null()).then_some(i.raw()))
                .collect();
            out.push(PyTuple::new(py, vals)?);
        }
        g.cursor = g.rows.rows.len();
        let snap = g.snap.take();
        let shared = g.shared.take();
        drop(g);
        py.detach(move || {
            drop(snap);
            drop(shared);
        });
        Ok(PyList::new(py, out)?.into_any().unbind())
    }

    /// The remaining rows as a `pyarrow.Table`, one column per variable,
    /// decoded column by column (needs the `graphlava[arrow]` extra).
    #[cfg(feature = "arrow")]
    fn to_arrow(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let n = lock(&self.state).remaining();
        let cols = self.batch(py, n)?;
        let c = classes(py)?;
        let mut pycols = Vec::with_capacity(self.vars.len());
        for i in 0..self.vars.len() {
            let col = cols.get(i).map(Vec::as_slice).unwrap_or(&[]);
            let vals = col
                .iter()
                .map(|t| opt_term_to_py(py, c, t))
                .collect::<PyResult<Vec<_>>>()?;
            pycols.push(PyList::new(py, vals)?);
        }
        Ok(c.arrow_table
            .bind(py)
            .call1((self.vars.clone(), PyList::new(py, pycols)?))?
            .unbind())
    }

    /// The number of rows not yet consumed.
    fn __len__(&self) -> usize {
        lock(&self.state).remaining()
    }

    /// Releases the snapshot; further iteration yields nothing.
    fn close(&self, py: Python<'_>) {
        let mut g = lock(&self.state);
        let snap = g.snap.take();
        let shared = g.shared.take();
        drop(g);
        py.detach(move || {
            drop(snap);
            drop(shared);
        });
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    #[pyo3(signature = (*_exc))]
    fn __exit__(&self, py: Python<'_>, _exc: &Bound<'_, PyTuple>) -> bool {
        self.close(py);
        false
    }
}

/// Iterator returned by `Rows.iter_batches()`.
#[pyclass(module = "graphlava._native", name = "RowBatches", frozen)]
struct PyBatches {
    rows: Py<PyRows>,
    size: usize,
}

#[pymethods]
impl PyBatches {
    fn __iter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let rows = self.rows.get();
        let cols = rows.batch(py, self.size)?;
        if cols.first().is_none_or(Vec::is_empty) {
            return Err(PyStopIteration::new_err(()));
        }
        Ok(PyList::new(py, PyRows::rows_to_py(py, &cols)?)?
            .into_any()
            .unbind())
    }
}

// ---- admin ---------------------------------------------------------------------------------

/// Administration: dictionary sweep and on-demand structures.
#[pyclass(module = "graphlava._native", name = "Admin", frozen)]
struct PyAdmin {
    shared: Owned<Arc<Shared>>,
}

impl PyAdmin {
    fn db<T: Send>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&Database) -> graphlava::Result<T> + Send,
    ) -> PyResult<T> {
        let s = self.shared.get();
        check_pid(py, s.owner_pid())?;
        run(py, || f(s.db()))
    }
}

#[pymethods]
impl PyAdmin {
    /// Runs a dictionary sweep, spending at most `window_ms` between mark and sweep.
    #[pyo3(signature = (window_ms=50))]
    fn sweep(&self, py: Python<'_>, window_ms: u64) -> PyResult<Py<PyAny>> {
        let r = self.db(py, |d| d.admin().sweep(Duration::from_millis(window_ms)))?;
        Ok(classes(py)?
            .sweep
            .bind(py)
            .call1((
                r.marked,
                r.examined,
                r.deleted,
                r.kept_young,
                r.kept_rereferenced,
            ))?
            .unbind())
    }

    /// Whether enough garbage accumulated for a sweep to be worth it.
    fn sweep_due(&self, py: Python<'_>) -> PyResult<bool> {
        self.db(py, |d| d.admin().sweep_due())
    }

    /// Builds an on-demand ordering: `"PSO"`, `"SOP"` or `"OPS"`.
    fn build_ordering(&self, py: Python<'_>, perm: &str) -> PyResult<()> {
        let perm = match perm.to_ascii_uppercase().as_str() {
            "PSO" => graphlava::Perm::Pso,
            "SOP" => graphlava::Perm::Sop,
            "OPS" => graphlava::Perm::Ops,
            other => {
                return Err(PyValueError::new_err(format!(
                    "ordering must be 'PSO', 'SOP' or 'OPS', got {other:?}"
                )));
            }
        };
        self.db(py, move |d| d.admin().build_ordering(perm))
    }

    /// Builds the value index of predicate `p`.
    fn build_value_index(&self, py: Python<'_>, p: &Bound<'_, PyAny>) -> PyResult<()> {
        let t = to_term(py, classes(py)?, p, Pos::Object)?;
        self.db(py, move |d| d.admin().build_value_index(&t))
    }
}

// ---- database ------------------------------------------------------------------------------

/// The owner handle of one database file (rule `api-single-owner`). Use it in
/// the process that opened it only; after `os.fork` the child gets
/// `ForkedHandleError`.
#[pyclass(module = "graphlava._native", name = "Database", frozen)]
struct PyDatabase {
    inner: Mutex<Option<Arc<Shared>>>,
    owner_pid: u32,
    path: String,
}

impl PyDatabase {
    fn shared(&self, py: Python<'_>) -> PyResult<Arc<Shared>> {
        check_pid(py, self.owner_pid)?;
        lock(&self.inner)
            .clone()
            .ok_or_else(|| closed(py, "the database"))
    }

    fn query(&self, py: Python<'_>, lang: Lang, text: &str) -> PyResult<Py<PyAny>> {
        let shared = self.shared(py)?;
        let q = text.to_string();
        let s2 = Arc::clone(&shared);
        let (txn, rows) = run(py, move || {
            // SAFETY: the snapshot is stored in `RowsState` next to `shared`
            // and dropped first (see `TxnState`).
            let db = unsafe { extend(s2.db()) };
            let r = db.query_ids(lang, &q);
            drop(s2);
            r
        })?;
        let owner_pid = shared.owner_pid();
        if lang == Lang::Sparql && rows.vars == ["ask"] && rows.rows.len() == 1 && is_ask(text) {
            if let Ok(Term::Literal(l)) = txn.decode_row_id(&rows, rows.rows[0][0]) {
                py.detach(move || drop(txn));
                return (l.lexical == "true").into_py_any(py);
            }
        }
        let vars = rows.vars.clone();
        let empty = rows.rows.is_empty();
        let rows_obj = PyRows {
            vars,
            owner_pid,
            state: Mutex::new(RowsState {
                snap: if empty { None } else { Some(Owned::new(txn)) },
                shared: if empty { None } else { Some(shared) },
                rows,
                cursor: 0,
            }),
        };
        Ok(Py::new(py, rows_obj)?.into_any())
    }
}

fn is_ask(text: &str) -> bool {
    matches!(
        graphlava::sparql::parse_query(text, None),
        Ok(spargebra::Query::Ask { .. })
    )
}

#[pymethods]
impl PyDatabase {
    /// Opens or creates a database file and takes its OS lock for the life of
    /// the handle. `LockedError` if another process or handle holds it.
    #[staticmethod]
    #[pyo3(signature = (path, *, write_timeout_ms=None, group_window_ms=None, cache_bytes=None, graph_depth_limit=None))]
    fn open(
        py: Python<'_>,
        path: std::path::PathBuf,
        write_timeout_ms: Option<u64>,
        group_window_ms: Option<u64>,
        cache_bytes: Option<usize>,
        graph_depth_limit: Option<u32>,
    ) -> PyResult<PyDatabase> {
        let mut opts = OpenOptions::default();
        if let Some(ms) = write_timeout_ms {
            opts.write_timeout = Duration::from_millis(ms);
        }
        if let Some(ms) = group_window_ms {
            opts.group_window = Duration::from_millis(ms);
        }
        if let Some(b) = cache_bytes {
            opts.cache_bytes = b;
        }
        if let Some(l) = graph_depth_limit {
            opts.graph_depth_limit = l;
        }
        let shown = path.display().to_string();
        let db = run(py, move || Database::open(&path, opts))?;
        let shared = Shared { db: Owned::new(db) };
        Ok(PyDatabase {
            owner_pid: shared.owner_pid(),
            inner: Mutex::new(Some(Arc::new(shared))),
            path: shown,
        })
    }

    /// The file path given to `open`.
    #[getter]
    fn path(&self) -> String {
        self.path.clone()
    }

    /// Whether `close()` was called.
    #[getter]
    fn closed(&self) -> bool {
        lock(&self.inner).is_none()
    }

    /// A read snapshot; use it as `with db.read() as r:`.
    fn read(&self, py: Python<'_>) -> PyResult<PyReadTxn> {
        let shared = self.shared(py)?;
        let s2 = Arc::clone(&shared);
        let txn = run(py, move || {
            // SAFETY: stored in `TxnState::Read` next to `shared`, dropped first.
            let db = unsafe { extend(s2.db()) };
            let t = db.read();
            drop(s2);
            t
        })?;
        Ok(PyReadTxn {
            state: Mutex::new(TxnState::Read(Owned::new(txn), shared)),
        })
    }

    /// An interactive write transaction: `with db.write(Durability.SYNC) as w:`.
    /// It holds the single writer for at most the configured time limit
    /// (default 100 ms); prefer `submit` for writes.
    #[pyo3(signature = (durability=None))]
    fn write(
        &self,
        py: Python<'_>,
        durability: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyWriteTxn>> {
        let d = parse_durability(py, durability)?;
        let shared = self.shared(py)?;
        let s2 = Arc::clone(&shared);
        let w = run(py, move || {
            // SAFETY: stored in `TxnState::Write` next to `shared`, dropped first.
            let db = unsafe { extend(s2.db()) };
            let w = db.write(d);
            drop(s2);
            w
        })?;
        let init = PyClassInitializer::from(PyReadTxn {
            state: Mutex::new(TxnState::Write(Owned::new(w), shared)),
        })
        .add_subclass(PyWriteTxn {});
        Py::new(py, init)
    }

    /// Runs a one-shot write request (a list of operation objects): all
    /// operations apply or none, acknowledged once `durability` holds.
    #[pyo3(signature = (ops, durability=None))]
    fn submit(
        &self,
        py: Python<'_>,
        ops: &Bound<'_, PyAny>,
        durability: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let d = parse_durability(py, durability)?;
        let c = classes(py)?;
        let mut req = WriteRequest::default();
        for op in ops.try_iter()? {
            req.ops.push(to_core_op(py, c, &op?)?);
        }
        let shared = self.shared(py)?;
        let t = run(py, move || shared.db().submit(req, d))?;
        commit_result(py, &t.results, t.batch)
    }

    /// A SPARQL query on a fresh snapshot: `Rows`, or a `bool` for `ASK`.
    fn sparql(&self, py: Python<'_>, query: &str) -> PyResult<Py<PyAny>> {
        self.query(py, Lang::Sparql, query)
    }

    /// A read-only Datalog program on a fresh snapshot: `Rows`.
    fn datalog(&self, py: Python<'_>, program: &str) -> PyResult<Py<PyAny>> {
        self.query(py, Lang::Datalog, program)
    }

    /// Runs a SPARQL update request as one transaction.
    #[pyo3(signature = (update, durability=None))]
    fn sparql_update(
        &self,
        py: Python<'_>,
        update: &str,
        durability: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let d = parse_durability(py, durability)?;
        let shared = self.shared(py)?;
        let u = update.to_string();
        let rep = run(py, move || shared.db().sparql_update(&u, d))?;
        Ok(classes(py)?
            .update_report
            .bind(py)
            .call1((rep.inserted, rep.deleted, rep.updated_in_place))?
            .unbind())
    }

    /// Runs a Datalog program whose `@assert` heads are written, all or nothing.
    #[pyo3(signature = (program, durability=None))]
    fn datalog_update(
        &self,
        py: Python<'_>,
        program: &str,
        durability: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let d = parse_durability(py, durability)?;
        let shared = self.shared(py)?;
        let p = program.to_string();
        let r = run(py, move || shared.db().datalog_update(&p, d))?;
        query_result_to_py(py, &r)
    }

    /// Administration: sweep and on-demand structures.
    fn admin(&self, py: Python<'_>) -> PyResult<PyAdmin> {
        Ok(PyAdmin {
            shared: Owned::new(self.shared(py)?),
        })
    }

    /// Size of the database file in bytes.
    fn file_size(&self, py: Python<'_>) -> PyResult<u64> {
        let shared = self.shared(py)?;
        run(py, move || shared.db().file_size())
    }

    /// Commits any open batch and closes the file. If transactions or results
    /// are still open, the file closes when the last of them is released.
    fn close(&self, py: Python<'_>) -> PyResult<()> {
        check_pid(py, self.owner_pid)?;
        let Some(arc) = lock(&self.inner).take() else {
            return Ok(());
        };
        match Arc::try_unwrap(arc) {
            Ok(shared) => {
                let db = shared.db.take();
                run(py, move || db.close())
            }
            Err(arc) => {
                py.detach(move || drop(arc));
                Ok(())
            }
        }
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    #[pyo3(signature = (*_exc))]
    fn __exit__(&self, py: Python<'_>, _exc: &Bound<'_, PyTuple>) -> PyResult<bool> {
        self.close(py)?;
        Ok(false)
    }

    fn __repr__(&self) -> String {
        format!(
            "Database({:?}{})",
            self.path,
            if self.closed() { ", closed" } else { "" }
        )
    }
}

impl Drop for PyDatabase {
    fn drop(&mut self) {
        // Garbage-collected without close(): the last `Arc` drops the
        // `Database`, which flushes the batch and releases the lock — in the
        // owner process only (`Owned`).
        let _ = self.inner.get_mut().map(Option::take);
    }
}

/// The version of the graphlava core crate this module was built with.
#[pyfunction]
fn core_version() -> &'static str {
    graphlava::VERSION
}

#[pyfunction]
fn _debug_decode_count() -> u64 {
    DECODES.load(Ordering::Relaxed)
}

#[pyfunction]
fn _debug_panic(py: Python<'_>) -> PyResult<()> {
    run(py, || -> graphlava::Result<()> {
        panic!("graphlava._debug.panic() was called")
    })
}

#[pymodule(gil_used = false)]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDatabase>()?;
    m.add_class::<PyReadTxn>()?;
    m.add_class::<PyWriteTxn>()?;
    m.add_class::<PyRows>()?;
    m.add_class::<PyBatches>()?;
    m.add_class::<PyAdmin>()?;
    m.add_function(wrap_pyfunction!(core_version, m)?)?;
    m.add_function(wrap_pyfunction!(_debug_decode_count, m)?)?;
    m.add_function(wrap_pyfunction!(_debug_panic, m)?)?;
    Ok(())
}
