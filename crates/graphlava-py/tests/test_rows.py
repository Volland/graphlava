"""Requirement "Python row batches"."""

from __future__ import annotations

import pytest

import graphlava as gl
from conftest import EX, PFX
from graphlava import Durability


def fill(db: gl.Database, n: int) -> None:
    for start in range(0, n, 5000):
        db.submit(
            [gl.Set(EX[f"n{i}"], EX.age, i) for i in range(start, min(n, start + 5000))],
            durability=Durability.RELAXED,
        )


# @tg: verifies:: [[openspec:python-binding#Python row batches#IDs without decoding]] {level: e2e}
def test_ids_without_decoding(db: gl.Database) -> None:
    fill(db, 10_000)
    rows = db.sparql(PFX + "SELECT ?s ?age WHERE { ?s :age ?age }")
    before = gl._debug.decode_count()
    ids = rows.ids()
    assert gl._debug.decode_count() == before
    assert len(ids) == 10_000 and all(len(r) == 2 for r in ids)
    assert all(isinstance(i, int) and 0 < i < 2**64 for r in ids for i in r)
    assert list(rows) == [], "ids() consumed the result"


# @tg: verifies:: [[openspec:python-binding#Python row batches#Arrow export]] {level: e2e}
def test_arrow_export(db: gl.Database) -> None:
    pa = pytest.importorskip("pyarrow")
    fill(db, 2_500)
    q = PFX + "SELECT ?s ?age WHERE { ?s :age ?age }"
    table = db.sparql(q).to_arrow()
    assert isinstance(table, pa.Table)
    assert table.num_rows == 2_500 and table.column_names == ["s", "age"]
    assert table.schema.field("age").type == pa.int64()
    by_iter = sorted((s.value, a) for s, a in db.sparql(q))
    by_arrow = sorted(zip(table.column("s").to_pylist(), table.column("age").to_pylist()))
    assert by_arrow == by_iter


# @tg: verifies:: [[openspec:python-binding#Python row batches#Empty result]] {level: unit}
def test_empty_result(db: gl.Database) -> None:
    pytest.importorskip("pyarrow")
    q = PFX + "SELECT ?s ?age WHERE { ?s :age ?age }"
    assert list(db.sparql(q)) == []
    t = db.sparql(q).to_arrow()
    assert t.num_rows == 0 and t.column_names == ["s", "age"]
    assert list(db.sparql(q).iter_batches()) == []


# @tg: verifies:: [[openspec:python-binding#Python row batches]] {level: e2e}
def test_batches_and_exhaustion(db: gl.Database) -> None:
    fill(db, 2_500)
    rows = db.sparql(PFX + "SELECT ?s ?age WHERE { ?s :age ?age }")
    assert rows.vars == ["s", "age"]
    batches = list(rows.iter_batches())
    assert [len(b) for b in batches] == [1024, 1024, 452]
    assert all(isinstance(r, tuple) and len(r) == 2 for b in batches for r in b)
    assert list(rows) == [] and list(rows.iter_batches()) == [], "exhausted stops cleanly"
    rows2 = db.sparql(PFX + "SELECT ?s WHERE { ?s :age ?age }")
    first = next(iter(rows2))
    assert isinstance(first[0], gl.IRI)
    rows2.close()
    assert list(rows2) == [], "a closed result stops cleanly"
    with db.sparql(PFX + "SELECT ?s WHERE { ?s :age ?age }") as rows3:
        assert len(list(rows3)) == 2_500
    dl = db.datalog("@prefix : <http://ex.org/> .\n?- triple(S, :age, A).")
    sizes = [len(b) for b in dl.iter_batches(size=1000)]
    assert sizes == [1000, 1000, 500]
