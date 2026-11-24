"""Diagnostics for tests: ``graphlava._debug.decode_count()`` counts dictionary
decodes done for results, and ``panic()`` raises a Rust panic inside a core call."""

from __future__ import annotations

from ._native import _debug_decode_count, _debug_panic


def decode_count() -> int:
    """Terms decoded for query results and ``ReadTxn.decode`` since import."""
    return _debug_decode_count()


def panic() -> None:
    """Panics in Rust with the GIL released; surfaces as ``InternalError``."""
    _debug_panic()
