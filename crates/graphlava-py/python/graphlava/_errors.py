"""The exception hierarchy: one subclass of ``Error`` per Rust ``Error`` variant,
with the variant's fields as attributes, plus the binding's own errors."""

# @tg: implements:: [[openspec:python-binding#Python errors]]

from __future__ import annotations

from typing import Any


class Error(Exception):
    """Base class of every graphlava error."""

    def __init__(self, message: str = "", **fields: Any) -> None:
        super().__init__(message)
        for k, v in fields.items():
            setattr(self, k, v)


class IoError(Error):
    """OS I/O failure."""


class CorruptedError(Error):
    """Checksum or structure failure in the file."""


class FullError(Error):
    """Disk or address space exhausted."""


class FormatMismatchError(Error):
    """Unknown format version or feature flag; the file was not modified."""


class LockedError(Error):
    """The file is held by another owner process or handle."""


class ConflictError(Error):
    """A duplicate (s, p, o) or a compare-and-set that found another value."""


class NotFoundError(Error):
    """The ID does not name a live item."""

    id: int


class ReferencedByStatementError(Error):
    """Delete blocked: statement ``by`` has ``item`` in object position."""

    item: int
    by: int


class FillsRoleError(Error):
    """Delete blocked: ``item`` fills a role of hyperedge ``hedge``."""

    item: int
    hedge: int


class GraphCycleError(Error):
    """Nesting ``child`` under ``parent`` would close a cycle."""

    child: int
    parent: int


class GraphDepthExceededError(Error):
    """Graph nesting would exceed the depth ``limit``."""

    limit: int


class HyperedgeShapeMismatchError(Error):
    """A hyperedge does not match its type's roles."""


class CascadeLimitExceededError(Error):
    """A cascade would delete more than ``limit`` items."""

    limit: int


class WriteTimeoutError(Error):
    """An interactive write transaction exceeded its time limit and was aborted."""


class ReadOnlyError(Error):
    """The database is read-only after a failed batch commit; reopen it."""


class InvalidError(Error, ValueError):
    """The request is malformed: a bad term, literal, query or argument."""


class InternalError(Error):
    """A Rust panic or an error this binding does not know. The interpreter keeps running."""


class TransactionClosedError(Error):
    """A transaction or result used after its block ended, or a closed database."""


class ForkedHandleError(Error):
    """A handle used in a process other than the one that opened it (after ``os.fork``)."""

    owner_pid: int
    pid: int


__all__ = [
    "Error", "IoError", "CorruptedError", "FullError", "FormatMismatchError", "LockedError",
    "ConflictError", "NotFoundError", "ReferencedByStatementError", "FillsRoleError",
    "GraphCycleError", "GraphDepthExceededError", "HyperedgeShapeMismatchError",
    "CascadeLimitExceededError", "WriteTimeoutError", "ReadOnlyError", "InvalidError",
    "InternalError", "TransactionClosedError", "ForkedHandleError",
]
