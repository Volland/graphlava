#!/usr/bin/env python3
"""Generates bindings/conformance/core.jsonl, the shared binding corpus.

Edit this file, not core.jsonl. Run: python3 bindings/conformance/gen_core.py
The expected canonical forms follow [[02-id-encoding#Canonical forms]]; the
Rust runner (crates/graphlava/tests/conformance.rs) checks them against the
core, so a wrong expectation fails there first.
"""

from __future__ import annotations

import json
import pathlib
from decimal import Decimal
from typing import Any

XSD = "http://www.w3.org/2001/XMLSchema#"
EX = "http://ex.org/"
B57 = 1 << 57

cases: list[dict[str, Any]] = []


def iri(local: str) -> dict[str, str]:
    return {"iri": EX + local}


def lit(lex: str, dt: str | None = None, lang: str | None = None) -> dict[str, str]:
    t: dict[str, str] = {"literal": lex}
    if lang is not None:
        t["lang"] = lang
    elif dt is not None and dt != "string":
        t["datatype"] = XSD + dt if ":" not in dt else dt
    return t


def add(case: dict[str, Any]) -> None:
    cases.append(case)


def roundtrip(cid: str, group: str, term: dict[str, Any], *, native: Any = None,
              expect: dict[str, Any] | None = None, expect_native: Any = None,
              inline: bool | None = None, reopen: bool = False, hosts: list[str] | None = None,
              description: str | None = None) -> None:
    value: dict[str, Any] = {"term": term}
    if native is not None:
        value["native"] = native
    c: dict[str, Any] = {"id": cid, "group": group, "kind": "roundtrip", "value": value}
    if expect is not None or expect_native is not None:
        e: dict[str, Any] = {"term": expect if expect is not None else term}
        if expect_native is not None:
            e["native"] = expect_native
        elif native is not None:
            e["native"] = native
        c["expect"] = e
    if inline is not None:
        c["inline"] = inline
    if reopen:
        c["reopen"] = True
    if hosts:
        c["hosts"] = hosts
    if description:
        c["description"] = description
    add(c)


# ---- integers: the ±2^57 inline limit, ±2^63, 2**70 -------------------------

INTS = [
    0, 1, -1, 7, -7, 42, 1000, -1000, 2**31 - 1, -(2**31), 2**32, 2**53 - 1, 2**53, -(2**53),
    2**56, -(2**56), B57 - 2, B57 - 1, -B57, -B57 + 1, B57, -B57 - 1, B57 + 1, 2**60,
    2**62, 2**63 - 1, -(2**63), 2**63, -(2**63) - 1, 2**64 - 1, 2**64, 2**70, -(2**70),
    10**30, -(10**30),
]
for i, v in enumerate(INTS):
    inline = -B57 <= v <= B57 - 1
    for reopen in (False, True):
        roundtrip(
            f"int-{'neg' if v < 0 else 'pos'}{abs(v)}{'-reopen' if reopen else ''}",
            "integer",
            lit(str(v), "integer"),
            native={"type": "int", "value": str(v)},
            inline=inline,
            reopen=reopen,
        )

# Integer lexical forms that fold to the canonical form.
for lex, canon in [("+5", "5"), ("007", "7"), ("-0", "0"), ("+0", "0"), ("-007", "-7"),
                   ("000000000000000000000000000000001", "1")]:
    roundtrip(f"int-lex-{lex.replace('+', 'plus').replace('-', 'minus')}", "integer",
              lit(lex, "integer"), expect=lit(canon, "integer"), inline=True)

# Integer-derived types fold to xsd:integer when in range; out of range they stay as written.
for dt, lex in [("int", "5"), ("long", "-9"), ("short", "300"), ("byte", "-128"),
                ("nonNegativeInteger", "0"), ("positiveInteger", "12"),
                ("negativeInteger", "-3"), ("unsignedByte", "255"), ("unsignedLong", "18")]:
    roundtrip(f"int-derived-{dt.lower()}", "integer", lit(lex, dt),
              expect=lit(str(int(lex)), "integer"), inline=True)
for dt, lex in [("byte", "128"), ("unsignedByte", "-1"), ("positiveInteger", "0"),
                ("int", "2147483648")]:
    roundtrip(f"int-derived-out-of-range-{dt.lower()}-{lex.replace('-', 'minus')}", "integer",
              lit(lex, dt), inline=False)
roundtrip("int-ill-typed", "integer", lit("twelve", "integer"), inline=False,
          description="An ill-typed integer is stored as written in the dictionary.")

# ---- booleans ---------------------------------------------------------------

for lex, canon in [("true", True), ("false", False), ("1", True), ("0", False)]:
    roundtrip(f"bool-{lex}", "boolean", lit(lex, "boolean"),
              native={"type": "bool", "value": canon},
              expect=lit("true" if canon else "false", "boolean"),
              expect_native={"type": "bool", "value": canon}, inline=True)


# ---- doubles: exponent range [-255, 254], NaN, infinities, -0.0 ---------------

def dbl_lex(x: float) -> str:
    if x != x:
        return "NaN"
    if x in (float("inf"), float("-inf")):
        return "INF" if x > 0 else "-INF"
    if x == 0:
        return "-0.0E0" if str(x).startswith("-") else "0.0E0"
    sign, digits, exp = Decimal(repr(x)).normalize().as_tuple()
    ds = "".join(map(str, digits))
    e = int(exp) + len(ds) - 1
    mant = ds[0] + "." + (ds[1:] or "0")
    return ("-" if sign else "") + mant + "E" + str(e)


def dbl_inline(x: float) -> bool:
    import struct
    if x != x:
        return False
    bits = struct.unpack("<Q", struct.pack("<d", abs(x)))[0]
    e, m = bits >> 52, bits & ((1 << 52) - 1)
    return (e == 0 and m == 0) or (e == 2047 and m == 0) or 768 <= e <= 1277


DOUBLES = [
    ("zero", 0.0), ("neg-zero", -0.0), ("one", 1.0), ("one-half", 1.5), ("tenth", 0.1),
    ("neg-tenth", -0.1), ("pi", 3.141592653589793), ("hundred", 100.0), ("big", 1e20),
    ("conf", 0.9), ("third", 1 / 3), ("pow2-254", 2.0**254), ("pow2-255", 2.0**255),
    ("pow2-neg255", 2.0**-255), ("pow2-neg256", 2.0**-256), ("neg-pow2-254", -(2.0**254)),
    ("neg-pow2-255", -(2.0**255)), ("e300", 1e300), ("e-300", 1e-300), ("subnormal", 5e-324),
    ("max", 1.7976931348623157e308), ("inf", float("inf")), ("neg-inf", float("-inf")),
    ("e76", 1e76), ("e77", 1e77), ("e-77", 1e-77), ("e-78", 1e-78),
]
for name, x in DOUBLES:
    lex = dbl_lex(x)
    native_val = lex if x != x or x in (float("inf"), float("-inf")) else repr(x)
    if name == "neg-zero":
        native_val = "-0.0"
    for reopen in (False, True):
        roundtrip(f"double-{name}{'-reopen' if reopen else ''}", "double", lit(lex, "double"),
                  native={"type": "float", "value": native_val}, inline=dbl_inline(x),
                  reopen=reopen)
# Lexical forms that fold.
for lex, canon in [("1.50", "1.5E0"), ("1e2", "1.0E2"), ("+0.5", "5.0E-1"), ("-0", "-0.0E0"),
                   ("+INF", "INF")]:
    roundtrip(f"double-lex-{lex.lower().replace('+', 'plus').replace('-', 'minus').replace('.', 'p')}",
              "double", lit(lex, "double"), expect=lit(canon, "double"))
roundtrip("double-nan-term", "double", lit("NaN", "double"), inline=False, hosts=["rust"],
          description="NaN is a valid RDF double; host bindings reject a NaN host value instead (see reject cases).")
add({"id": "reject-float-nan", "group": "double", "kind": "reject",
     "value": {"native": {"type": "float", "value": "NaN"}}, "error": "Invalid",
     "hosts": ["python", "node", "swift", "kotlin"],
     "description": "A NaN host value is rejected (no host can compare it for equality)."})

# ---- decimals: 9 fractional digits inline, scale kept by value -------------

DECIMALS = [
    ("1.50", "1.5", True), ("0.5", "0.5", True), ("-0.5", "-0.5", True), ("1", "1", True),
    ("1.0", "1", True), ("-0.0", "0", True), ("0", "0", True), ("123.456", "123.456", True),
    ("0.000000001", "0.000000001", True), ("-0.000000001", "-0.000000001", True),
    ("0.0000000001", "0.0000000001", False), ("100000000.5", "100000000.5", True),
    # value × 10^9 must fit the signed 58-bit payload: ±2^57 = 144115188.075855872
    ("144115188.075855871", "144115188.075855871", True),
    ("144115188.075855872", "144115188.075855872", False),
    ("-144115188.075855872", "-144115188.075855872", True),
    ("-144115188.075855873", "-144115188.075855873", False),
    ("12345678901234567890.1", "12345678901234567890.1", False),
    ("+7.25", "7.25", True), ("007.250", "7.25", True), (".5", "0.5", True),
    ("3.14159265358979323846", "3.14159265358979323846", False),
]
for i, (lex, canon, inline) in enumerate(DECIMALS):
    for reopen in (False, True):
        roundtrip(f"decimal-{i:02d}{'-reopen' if reopen else ''}", "decimal", lit(lex, "decimal"),
                  native={"type": "decimal", "value": lex},
                  expect=lit(canon, "decimal"),
                  expect_native={"type": "decimal", "value": canon},
                  inline=inline, reopen=reopen,
                  description=f"decimal {lex} (value equality; the canonical form is {canon})")

# ---- datetimes: aware ones normalise to UTC, naive ones are host-rejected ---

DATETIMES = [
    ("2026-11-24T09:00:00Z", "2026-11-24T09:00:00Z"),
    ("2026-11-24T09:00:00+02:00", "2026-11-24T07:00:00Z"),
    ("2026-11-24T09:00:00-05:30", "2026-11-24T14:30:00Z"),
    ("2026-11-24T09:00:00.5Z", "2026-11-24T09:00:00.5Z"),
    ("2026-11-24T09:00:00.123456Z", "2026-11-24T09:00:00.123456Z"),
    ("2026-11-24T09:00:00.000001Z", "2026-11-24T09:00:00.000001Z"),
    ("1970-01-01T00:00:00Z", "1970-01-01T00:00:00Z"),
    ("1969-12-31T23:59:59Z", "1969-12-31T23:59:59Z"),
    ("0001-01-01T00:00:00Z", "0001-01-01T00:00:00Z"),
    ("2000-02-29T12:00:00Z", "2000-02-29T12:00:00Z"),
    ("2026-01-01T00:30:00+01:00", "2025-12-31T23:30:00Z"),
]
for i, (lex, canon) in enumerate(DATETIMES):  # all inside the inline range (±2^56 µs)
    for reopen in (False, True):
        roundtrip(f"datetime-{i:02d}{'-reopen' if reopen else ''}", "datetime",
                  lit(lex, "dateTime"), native={"type": "datetime", "value": lex},
                  expect=lit(canon, "dateTime"),
                  expect_native={"type": "datetime", "value": canon}, inline=True, reopen=reopen)
for lex in ["9999-12-31T23:59:59Z", "4500-01-01T00:00:00Z"]:
    roundtrip(f"datetime-dict-{lex[:4]}", "datetime", lit(lex, "dateTime"),
              native={"type": "datetime", "value": lex}, inline=False, reopen=True,
              description="Outside the ±2^56 µs inline range: a dictionary literal.")
roundtrip("datetime-naive-term", "datetime", lit("2026-11-24T09:00:00", "dateTime"), inline=True,
          hosts=["rust"],
          description="A dateTime without timezone is valid RDF; host bindings reject a naive host value.")
for i, lex in enumerate(["2026-11-24T09:00:00", "1970-01-01T00:00:00", "2026-11-24T09:00:00.5"]):
    add({"id": f"reject-naive-datetime-{i}", "group": "datetime", "kind": "reject",
         "value": {"native": {"type": "naive_datetime", "value": lex}}, "error": "Invalid",
         "hosts": ["python"],
         "description": "A naive datetime has no instant; the binding rejects it."})
for lex, canon in [("2026-11-24", "2026-11-24"), ("2026-11-24Z", "2026-11-24Z"),
                   ("1969-07-20", "1969-07-20")]:
    roundtrip(f"date-{lex.lower()}", "date", lit(lex, "date"), expect=lit(canon, "date"),
              native={"type": "date", "value": lex} if not lex.endswith("Z") else None,
              inline=True)

# ---- strings: 7 bytes inline, trailing NUL, UTF-8 --------------------------

STRINGS = ["", "a", "abcdef", "abcdefg", "abcdefgh", "hello world", "héllo", "héllo!",
           "日本", "日本語", "🦀", "🦀🦀", "abc\u0000", "\u0000", "a\u0000b", "tab\there",
           "line\nbreak", "quote\"d", "back\\slash", "x" * 300, "ünïcödé text that is long"]
for i, s in enumerate(STRINGS):
    n = len(s.encode())
    inline = n <= 7 and not s.endswith("\u0000")
    for reopen in (False, True):
        roundtrip(f"string-{i:02d}{'-reopen' if reopen else ''}", "string", lit(s),
                  native={"type": "str", "value": s}, inline=inline, reopen=reopen,
                  description=f"{n}-byte string")

# ---- language strings ------------------------------------------------------

for i, (text, tag, canon_tag) in enumerate([("hi", "en", "en"), ("hello", "en", "en"),
                                            ("hello!", "en", "en"), ("x", "EN", "en"),
                                            ("grüß", "de-AT", "de-at"),
                                            ("a long sentence", "en-GB", "en-gb"),
                                            ("", "fr", "fr")]):
    roundtrip(f"langstring-{i:02d}", "langstring", lit(text, lang=tag),
              expect=lit(text, lang=canon_tag), reopen=i % 2 == 1)

# ---- IRIs, typed literals of unknown datatypes ---------------------------------

for i, o in enumerate([iri("bob"), {"iri": "urn:uuid:6e8bc430-9c3a-11d9-9669-0800200c9a66"},
                       {"iri": EX + "a/very/long/path/" + "seg/" * 40},
                       {"iri": "http://ex.org/ünï"},
                       lit("POINT(1 2)", "http://www.opengis.net/ont/geosparql#wktLiteral"),
                       lit("{\"a\":1}", "http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON")]):
    roundtrip(f"term-{i:02d}", "term", o, reopen=True)

# ---- queries ------------------------------------------------------------------

PFX = "PREFIX : <http://ex.org/> "


def q(cid: str, group: str, text: str, expect: dict[str, Any], setup: list[dict[str, Any]],
      lang: str = "sparql", hosts: list[str] | None = None, description: str | None = None) -> None:
    c: dict[str, Any] = {"id": cid, "group": group, "kind": "query", "setup": setup,
                         "lang": lang, "text": (PFX + text) if lang == "sparql" else text,
                         "expect": expect}
    if hosts:
        c["hosts"] = hosts
    if description:
        c["description"] = description
    add(c)


def ins(s: str, p: str, o: dict[str, Any], **kw: Any) -> dict[str, Any]:
    d = {"op": "insert", "s": iri(s), "p": iri(p), "o": o}
    d.update(kw)
    return d


NS = [B57 - 1, -B57, B57, 2**70]
num_setup = [ins(f"n{i}", "n", lit(str(v), "integer")) for i, v in enumerate(NS)]
q("query-int-filter-inline-limit", "query",
  f"SELECT ?s WHERE {{ ?s :n ?n FILTER(?n > {B57 - 1}) }}",
  {"vars": ["s"], "rows": [[iri("n2")], [iri("n3")]]}, num_setup,
  description="FILTER(?n > 2**57 - 1) returns exactly the dictionary-stored integers.")
q("query-int-select-all", "query", "SELECT ?n WHERE { ?s :n ?n } ORDER BY ?n",
  {"vars": ["n"], "rows": [[lit(str(v), "integer")] for v in sorted(NS)], "ordered": True},
  num_setup)
q("query-int-count", "query", "SELECT (COUNT(?s) AS ?c) WHERE { ?s :n ?n }",
  {"vars": ["c"], "rows": [[lit("4", "integer")]]}, num_setup)
q("query-int-below", "query", f"SELECT ?s WHERE {{ ?s :n ?n FILTER(?n < {-B57 + 1}) }}",
  {"vars": ["s"], "rows": [[iri("n1")]]}, num_setup)

q("query-empty-store", "empty", "SELECT ?s ?p ?o WHERE { ?s ?p ?o }",
  {"vars": ["s", "p", "o"], "rows": []}, [])
q("query-empty-no-match", "empty", "SELECT ?x WHERE { :nobody :knows ?x }",
  {"vars": ["x"], "rows": []}, [ins("alice", "knows", iri("bob"))])
q("query-empty-filter", "empty", "SELECT ?s WHERE { ?s :age ?a FILTER(?a > 1000) }",
  {"vars": ["s"], "rows": []}, [ins("alice", "age", lit("30", "integer"))])
q("query-empty-limit0", "empty", "SELECT ?s WHERE { ?s ?p ?o } LIMIT 0",
  {"vars": ["s"], "rows": []}, [ins("alice", "knows", iri("bob"))])
q("query-empty-count", "empty", "SELECT (COUNT(*) AS ?c) WHERE { ?s :zzz ?o }",
  {"vars": ["c"], "rows": [[lit("0", "integer")]]}, [])
q("query-ask-true", "query", "ASK { :alice :knows :bob }", {"boolean": True},
  [ins("alice", "knows", iri("bob"))])
q("query-ask-false", "query", "ASK { :alice :knows :carol }", {"boolean": False},
  [ins("alice", "knows", iri("bob"))])
q("query-ask-empty", "empty", "ASK { ?s ?p ?o }", {"boolean": False}, [])

memory = [ins("alice", "knows", iri("bob"), **{"as": "st"}),
          {"op": "annotate", "id": "st", "p": iri("confidence"), "o": lit("0.9E0", "double")}]
q("query-annotation-confidence", "query",
  "SELECT ?c WHERE { << :alice :knows :bob >> :confidence ?c }",
  {"vars": ["c"], "rows": [[lit("9.0E-1", "double")]]}, memory,
  description="The agent memory session: an annotated statement read back through RDF 1.2 syntax.")
q("query-annotation-reopen", "query",
  "SELECT ?c WHERE { << :alice :knows :bob >> :confidence ?c }",
  {"vars": ["c"], "rows": [[lit("9.0E-1", "double")]]}, memory + [{"op": "reopen"}])
q("query-optional-unbound", "query",
  "SELECT ?s ?a WHERE { ?s :knows ?o OPTIONAL { ?s :age ?a } } ORDER BY ?s",
  {"vars": ["s", "a"], "rows": [[iri("alice"), lit("30", "integer")], [iri("bob"), None]],
   "ordered": True},
  [ins("alice", "knows", iri("bob")), ins("bob", "knows", iri("alice")),
   ins("alice", "age", lit("30", "integer"))])
q("query-lang-filter", "query", "SELECT ?l WHERE { :x :label ?l FILTER(lang(?l) = 'de') }",
  {"vars": ["l"], "rows": [[lit("Hallo", lang="de")]]},
  [ins("x", "label", lit("Hello", lang="en")), ins("x", "label", lit("Hallo", lang="de"))])
q("query-decimal-sum", "query", "SELECT (SUM(?v) AS ?t) WHERE { ?s :v ?v }",
  {"vars": ["t"], "rows": [[lit("3.75", "decimal")]]},
  [ins("a", "v", lit("1.50", "decimal")), ins("b", "v", lit("2.25", "decimal"))])
q("query-datetime-order", "query", "SELECT ?s WHERE { ?s :at ?t } ORDER BY ?t",
  {"vars": ["s"], "rows": [[iri("b")], [iri("a")]], "ordered": True},
  [ins("a", "at", lit("2026-11-24T09:00:00Z", "dateTime")),
   ins("b", "at", lit("2026-11-24T09:00:00+02:00", "dateTime"))])
q("query-string-boundary", "query", "SELECT ?s WHERE { ?s :name ?n FILTER(STRLEN(?n) > 7) }",
  {"vars": ["s"], "rows": [[iri("b")]]},
  [ins("a", "name", lit("abcdefg")), ins("b", "name", lit("abcdefgh"))])
q("query-double-inline-boundary", "query", "SELECT ?s WHERE { ?s :x ?x FILTER(?x > 1.0E70) }",
  {"vars": ["s"], "rows": [[iri("b")]]},
  [ins("a", "x", lit("1.0E20", "double")), ins("b", "x", lit("1.0E300", "double"))])
q("query-graph-scope", "graphs", "SELECT ?o WHERE { GRAPH :g { :a :p ?o } }",
  {"vars": ["o"], "rows": [[iri("b")]]},
  [ins("a", "p", iri("b"), graph=iri("g")), ins("a", "p", iri("c"))])
q("query-graph-transitive", "graphs", "SELECT ?o WHERE { GRAPH :parent { :a :p ?o } }",
  {"vars": ["o"], "rows": [[iri("b")]]},
  [ins("a", "p", iri("b"), graph=iri("child")),
   {"op": "nest", "child": iri("child"), "parent": iri("parent")}])
q("query-many-rows", "batches", "SELECT (COUNT(?s) AS ?c) WHERE { ?s :k ?v }",
  {"vars": ["c"], "rows": [[lit("1025", "integer")]]},
  [ins(f"r{i}", "k", lit(str(i), "integer")) for i in range(1025)],
  description="1025 rows: one more than a full 1024-row batch.")
q("query-batch-exact", "batches", "SELECT ?v WHERE { ?s :k ?v }", {"count": 1024},
  [ins(f"r{i}", "k", lit(str(i), "integer")) for i in range(1024)])
q("query-batch-plus-one", "batches", "SELECT ?v WHERE { ?s :k ?v }", {"count": 1025},
  [ins(f"r{i}", "k", lit(str(i), "integer")) for i in range(1025)])
q("datalog-simple", "datalog", "knows(?x, ?y) :- triple(?x, <http://ex.org/knows>, ?y) .\n@output knows .",
  {"count": 2}, [ins("alice", "knows", iri("bob")), ins("bob", "knows", iri("carol"))],
  lang="datalog")
q("datalog-transitive", "datalog",
  "reach(?x, ?y) :- triple(?x, <http://ex.org/knows>, ?y) .\n"
  "reach(?x, ?z) :- reach(?x, ?y), triple(?y, <http://ex.org/knows>, ?z) .\n@output reach .",
  {"count": 3}, [ins("alice", "knows", iri("bob")), ins("bob", "knows", iri("carol"))],
  lang="datalog")
q("datalog-empty", "empty", "knows(?x, ?y) :- triple(?x, <http://ex.org/knows>, ?y) .\n@output knows .",
  {"count": 0}, [], lang="datalog")

# ---- nesting: 64-deep chain, the 65th level is refused ------------------------

chain = [{"op": "nest", "child": iri(f"g{i + 1}"), "parent": iri(f"g{i}")} for i in range(64)]
add({"id": "nesting-64-deep", "group": "nesting", "kind": "ops", "ops": chain + [
    {"op": "nest", "child": iri("g65"), "parent": iri("g64"), "error": "GraphDepthExceeded"}],
    "description": "A 64-edge nesting chain is accepted; the 65th edge exceeds the depth limit."})
q("nesting-64-deep-scope", "nesting", "SELECT ?o WHERE { GRAPH :g0 { :a :p ?o } }",
  {"vars": ["o"], "rows": [[iri("deep")]]},
  chain + [ins("a", "p", iri("deep"), graph=iri("g64"))],
  description="A statement 64 levels down is in the transitive scope of the root.")
add({"id": "nesting-cycle", "group": "nesting", "kind": "ops", "ops": [
    {"op": "nest", "child": iri("b"), "parent": iri("a")},
    {"op": "nest", "child": iri("c"), "parent": iri("b")},
    {"op": "nest", "child": iri("a"), "parent": iri("c"), "error": "GraphCycle"}]})
add({"id": "nesting-self", "group": "nesting", "kind": "ops", "ops": [
    {"op": "create_graph", "graph": iri("a")},
    {"op": "nest", "child": iri("a"), "parent": iri("a"), "error": "GraphCycle"}]})
add({"id": "nesting-idempotent", "group": "nesting", "kind": "ops", "ops": [
    {"op": "nest", "child": iri("b"), "parent": iri("a"), "result": True},
    {"op": "nest", "child": iri("b"), "parent": iri("a"), "result": False}]})

# ---- errors and guards ---------------------------------------------------------

add({"id": "error-cas-conflict", "group": "errors", "kind": "ops", "ops": [
    ins("x", "v", lit("1", "integer")),
    {"op": "statement_id", "s": iri("x"), "p": iri("v"), "o": lit("1", "integer"), "as": "st"},
    {"op": "update", "id": "st", "o": lit("3", "integer")},
    {"op": "update", "id": "st", "o": lit("2", "integer"), "expected": lit("1", "integer"),
     "error": "Conflict"}]})
add({"id": "error-cas-ok", "group": "errors", "kind": "ops", "ops": [
    ins("x", "v", lit("1", "integer")),
    {"op": "statement_id", "s": iri("x"), "p": iri("v"), "o": lit("1", "integer"), "as": "st"},
    {"op": "update", "id": "st", "o": lit("2", "integer"), "expected": lit("1", "integer")}]})
add({"id": "error-fills-role", "group": "errors", "kind": "ops", "ops": [
    {"op": "declare_hedge_type", "type": iri("Meeting"),
     "roles": [{"role": iri("agent")}, {"role": iri("target")}]},
    ins("alice", "knows", iri("bob"), **{"as": "st"}),
    {"op": "create_hedge", "type": iri("Meeting"),
     "roles": [[iri("agent"), iri("dan")], [iri("target"), {"ref": "st"}]], "as": "e"},
    {"op": "delete", "id": "st", "mode": "cascade", "error": "FillsRole"},
    {"op": "delete", "id": "st", "mode": "detach", "result": 2}],
    "description": "A statement filling a hyperedge role cannot be deleted without detach."})
add({"id": "error-referenced-by-statement", "group": "errors", "kind": "ops", "ops": [
    ins("alice", "knows", iri("bob"), **{"as": "st"}),
    ins("carol", "believes", {"ref": "st"}),
    {"op": "delete", "id": "st", "mode": "cascade", "error": "ReferencedByStatement"}]})
add({"id": "error-not-found-update", "group": "errors", "kind": "ops", "ops": [
    ins("x", "v", iri("y"), **{"as": "st"}),
    {"op": "delete", "id": "st", "mode": "cascade", "result": 1},
    {"op": "update", "id": "st", "o": iri("z"), "error": "NotFound"}]})
add({"id": "error-literal-subject", "group": "errors", "kind": "ops", "ops": [
    {"op": "insert", "s": lit("x"), "p": iri("p"), "o": iri("o"), "error": "Invalid"}]})
add({"id": "error-literal-predicate", "group": "errors", "kind": "ops", "ops": [
    {"op": "insert", "s": iri("s"), "p": lit("p"), "o": iri("o"), "error": "Invalid"}]})
add({"id": "error-hedge-shape", "group": "errors", "kind": "ops", "ops": [
    {"op": "declare_hedge_type", "type": iri("Meeting"),
     "roles": [{"role": iri("agent")}, {"role": iri("target")}]},
    {"op": "create_hedge", "type": iri("Meeting"), "roles": [[iri("agent"), iri("dan")]],
     "error": "HyperedgeShapeMismatch"}]})
add({"id": "ops-duplicate-insert", "group": "statements", "kind": "ops", "ops": [
    ins("a", "knows", iri("b"), **{"as": "s1"}),
    ins("a", "knows", iri("b"), **{"as": "s2"}),
    {"op": "delete", "id": "s1", "mode": "cascade", "result": 1},
    {"op": "delete", "id": "s2", "mode": "cascade", "error": "NotFound"}],
    "description": "Statements are unique: the second insert returns the same ID."})
add({"id": "ops-annotation-cascade", "group": "statements", "kind": "ops", "ops": [
    ins("a", "knows", iri("b"), **{"as": "s"}),
    {"op": "annotate", "id": "s", "p": iri("source"), "o": iri("doc"), "as": "a1"},
    {"op": "annotate", "id": "a1", "p": iri("checkedBy"), "o": iri("carol")},
    {"op": "delete", "id": "s", "mode": "cascade", "result": 3}]})
add({"id": "ops-remove", "group": "statements", "kind": "ops", "ops": [
    ins("a", "knows", iri("b")),
    {"op": "remove", "s": iri("a"), "p": iri("knows"), "o": iri("b"), "result": True},
    {"op": "remove", "s": iri("a"), "p": iri("knows"), "o": iri("b"), "result": False}]})
add({"id": "ops-set-overwrites", "group": "statements", "kind": "ops", "ops": [
    {"op": "set", "s": iri("a"), "p": iri("age"), "o": lit("30", "integer")},
    {"op": "set", "s": iri("a"), "p": iri("age"), "o": lit("31", "integer")}]})
q("ops-set-overwrites-query", "statements", "SELECT ?a WHERE { :a :age ?a }",
  {"vars": ["a"], "rows": [[lit("31", "integer")]]},
  [{"op": "set", "s": iri("a"), "p": iri("age"), "o": lit("30", "integer")},
   {"op": "set", "s": iri("a"), "p": iri("age"), "o": lit("31", "integer")}])
q("ops-multi-predicate", "statements", "SELECT ?t WHERE { :a :tag ?t }", {"count": 2},
  [{"op": "declare_predicate", "p": iri("tag"), "multi": True},
   ins("a", "tag", lit("x")), ins("a", "tag", lit("y"))])

out = pathlib.Path(__file__).with_name("core.jsonl")
ids = [c["id"] for c in cases]
assert len(ids) == len(set(ids)), "duplicate case id"
with out.open("w", encoding="utf-8") as f:
    for c in cases:
        f.write(json.dumps(c, ensure_ascii=False, sort_keys=False) + "\n")
print(f"wrote {len(cases)} cases to {out}")
