"""Runs the shared conformance corpus ``bindings/conformance/*.jsonl`` (ADR 0042)
through the Python binding: the cases for every host (``*``) and for ``python``."""

# @tg: verifies:: [[openspec:python-binding#Python API parity]] {level: conformance}
# @tg: verifies:: [[openspec:python-binding#Python term mapping]] {level: conformance}
# @tg: verifies:: [[openspec:python-binding#Python row batches]] {level: conformance}

from __future__ import annotations

import datetime as dt
import json
import math
import pathlib
from decimal import Decimal
from typing import Any

import pytest

import graphlava as gl

CORPUS = pathlib.Path(__file__).resolve().parents[3] / "bindings" / "conformance"
XSD = "http://www.w3.org/2001/XMLSchema#"
EX = "http://ex.org/"


def load() -> list[dict[str, Any]]:
    cases = []
    for f in sorted(CORPUS.glob("*.jsonl")):
        for line in f.read_text(encoding="utf-8").splitlines():
            if line.strip():
                cases.append(json.loads(line))
    return [c for c in cases if {"*", "python"} & set(c.get("hosts", ["*"]))]


CASES = load()


class Ctx:
    def __init__(self, path: pathlib.Path) -> None:
        self.path = path
        self.db = gl.Database.open(path)
        self.names: dict[str, gl.Id] = {}

    def reopen(self) -> None:
        self.db.close()
        self.db = gl.Database.open(self.path)

    def term(self, t: dict[str, Any]) -> Any:
        if "iri" in t:
            return gl.IRI(t["iri"])
        if "bnode" in t:
            return gl.BNode(t["bnode"])
        if "ref" in t:
            return self.names[t["ref"]]
        if "triple" in t:
            return gl.Triple(*(self.term(x) for x in t["triple"]))
        if "lang" in t:
            return gl.Literal(t["literal"], lang=t["lang"])
        return gl.Literal(t["literal"], t.get("datatype", XSD + "string"))


def native(n: dict[str, Any]) -> Any:
    ty, v = n["type"], n["value"]
    if ty == "int":
        return int(v)
    if ty == "float":
        return {"NaN": math.nan, "INF": math.inf, "-INF": -math.inf}.get(v, None) if v in ("NaN", "INF", "-INF") else float(v)
    if ty == "bool":
        return bool(v)
    if ty == "str":
        return str(v)
    if ty == "decimal":
        return Decimal(v)
    if ty == "datetime":
        return dt.datetime.fromisoformat(v.replace("Z", "+00:00"))
    if ty == "naive_datetime":
        return dt.datetime.fromisoformat(v)
    if ty == "date":
        return dt.date.fromisoformat(v)
    raise KeyError(ty)


def as_term_json(v: Any) -> dict[str, Any]:
    """The corpus JSON form of a decoded Python value (inverse of the mapping)."""
    if isinstance(v, gl.IRI):
        return {"iri": v.value}
    if isinstance(v, gl.BNode):
        return {"bnode": v.label}
    if isinstance(v, gl.Literal):
        if v.lang is not None:
            return {"literal": v.lexical, "lang": v.lang}
        if v.datatype == XSD + "string":
            return {"literal": v.lexical}
        return {"literal": v.lexical, "datatype": v.datatype}
    raise TypeError(v)


def same_native(got: Any, want: Any) -> bool:
    if type(got) is not type(want):
        return False
    if isinstance(want, float):
        return got == want and math.copysign(1, got) == math.copysign(1, want)
    return bool(got == want)


def norm(t: dict[str, Any]) -> dict[str, Any]:
    t = dict(t)
    if t.get("datatype") == XSD + "string":
        del t["datatype"]
    return t


def py_value(t: dict[str, Any], ctx: Ctx) -> Any:  # noqa: C901
    """The Python value the binding decodes a corpus term to (the mapping of
    tg.md/16-api.md#Python binding)."""
    if "literal" not in t or "lang" in t:
        return ctx.term(t)
    lex, dtp = t["literal"], t.get("datatype", XSD + "string")
    try:
        return _py_value(lex, dtp)
    except ValueError:  # ill-typed or out of the Python type's range
        return gl.Literal(lex, dtp)


def _py_value(lex: str, dtp: str) -> Any:
    local = dtp[len(XSD):] if dtp.startswith(XSD) else None
    if local == "string":
        return lex
    if local == "integer":
        return int(lex)
    if local == "boolean":
        return lex == "true"
    if local == "double":
        return {"INF": math.inf, "-INF": -math.inf, "NaN": math.nan}.get(lex) if lex in ("INF", "-INF", "NaN") else float(lex)
    if local == "decimal":
        return Decimal(lex)
    if local == "dateTime" and lex.endswith("Z"):
        return dt.datetime.fromisoformat(lex[:-1] + "+00:00")
    if local == "date" and not lex.endswith("Z"):
        return dt.date.fromisoformat(lex)
    return gl.Literal(lex, dtp)


def decoded_matches(got: Any, want_term: dict[str, Any], ctx: Ctx) -> bool:
    want = py_value(want_term, ctx)
    if isinstance(want, float) and math.isnan(want):
        return isinstance(got, float) and math.isnan(got)
    return same_native(got, want)


def run_ops(ctx: Ctx, ops: list[dict[str, Any]]) -> None:
    for i, op in enumerate(ops):
        name = op["op"]
        if name == "reopen":
            ctx.reopen()
            continue
        t = lambda k: ctx.term(op[k])  # noqa: E731
        opt = lambda k: ctx.term(op[k]) if k in op else None  # noqa: E731
        ref = lambda: ctx.names[op["id"]]  # noqa: E731
        mode = op.get("mode", "cascade")
        o: gl.Op
        if name == "insert":
            o = gl.Insert(t("s"), t("p"), t("o"), graph=opt("graph"))
        elif name == "set":
            o = gl.Set(t("s"), t("p"), t("o"))
        elif name == "update":
            o = gl.Update(ref(), t("o"), expected=opt("expected"))
        elif name == "annotate":
            o = gl.Annotate(ref(), t("p"), t("o"))
        elif name == "delete":
            o = gl.Delete(ref(), mode)
        elif name == "remove":
            o = gl.Remove(t("s"), t("p"), t("o"), mode, graph=opt("graph"))
        elif name == "statement_id":
            o = gl.StatementId(t("s"), t("p"), t("o"))
        elif name == "insert_unasserted":
            o = gl.InsertUnasserted(t("s"), t("p"), t("o"))
        elif name == "declare_predicate":
            o = gl.DeclarePredicate(t("p"), multi=op.get("multi", False), functional=op.get("functional", False))
        elif name == "create_graph":
            o = gl.CreateGraph(t("graph"))
        elif name == "add_member":
            o = gl.AddMember(ref(), t("graph"))
        elif name == "remove_member":
            o = gl.RemoveMember(ref(), t("graph"))
        elif name == "nest":
            o = gl.Nest(t("child"), t("parent"))
        elif name == "unnest":
            o = gl.Unnest(t("child"), t("parent"))
        elif name == "drop_graph":
            o = gl.DropGraph(t("graph"), recursive=op.get("recursive", False))
        elif name == "declare_hedge_type":
            o = gl.DeclareHedgeType(
                t("type"),
                roles=[gl.Role(ctx.term(r["role"]), r.get("many", False), r.get("required", True)) for r in op["roles"]],
                attrs=[ctx.term(a) for a in op.get("attrs", [])],
            )
        elif name == "create_hedge":
            o = gl.CreateHedge(
                t("type"),
                roles=[(ctx.term(a), ctx.term(b)) for a, b in op.get("roles", [])],
                attrs=[(ctx.term(a), ctx.term(b)) for a, b in op.get("attrs", [])],
                graph=opt("graph"),
            )
        elif name == "update_role":
            o = gl.UpdateRole(ref(), t("role"), t("filler"))
        else:
            raise KeyError(name)
        want_err = op.get("error")
        if want_err:
            cls = getattr(gl, want_err + "Error")
            with pytest.raises(cls):
                ctx.db.submit([o])
            continue
        (res,) = ctx.db.submit([o]).results
        if "as" in op:
            assert isinstance(res, gl.Id), (i, op, res)
            ctx.names[op["as"]] = res
        if "result" in op:
            got = len(res) if isinstance(res, list) else res
            assert got == op["result"], (i, op, res)


@pytest.mark.parametrize("case", CASES, ids=[c["id"] for c in CASES])
# @tg: verifies:: [[openspec:python-binding#Python term mapping]] {level: conformance}
def test_case(case: dict[str, Any], tmp_path: pathlib.Path) -> None:
    ctx = Ctx(tmp_path / "case.glv")
    try:
        kind = case["kind"]
        if kind in ("roundtrip", "reject"):
            value = case["value"]
            try:
                v = native(value["native"]) if "native" in value else ctx.term(value["term"])
            except KeyError:
                v = ctx.term(value["term"])
            if kind == "reject":
                cls = getattr(gl, case["error"] + "Error")
                with pytest.raises(cls):
                    ctx.db.submit([gl.Insert(gl.IRI(EX + "s"), gl.IRI(EX + "p"), v)])
                assert ctx.db.sparql("ASK { ?s ?p ?o }") is False
                return
            ctx.db.submit([gl.Insert(gl.IRI(EX + "s"), gl.IRI(EX + "p"), v)])
            if case.get("reopen"):
                ctx.reopen()
            with ctx.db.read() as r:
                (m,) = r.out(r.lookup(gl.IRI(EX + "s")), r.lookup(gl.IRI(EX + "p")))
                got = r.decode(m.o)
            if "inline" in case:
                assert m.o.is_inline == case["inline"]
            expect = case.get("expect", value)
            if "native" in expect:
                try:
                    want = native(expect["native"])
                except KeyError:
                    want = None
                if want is not None:
                    assert same_native(got, want), (got, want)
                    return
            assert decoded_matches(got, expect.get("term", value["term"]), ctx), got
        elif kind == "query":
            run_ops(ctx, case.get("setup", []))
            q = ctx.db.sparql(case["text"]) if case["lang"] == "sparql" else ctx.db.datalog(case["text"])
            e = case["expect"]
            if "boolean" in e:
                assert q is e["boolean"]
                return
            assert not isinstance(q, bool)
            if "count" in e:
                assert len(list(q)) == e["count"]
                return
            assert q.vars == e["vars"]
            got_rows = list(q)
            assert len(got_rows) == len(e["rows"])
            def key(row: tuple[Any, ...]) -> str:
                return json.dumps([None if x is None else repr(x) for x in row])

            decoded_want = [
                tuple(None if t is None else py_value(t, ctx) for t in row) for row in e["rows"]
            ]
            if e.get("ordered"):
                assert got_rows == decoded_want
            else:
                assert sorted(got_rows, key=key) == sorted(decoded_want, key=key)
        elif kind == "ops":
            run_ops(ctx, case["ops"])
    finally:
        ctx.db.close()


# @tg: verifies:: [[openspec:python-binding#Python API parity]] {level: conformance}
def test_corpus_size() -> None:
    assert len(CASES) >= 200
