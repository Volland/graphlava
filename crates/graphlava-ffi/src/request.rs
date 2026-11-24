//! Write requests built through the C API, their conversion to core
//! operations, and their byte encoding (the payload of an extension journal
//! frame, [[16-api#Mobile bindings]]).
//!
//! A request is either a list of operations, each an op code with arguments,
//! or the text of a SPARQL Update or a Datalog program
//! ([[16-api#Write requests]]). Encoding (all integers little-endian):
//!
//! ```text
//! request := version:u8 (=1) kind:u8 body
//! kind 1  := count:u32 op*            op  := code:u32 nargs:u32 arg*
//! kind 2  := str (SPARQL Update)      kind 3 := str (Datalog program)
//! arg     := tag:u8 payload           str := len:u32 utf8[len]
//!   tag 0 none | 1 iri str | 2 blank str | 3 literal str flags:u8 [str] [str]
//!   | 5 id u64 | 6 uint u64           (literal flags: bit 0 datatype, bit 1 lang)
//! ```

use graphlava::{
    CoreOp, DeleteMode, Error, HedgeTypeDecl, Id, Literal, PredicateDecl, RequestBody, RoleSpec,
    Term, TermRef, WriteRequest,
};

/// Version byte of the request encoding.
pub(crate) const CODEC_VERSION: u8 = 1;
/// Upper bound on one decoded string or collection, so corrupt input cannot
/// make the decoder allocate without limit.
const MAX_LEN: usize = 64 << 20;

/// One argument of an operation, owned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    None,
    Iri(String),
    Blank(String),
    Literal {
        lexical: String,
        datatype: Option<String>,
        lang: Option<String>,
    },
    Id(u64),
    Uint(u64),
}

/// A request under construction or decoded from bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Ops(Vec<(u32, Vec<Arg>)>),
    Sparql(String),
    Datalog(String),
}

// Op codes: the C constants `GRAPHLAVA_OP_*` of lib.rs.
pub(crate) const OP_INSERT: u32 = crate::GRAPHLAVA_OP_INSERT;
pub(crate) const OP_SET: u32 = crate::GRAPHLAVA_OP_SET;
pub(crate) const OP_UPDATE: u32 = crate::GRAPHLAVA_OP_UPDATE;
pub(crate) const OP_ANNOTATE: u32 = crate::GRAPHLAVA_OP_ANNOTATE;
pub(crate) const OP_DELETE: u32 = crate::GRAPHLAVA_OP_DELETE;
pub(crate) const OP_REMOVE: u32 = crate::GRAPHLAVA_OP_REMOVE;
pub(crate) const OP_STATEMENT_ID: u32 = crate::GRAPHLAVA_OP_STATEMENT_ID;
pub(crate) const OP_INSERT_UNASSERTED: u32 = crate::GRAPHLAVA_OP_INSERT_UNASSERTED;
pub(crate) const OP_DECLARE_PREDICATE: u32 = crate::GRAPHLAVA_OP_DECLARE_PREDICATE;
pub(crate) const OP_CREATE_GRAPH: u32 = crate::GRAPHLAVA_OP_CREATE_GRAPH;
pub(crate) const OP_ADD_MEMBER: u32 = crate::GRAPHLAVA_OP_ADD_MEMBER;
pub(crate) const OP_REMOVE_MEMBER: u32 = crate::GRAPHLAVA_OP_REMOVE_MEMBER;
pub(crate) const OP_MOVE_MEMBER: u32 = crate::GRAPHLAVA_OP_MOVE_MEMBER;
pub(crate) const OP_MEMBERSHIP_ID: u32 = crate::GRAPHLAVA_OP_MEMBERSHIP_ID;
pub(crate) const OP_NEST: u32 = crate::GRAPHLAVA_OP_NEST;
pub(crate) const OP_UNNEST: u32 = crate::GRAPHLAVA_OP_UNNEST;
pub(crate) const OP_DROP_GRAPH: u32 = crate::GRAPHLAVA_OP_DROP_GRAPH;
pub(crate) const OP_DECLARE_HEDGE_TYPE: u32 = crate::GRAPHLAVA_OP_DECLARE_HEDGE_TYPE;
pub(crate) const OP_CREATE_HEDGE: u32 = crate::GRAPHLAVA_OP_CREATE_HEDGE;
pub(crate) const OP_UPDATE_ROLE: u32 = crate::GRAPHLAVA_OP_UPDATE_ROLE;
pub(crate) const OP_ROLE_ID: u32 = crate::GRAPHLAVA_OP_ROLE_ID;

fn bad(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

fn term(a: &Arg, what: &str) -> Result<TermRef, Error> {
    Ok(match a {
        Arg::Iri(i) => TermRef::Term(Term::iri(i.as_str())),
        Arg::Blank(b) => TermRef::Term(Term::Blank(b.clone())),
        Arg::Literal {
            lexical,
            datatype,
            lang,
        } => TermRef::Term(Term::lit(match (datatype, lang) {
            (_, Some(tag)) => Literal::lang(lexical.as_str(), tag.as_str()),
            (Some(dt), None) => Literal::typed(lexical.as_str(), dt.as_str()),
            (None, None) => Literal::string(lexical.as_str()),
        })),
        Arg::Id(0) => return Err(bad(format!("{what}: ID 0 names nothing"))),
        Arg::Id(i) => TermRef::Id(Id(*i)),
        Arg::None | Arg::Uint(_) => return Err(bad(format!("{what}: expected a term or an ID"))),
    })
}

fn opt_term(a: Option<&Arg>, what: &str) -> Result<Option<TermRef>, Error> {
    match a {
        None | Some(Arg::None) => Ok(None),
        Some(a) => term(a, what).map(Some),
    }
}

fn id(a: &Arg, what: &str) -> Result<Id, Error> {
    match a {
        Arg::Id(i) if *i != 0 => Ok(Id(*i)),
        _ => Err(bad(format!("{what}: expected a non-zero ID"))),
    }
}

fn uint(a: Option<&Arg>, what: &str) -> Result<u64, Error> {
    match a {
        None | Some(Arg::None) => Ok(0),
        Some(Arg::Uint(v)) => Ok(*v),
        Some(_) => Err(bad(format!("{what}: expected an unsigned integer"))),
    }
}

fn mode(v: u64) -> Result<DeleteMode, Error> {
    match v {
        0 => Ok(DeleteMode::Cascade),
        1 => Ok(DeleteMode::KeepAnnotations),
        2 => Ok(DeleteMode::Detach),
        m => Err(bad(format!("unknown delete mode {m}"))),
    }
}

fn arity(args: &[Arg], min: usize, max: usize, name: &str) -> Result<(), Error> {
    if args.len() < min || args.len() > max {
        return Err(bad(format!(
            "{name} takes {min}..={max} arguments, got {}",
            args.len()
        )));
    }
    Ok(())
}

/// Converts one operation to its core form, checking arity and argument kinds.
pub fn to_core(code: u32, a: &[Arg]) -> Result<CoreOp, Error> {
    Ok(match code {
        OP_INSERT => {
            arity(a, 3, 4, "INSERT")?;
            CoreOp::Insert {
                s: term(&a[0], "s")?,
                p: term(&a[1], "p")?,
                o: term(&a[2], "o")?,
                graph: opt_term(a.get(3), "graph")?,
            }
        }
        OP_SET => {
            arity(a, 3, 3, "SET")?;
            CoreOp::Set {
                s: term(&a[0], "s")?,
                p: term(&a[1], "p")?,
                o: term(&a[2], "o")?,
            }
        }
        OP_UPDATE => {
            arity(a, 2, 3, "UPDATE")?;
            CoreOp::Update {
                id: id(&a[0], "id")?,
                o: term(&a[1], "o")?,
                expected: opt_term(a.get(2), "expected")?,
            }
        }
        OP_ANNOTATE => {
            arity(a, 3, 3, "ANNOTATE")?;
            CoreOp::Annotate {
                id: id(&a[0], "id")?,
                p: term(&a[1], "p")?,
                o: term(&a[2], "o")?,
            }
        }
        OP_DELETE => {
            arity(a, 1, 2, "DELETE")?;
            CoreOp::Delete {
                id: id(&a[0], "id")?,
                mode: mode(uint(a.get(1), "mode")?)?,
            }
        }
        OP_REMOVE => {
            arity(a, 3, 5, "REMOVE")?;
            CoreOp::Remove {
                s: term(&a[0], "s")?,
                p: term(&a[1], "p")?,
                o: term(&a[2], "o")?,
                mode: mode(uint(a.get(3), "mode")?)?,
                graph: opt_term(a.get(4), "graph")?,
            }
        }
        OP_STATEMENT_ID | OP_INSERT_UNASSERTED => {
            arity(a, 3, 3, "STATEMENT_ID")?;
            let (s, p, o) = (term(&a[0], "s")?, term(&a[1], "p")?, term(&a[2], "o")?);
            if code == OP_STATEMENT_ID {
                CoreOp::StatementId { s, p, o }
            } else {
                CoreOp::InsertUnasserted { s, p, o }
            }
        }
        OP_DECLARE_PREDICATE => {
            arity(a, 2, 2, "DECLARE_PREDICATE")?;
            let f = uint(a.get(1), "flags")?;
            if f & !3 != 0 {
                return Err(bad(format!("unknown predicate flags {f:#x}")));
            }
            CoreOp::DeclarePredicate {
                p: term(&a[0], "p")?,
                decl: PredicateDecl {
                    multi: f & 1 != 0,
                    functional: f & 2 != 0,
                },
            }
        }
        OP_CREATE_GRAPH => {
            arity(a, 1, 1, "CREATE_GRAPH")?;
            CoreOp::CreateGraph {
                name: term(&a[0], "name")?,
            }
        }
        OP_ADD_MEMBER | OP_REMOVE_MEMBER | OP_MEMBERSHIP_ID => {
            arity(a, 2, 2, "membership")?;
            let (item, graph) = (id(&a[0], "item")?, term(&a[1], "graph")?);
            match code {
                OP_ADD_MEMBER => CoreOp::AddMember { item, graph },
                OP_REMOVE_MEMBER => CoreOp::RemoveMember { item, graph },
                _ => CoreOp::MembershipId { item, graph },
            }
        }
        OP_MOVE_MEMBER => {
            arity(a, 3, 3, "MOVE_MEMBER")?;
            CoreOp::MoveMember {
                item: id(&a[0], "item")?,
                from: term(&a[1], "from")?,
                to: term(&a[2], "to")?,
            }
        }
        OP_NEST | OP_UNNEST => {
            arity(a, 2, 2, "NEST")?;
            let (child, parent) = (term(&a[0], "child")?, term(&a[1], "parent")?);
            if code == OP_NEST {
                CoreOp::Nest { child, parent }
            } else {
                CoreOp::Unnest { child, parent }
            }
        }
        OP_DROP_GRAPH => {
            arity(a, 1, 2, "DROP_GRAPH")?;
            CoreOp::DropGraph {
                graph: term(&a[0], "graph")?,
                recursive: uint(a.get(1), "recursive")? != 0,
            }
        }
        OP_DECLARE_HEDGE_TYPE => {
            // ty, nroles, (role, flags) * nroles, attr*
            if a.len() < 2 {
                return Err(bad("DECLARE_HEDGE_TYPE takes ty, nroles, ..."));
            }
            let n = uint(a.get(1), "nroles")? as usize;
            if a.len() < 2 + 2 * n {
                return Err(bad("DECLARE_HEDGE_TYPE: fewer role arguments than nroles"));
            }
            let mut roles = Vec::with_capacity(n);
            for i in 0..n {
                let f = uint(a.get(2 + 2 * i + 1), "role flags")?;
                if f & !3 != 0 {
                    return Err(bad(format!("unknown role flags {f:#x}")));
                }
                roles.push(RoleSpec {
                    required: f & 1 != 0,
                    many: f & 2 != 0,
                    ..RoleSpec::one(term(&a[2 + 2 * i], "role")?)
                });
            }
            let attrs = a[2 + 2 * n..]
                .iter()
                .map(|x| term(x, "attr"))
                .collect::<Result<_, _>>()?;
            CoreOp::DeclareHedgeType {
                ty: term(&a[0], "ty")?,
                decl: HedgeTypeDecl { roles, attrs },
            }
        }
        OP_CREATE_HEDGE => {
            // ty, graph|none, nroles, (role, filler) * nroles, (attr, value)*
            if a.len() < 3 {
                return Err(bad("CREATE_HEDGE takes ty, graph, nroles, ..."));
            }
            let n = uint(a.get(2), "nroles")? as usize;
            let rest = a.len() - 3;
            if rest < 2 * n || !(rest - 2 * n).is_multiple_of(2) {
                return Err(bad(
                    "CREATE_HEDGE: role and attribute arguments come in pairs",
                ));
            }
            let pairs = |xs: &[Arg]| -> Result<Vec<(TermRef, TermRef)>, Error> {
                xs.chunks_exact(2)
                    .map(|c| Ok((term(&c[0], "role")?, term(&c[1], "filler")?)))
                    .collect()
            };
            CoreOp::CreateHedge {
                ty: term(&a[0], "ty")?,
                graph: opt_term(a.get(1), "graph")?,
                roles: pairs(&a[3..3 + 2 * n])?,
                attrs: pairs(&a[3 + 2 * n..])?,
            }
        }
        OP_UPDATE_ROLE | OP_ROLE_ID => {
            arity(a, 3, 3, "UPDATE_ROLE")?;
            let (hedge, role, node) = (
                id(&a[0], "hedge")?,
                term(&a[1], "role")?,
                term(&a[2], "node")?,
            );
            if code == OP_UPDATE_ROLE {
                CoreOp::UpdateRole {
                    hedge,
                    role,
                    filler: node,
                }
            } else {
                CoreOp::RoleId { hedge, role, node }
            }
        }
        c => return Err(bad(format!("unknown op code {c}"))),
    })
}

impl Request {
    /// The core form of the request.
    pub fn to_body(&self) -> Result<RequestBody, Error> {
        Ok(match self {
            Request::Ops(ops) => RequestBody::Ops(WriteRequest {
                ops: ops
                    .iter()
                    .map(|(c, a)| to_core(*c, a))
                    .collect::<Result<_, _>>()?,
            }),
            Request::Sparql(t) => RequestBody::SparqlUpdate(t.clone()),
            Request::Datalog(t) => RequestBody::Datalog(t.clone()),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![CODEC_VERSION];
        let s = |out: &mut Vec<u8>, t: &str| {
            out.extend_from_slice(&(t.len() as u32).to_le_bytes());
            out.extend_from_slice(t.as_bytes());
        };
        match self {
            Request::Ops(ops) => {
                out.push(1);
                out.extend_from_slice(&(ops.len() as u32).to_le_bytes());
                for (code, args) in ops {
                    out.extend_from_slice(&code.to_le_bytes());
                    out.extend_from_slice(&(args.len() as u32).to_le_bytes());
                    for a in args {
                        match a {
                            Arg::None => out.push(0),
                            Arg::Iri(t) => {
                                out.push(1);
                                s(&mut out, t);
                            }
                            Arg::Blank(t) => {
                                out.push(2);
                                s(&mut out, t);
                            }
                            Arg::Literal {
                                lexical,
                                datatype,
                                lang,
                            } => {
                                out.push(3);
                                s(&mut out, lexical);
                                out.push(
                                    u8::from(datatype.is_some()) | (u8::from(lang.is_some()) << 1),
                                );
                                if let Some(d) = datatype {
                                    s(&mut out, d);
                                }
                                if let Some(l) = lang {
                                    s(&mut out, l);
                                }
                            }
                            Arg::Id(v) => {
                                out.push(5);
                                out.extend_from_slice(&v.to_le_bytes());
                            }
                            Arg::Uint(v) => {
                                out.push(6);
                                out.extend_from_slice(&v.to_le_bytes());
                            }
                        }
                    }
                }
            }
            Request::Sparql(t) => {
                out.push(2);
                s(&mut out, t);
            }
            Request::Datalog(t) => {
                out.push(3);
                s(&mut out, t);
            }
        }
        out
    }

    /// Decodes bytes from [`Request::encode`]. Every malformed input, however
    /// short or long, gives an error, never a panic.
    pub fn decode(b: &[u8]) -> Result<Request, Error> {
        let mut r = Reader { b, at: 0 };
        if r.u8()? != CODEC_VERSION {
            return Err(bad("unknown request encoding version"));
        }
        let req = match r.u8()? {
            1 => {
                let n = r.len()?;
                let mut ops = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    let code = r.u32()?;
                    let na = r.len()?;
                    let mut args = Vec::with_capacity(na.min(64));
                    for _ in 0..na {
                        args.push(match r.u8()? {
                            0 => Arg::None,
                            1 => Arg::Iri(r.str()?),
                            2 => Arg::Blank(r.str()?),
                            3 => {
                                let lexical = r.str()?;
                                let f = r.u8()?;
                                if f & !3 != 0 {
                                    return Err(bad("bad literal flags"));
                                }
                                let datatype = if f & 1 != 0 { Some(r.str()?) } else { None };
                                let lang = if f & 2 != 0 { Some(r.str()?) } else { None };
                                Arg::Literal {
                                    lexical,
                                    datatype,
                                    lang,
                                }
                            }
                            5 => Arg::Id(r.u64()?),
                            6 => Arg::Uint(r.u64()?),
                            t => return Err(bad(format!("unknown argument tag {t}"))),
                        });
                    }
                    ops.push((code, args));
                }
                Request::Ops(ops)
            }
            2 => Request::Sparql(r.str()?),
            3 => Request::Datalog(r.str()?),
            k => return Err(bad(format!("unknown request kind {k}"))),
        };
        if r.at != b.len() {
            return Err(bad("trailing bytes after the request"));
        }
        Ok(req)
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self
            .at
            .checked_add(n)
            .filter(|e| *e <= self.b.len())
            .ok_or_else(|| bad("truncated request"))?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4")))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8")))
    }
    fn len(&mut self) -> Result<usize, Error> {
        let n = self.u32()? as usize;
        if n > MAX_LEN {
            return Err(bad("length out of range"));
        }
        Ok(n)
    }
    fn str(&mut self) -> Result<String, Error> {
        let n = self.len()?;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| bad("string is not UTF-8"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Request {
        Request::Ops(vec![
            (
                OP_INSERT,
                vec![
                    Arg::Iri("http://ex.org/a".into()),
                    Arg::Iri("http://ex.org/p".into()),
                    Arg::Literal {
                        lexical: "hi".into(),
                        datatype: None,
                        lang: Some("en".into()),
                    },
                    Arg::None,
                ],
            ),
            (OP_DELETE, vec![Arg::Id(7), Arg::Uint(2)]),
        ])
    }

    #[test]
    fn round_trip() {
        for r in [
            sample(),
            Request::Sparql("INSERT DATA {}".into()),
            Request::Datalog("p(1).".into()),
        ] {
            assert_eq!(Request::decode(&r.encode()).unwrap(), r);
        }
    }

    #[test]
    fn every_prefix_and_trailing_byte_is_rejected() {
        let b = sample().encode();
        for n in 0..b.len() {
            assert!(Request::decode(&b[..n]).is_err(), "prefix {n}");
        }
        let mut more = b.clone();
        more.push(0);
        assert!(Request::decode(&more).is_err());
    }

    #[test]
    fn arity_and_kinds_are_checked() {
        assert!(to_core(OP_INSERT, &[Arg::Iri("a".into())]).is_err());
        assert!(to_core(OP_DELETE, &[Arg::Id(0)]).is_err());
        assert!(to_core(OP_DELETE, &[Arg::Id(1), Arg::Uint(9)]).is_err());
        assert!(to_core(99, &[]).is_err());
        assert!(
            to_core(
                OP_CREATE_HEDGE,
                &[Arg::Iri("t".into()), Arg::None, Arg::Uint(1)]
            )
            .is_err()
        );
        assert!(sample().to_body().is_ok());
    }
}
