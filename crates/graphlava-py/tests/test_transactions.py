"""Requirement "Python transactions as context managers"."""

from __future__ import annotations

import pathlib
import threading
import time

import pytest

import graphlava as gl
from conftest import EX, PFX, reopen
from graphlava import Durability


# @tg: verifies:: [[openspec:python-binding#Python transactions as context managers#Exception aborts the write]] {level: e2e}
def test_exception_aborts_write(path: pathlib.Path) -> None:
    db = gl.Database.open(path)
    with pytest.raises(RuntimeError, match="boom"):
        with db.write(Durability.SYNC) as w:
            for i in range(3):
                w.insert(EX[f"s{i}"], EX.p, EX.o)
            raise RuntimeError("boom")
    db = reopen(db, path)
    assert list(db.sparql(PFX + "SELECT ?s WHERE { ?s :p :o }")) == []
    db.close()


# @tg: verifies:: [[openspec:python-binding#Python transactions as context managers#Writer time limit applies to slow Python code]] {level: e2e}
def test_writer_time_limit(db: gl.Database) -> None:
    with pytest.raises(gl.WriteTimeoutError) as e:
        with db.write(Durability.SYNC) as w:
            time.sleep(0.15)
            w.insert(EX.late, EX.p, EX.o)
    assert "submit" in str(e.value), "the message points at the one-shot path"
    done: list[object] = []

    def other() -> None:
        done.append(db.submit([gl.Insert(EX.next, EX.p, EX.o)], durability=Durability.SYNC))

    t = threading.Thread(target=other)
    t.start()
    t.join(timeout=10)
    assert done and isinstance(done[0], gl.CommitResult)
    assert db.sparql(PFX + "ASK { :next :p :o }") is True
    assert db.sparql(PFX + "ASK { :late :p :o }") is False


# @tg: verifies:: [[openspec:python-binding#Python transactions as context managers#Empty write block]] {level: unit}
def test_empty_write_block(db: gl.Database) -> None:
    before = db.file_size()
    with db.write(Durability.SYNC):
        pass
    assert db.file_size() == before


# @tg: verifies:: [[openspec:python-binding#Python transactions as context managers]] {level: unit}
def test_transaction_closed_after_block(db: gl.Database) -> None:
    with db.write(Durability.SYNC) as w:
        w.insert(EX.a, EX.p, EX.b)
    with pytest.raises(gl.TransactionClosedError):
        w.insert(EX.a, EX.p, EX.c)
    with db.read() as r:
        assert r.lookup(EX.a) is not None
    with pytest.raises(gl.TransactionClosedError):
        r.lookup(EX.a)
    w2 = db.write()
    w2.insert(EX.x, EX.p, EX.y)
    ticket = w2.commit()
    assert isinstance(ticket, gl.CommitResult)
    with pytest.raises(gl.TransactionClosedError):
        w2.commit()
    w3 = db.write()
    w3.insert(EX.never, EX.p, EX.y)
    w3.abort()
    assert db.sparql(PFX + "ASK { :never :p :y }") is False
