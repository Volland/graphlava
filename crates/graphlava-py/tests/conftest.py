"""Shared fixtures: every test opens a real file in a fresh temp dir."""

from __future__ import annotations

import os
import pathlib
from collections.abc import Iterator

import pytest

import graphlava as gl

EX = gl.Namespace("http://ex.org/")
PFX = "PREFIX : <http://ex.org/> "


@pytest.fixture
def path(tmp_path: pathlib.Path) -> pathlib.Path:
    return tmp_path / "memory.glv"


@pytest.fixture
def db(path: pathlib.Path) -> Iterator[gl.Database]:
    d = gl.Database.open(path)
    yield d
    d.close()


def reopen(db: gl.Database, path: pathlib.Path) -> gl.Database:
    db.close()
    return gl.Database.open(path)


def slow_enabled() -> bool:
    return os.environ.get("GRAPHLAVA_SLOW") == "1"
