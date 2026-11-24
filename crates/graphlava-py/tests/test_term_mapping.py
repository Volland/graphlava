"""Requirement "Python term mapping"."""

from __future__ import annotations

import datetime as dt
import math
import pathlib
from decimal import Decimal

import pytest
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

import graphlava as gl
from conftest import EX, PFX, reopen

B57 = 1 << 57


# @tg: verifies:: [[openspec:python-binding#Python term mapping#Integers on both sides of the inline limit]] {level: e2e}
def test_integers_on_both_sides_of_inline_limit(path: pathlib.Path) -> None:
    db = gl.Database.open(path)
    values = [B57 - 1, -B57, B57, 2**70]
    db.submit([gl.Insert(EX[f"n{i}"], EX.n, v) for i, v in enumerate(values)])
    db = reopen(db, path)
    with db.read() as r:
        for i, v in enumerate(values):
            (m,) = r.out(r.lookup(EX[f"n{i}"]), r.lookup(EX.n))
            got = r.decode(m.o)
            assert type(got) is int and got == v
            assert m.o.is_inline == (i < 2), "inside ±2^57 inline, outside in the dictionary"
    rows = db.sparql(PFX + f"SELECT ?n WHERE {{ ?s :n ?n FILTER(?n > {B57 - 1}) }}")
    assert sorted(n for (n,) in rows) == [B57, 2**70]
    db.close()


# @tg: verifies:: [[openspec:python-binding#Python term mapping#Naive datetime is rejected]] {level: unit}
def test_naive_datetime_rejected(db: gl.Database) -> None:
    with db.write() as w:
        w.insert(EX.a, EX.p, EX.b)
        with pytest.raises(gl.InvalidError):
            w.insert(EX.a, EX.at, dt.datetime(2026, 11, 24, 9, 0))
        assert len(w.out(w.lookup(EX.a))) == 1, "the transaction is unchanged"
    with pytest.raises(gl.InvalidError):
        db.submit([gl.Insert(EX.a, EX.at, dt.datetime(2026, 11, 24, 9, 0))])
    with pytest.raises(gl.InvalidError):
        db.submit([gl.Insert(EX.a, EX.x, math.nan)])
    assert not db.sparql(PFX + "ASK { :a :at ?t }")


# @tg: verifies:: [[openspec:python-binding#Python term mapping#Statement ID with the top tag bits set]] {level: unit}
def test_statement_id_with_top_tag_bits(db: gl.Database) -> None:
    sid = db.submit([gl.Insert(EX.a, EX.knows, EX.b)]).results[0]
    assert isinstance(sid, int) and 0 <= sid < 2**64
    assert sid >> 60 == 4, "statement kind tag in bits 60-63"
    with db.read() as r:
        assert r.stmt(int(sid)) == (r.lookup(EX.a), r.lookup(EX.knows), r.lookup(EX.b))
    db.submit([gl.Set(EX.a, EX.age, 5)])
    with db.read() as r:
        (m,) = r.props(r.lookup(EX.a))
        assert m.o >= 2**63 and m.o < 2**64 and r.decode(m.o) == 5
        assert r.lookup(5) == m.o
    with pytest.raises((OverflowError, ValueError)):
        gl.Id(2**64)
    with pytest.raises((OverflowError, ValueError)):
        gl.Id(-1)


VALUES = [
    "", "héllo", "abcdefgh", "x" * 300, True, False, 0, -1, B57 - 1, -B57, B57, -B57 - 1,
    2**63, -(2**63), 2**70, 0.0, -0.0, 1.5, 0.1, 1e300, 5e-324, math.inf, -math.inf,
    Decimal("1.50"), Decimal("-0.000000001"), Decimal("12345678901234567890.1"),
    dt.datetime(2026, 11, 24, 9, 0, tzinfo=dt.timezone.utc),
    dt.datetime(2026, 11, 24, 9, 0, 0, 123456, tzinfo=dt.timezone(dt.timedelta(hours=2))),
    dt.date(2026, 11, 24), gl.Literal("POINT(1 2)", "http://www.opengis.net/ont/geosparql#wktLiteral"),
    gl.Literal("hello", lang="en"), EX.bob, gl.BNode("b"),
]


@pytest.mark.parametrize("value", VALUES, ids=repr)
# @tg: verifies:: [[openspec:python-binding#Python term mapping]] {level: e2e}
def test_boundary_round_trip(path: pathlib.Path, value: object) -> None:
    db = gl.Database.open(path)
    db.submit([gl.Insert(EX.s, EX.p, value)])  # type: ignore[arg-type]
    db = reopen(db, path)
    with db.read() as r:
        (m,) = r.out(r.lookup(EX.s), r.lookup(EX.p))
        got = r.decode(m.o)
    db.close()
    if isinstance(value, gl.BNode):
        assert isinstance(got, gl.BNode)
        return
    assert got == value and type(got) is type(value)
    if isinstance(value, float):
        assert math.copysign(1, got) == math.copysign(1, value)  # type: ignore[arg-type]


@settings(max_examples=200, deadline=None, suppress_health_check=[HealthCheck.function_scoped_fixture])
@given(
    v=st.one_of(
        st.integers(min_value=-(2**80), max_value=2**80),
        st.floats(allow_nan=False),
        st.text(max_size=20),
        st.booleans(),
        st.decimals(allow_nan=False, allow_infinity=False, places=6, min_value=-(10**12), max_value=10**12),
        st.datetimes(
            min_value=dt.datetime(1, 1, 2), max_value=dt.datetime(9999, 12, 30),
            timezones=st.just(dt.timezone.utc),
        ),
    )
)
# @tg: verifies:: [[openspec:python-binding#Python term mapping]] {level: property}
def test_hypothesis_round_trip(db: gl.Database, v: object) -> None:
    db.submit([gl.Set(EX.s, EX.p, v)])  # type: ignore[arg-type]
    with db.read() as r:
        (m,) = r.out(r.lookup(EX.s), r.lookup(EX.p))
        got = r.decode(m.o)
    assert got == v and type(got) is type(v)
