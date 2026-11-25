"""Requirement "Python errors"."""

from __future__ import annotations

import pathlib

import pytest

import graphlava as gl
from conftest import EX, PFX, reopen
from graphlava import Durability


# @tg: verifies:: [[openspec:python-binding#Python errors#Hyperedge guard error carries IDs]] {level: e2e}
def test_fills_role_error_carries_ids(path: pathlib.Path) -> None:
    db = gl.Database.open(path)
    res = db.submit(
        [
            gl.DeclareHedgeType(EX.Meeting, roles=[gl.Role(EX.agent), gl.Role(EX.target)]),
            gl.Insert(EX.alice, EX.knows, EX.bob),
        ]
    ).results
    node = res[1]
    assert isinstance(node, gl.Id)
    e = db.submit([gl.CreateHedge(EX.Meeting, roles=[(EX.agent, EX.dan), (EX.target, node)])]).results[0]
    with pytest.raises(gl.FillsRoleError) as err:
        db.submit([gl.Delete(node, gl.DeleteMode.CASCADE)])
    assert err.value.item == node and err.value.hedge == e
    assert isinstance(err.value, gl.Error)
    db = reopen(db, path)
    with db.read() as r:
        assert r.stmt(node) is not None, "the node still exists"
    db.close()


# @tg: verifies:: [[openspec:python-binding#Python errors#Compare-and-set conflict]] {level: e2e}
def test_compare_and_set_conflict(db: gl.Database) -> None:
    sid = db.submit([gl.Insert(EX.x, EX.v, 1), gl.StatementId(EX.x, EX.v, 1)]).results[1]
    assert isinstance(sid, gl.Id)
    db.submit([gl.Update(sid, o=3)])  # another client
    with pytest.raises(gl.ConflictError):
        db.submit([gl.Update(sid, o=2, expected=1)])
    with db.read() as r:
        assert r.decode(r.stmt(sid)[2]) == 3  # type: ignore[index]


# @tg: verifies:: [[openspec:python-binding#Python errors]] {level: e2e}
def test_every_variant_has_a_class(db: gl.Database) -> None:
    names = [
        "Io", "Corrupted", "Full", "FormatMismatch", "Locked", "Conflict", "NotFound",
        "ReferencedByStatement", "FillsRole", "GraphCycle", "GraphDepthExceeded",
        "HyperedgeShapeMismatch", "CascadeLimitExceeded", "WriteTimeout", "ReadOnly", "Invalid",
    ]
    for n in names:
        cls = getattr(gl, n + "Error")
        assert issubclass(cls, gl.Error)
    with pytest.raises(gl.GraphCycleError) as e:
        db.submit([gl.Nest(EX.b, EX.a), gl.Nest(EX.a, EX.b)])
    assert isinstance(e.value.child, int) and isinstance(e.value.parent, int)
    sid = db.submit([gl.Insert(EX.x, EX.p, EX.y)]).results[0]
    assert isinstance(sid, gl.Id)
    db.submit([gl.Delete(sid)])
    with pytest.raises(gl.NotFoundError) as nf:
        db.submit([gl.Update(sid, o=EX.z)])
    assert nf.value.id == sid
    with pytest.raises(gl.InvalidError):
        db.sparql("SELECT nonsense")
    with pytest.raises(gl.InvalidError):
        db.submit([gl.Insert("not an IRI", EX.p, EX.o)])


# @tg: verifies:: [[openspec:python-binding#Python errors]] {level: unit}
def test_panic_becomes_internal_error(db: gl.Database) -> None:
    with pytest.raises(gl.InternalError):
        gl._debug.panic()
    # the interpreter and the handle keep working
    db.submit([gl.Insert(EX.after, EX.p, EX.panic)], durability=Durability.SYNC)
    assert db.sparql(PFX + "ASK { :after :p :panic }") is True
