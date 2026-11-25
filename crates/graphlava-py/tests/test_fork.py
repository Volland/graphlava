"""Requirement "Single owner across fork"."""

from __future__ import annotations

import multiprocessing as mp
import os
import pathlib

import pytest

import graphlava as gl
from conftest import EX, PFX, reopen
from graphlava import Durability


@pytest.mark.skipif(not hasattr(os, "fork"), reason="os.fork is POSIX only")
# @tg: verifies:: [[openspec:python-binding#Single owner across fork#Child process uses inherited handle]] {level: e2e}
def test_child_uses_inherited_handle(path: pathlib.Path) -> None:
    db = gl.Database.open(path)
    db.submit([gl.Insert(EX.before, EX.p, EX.o)], durability=Durability.SYNC)
    size = db.file_size()
    r, w = os.pipe()
    pid = os.fork()
    if pid == 0:  # child
        code = 1
        try:
            os.close(r)
            try:
                db.read()
                msg = b"no error"
            except gl.ForkedHandleError as e:
                ok = e.owner_pid == os.getppid() and e.pid == os.getpid()
                msg = b"forked" if ok else b"bad fields"
            except BaseException as e:  # noqa: BLE001
                msg = repr(e).encode()
            for call in (lambda: db.submit([gl.Insert(EX.child, EX.p, EX.o)]), db.file_size, db.close):
                try:
                    call()
                    msg += b" call-succeeded"
                except gl.ForkedHandleError:
                    pass
            os.write(w, msg)
            code = 0
        finally:
            os._exit(code)
    os.close(w)
    got = os.read(r, 1000)
    _, status = os.waitpid(pid, 0)
    assert os.WEXITSTATUS(status) == 0
    assert got == b"forked", got
    assert db.file_size() == size, "the child did not touch the file"
    db.submit([gl.Insert(EX.after, EX.p, EX.o)], durability=Durability.SYNC)
    db = reopen(db, path)
    assert db.sparql(PFX + "ASK { :after :p :o }") is True
    assert db.sparql(PFX + "ASK { :child :p :o }") is False
    db.close()


@pytest.mark.skipif(not hasattr(os, "fork"), reason="os.fork is POSIX only")
# @tg: verifies:: [[openspec:python-binding#Single owner across fork]] {level: e2e}
def test_child_exit_does_not_flush_inherited_handle(path: pathlib.Path) -> None:
    """A child that exits normally (running finalizers) leaves the parent's
    open batch alone; only the parent commits it."""
    db = gl.Database.open(path)
    db.submit([gl.Insert(EX.relaxed, EX.p, EX.o)], durability=Durability.RELAXED)
    pid = os.fork()
    if pid == 0:
        code = 1
        try:
            del db  # the last reference: runs the handle's finalizer in the child
            import gc

            gc.collect()
            code = 0
        finally:
            os._exit(code)
    _, status = os.waitpid(pid, 0)
    assert os.WEXITSTATUS(status) == 0
    db.submit([gl.Insert(EX.sync, EX.p, EX.o)], durability=Durability.SYNC)
    db = reopen(db, path)
    assert db.sparql(PFX + "ASK { :relaxed :p :o . :sync :p :o }") is True
    db.close()


def _spawned_open(path: str, q: "mp.Queue[str]") -> None:
    import graphlava as g

    try:
        g.Database.open(path)
        q.put("opened")
    except g.LockedError:
        q.put("locked")
    except BaseException as e:  # noqa: BLE001
        q.put(repr(e))


# @tg: verifies:: [[openspec:python-binding#Single owner across fork#Second interpreter is refused]] {level: e2e}
def test_second_interpreter_is_refused(path: pathlib.Path) -> None:
    db = gl.Database.open(path)
    ctx = mp.get_context("spawn")
    q: mp.Queue[str] = ctx.Queue()
    p = ctx.Process(target=_spawned_open, args=(str(path), q))
    p.start()
    got = q.get(timeout=60)
    p.join(timeout=60)
    assert got == "locked"
    db.submit([gl.Insert(EX.parent, EX.p, EX.o)], durability=Durability.SYNC)
    assert db.sparql(PFX + "ASK { :parent :p :o }") is True
    # and in the same process: a second handle is refused too
    with pytest.raises(gl.LockedError):
        gl.Database.open(path)
    db.close()
