"""graphlava: a mutable, single-file metagraph database for agent memory.

The Python binding of the Rust API (ADR 0042, ``tg.md/16-api.md#Python binding``).
Open a file with :meth:`Database.open`; write with :meth:`Database.submit` (one-shot,
the preferred path) or ``with db.write(...)`` (interactive, at most 100 ms); read with
``with db.read()`` and query with :meth:`Database.sparql` / :meth:`Database.datalog`.
"""

from __future__ import annotations

from . import _debug
from ._errors import (
    CascadeLimitExceededError,
    ConflictError,
    CorruptedError,
    Error,
    FillsRoleError,
    ForkedHandleError,
    FormatMismatchError,
    FullError,
    GraphCycleError,
    GraphDepthExceededError,
    HyperedgeShapeMismatchError,
    InternalError,
    InvalidError,
    IoError,
    LockedError,
    NotFoundError,
    ReadOnlyError,
    ReferencedByStatementError,
    TransactionClosedError,
    WriteTimeoutError,
)
from ._native import Admin, Database, ReadTxn, Rows, WriteTxn, core_version
from ._types import (
    IRI,
    RDF_LANG_STRING,
    XSD,
    AddMember,
    Annotate,
    BNode,
    CommitResult,
    CreateGraph,
    CreateHedge,
    DeclareHedgeType,
    DeclarePredicate,
    Delete,
    DeleteMode,
    DropGraph,
    Durability,
    Hyperedge,
    Id,
    Insert,
    InsertUnasserted,
    Literal,
    Match,
    MembershipId,
    MoveMember,
    Namespace,
    Nest,
    Op,
    OpResult,
    Remove,
    RemoveMember,
    Role,
    RoleId,
    Set,
    StatementId,
    SweepReport,
    Term,
    Triple,
    Unnest,
    Update,
    UpdateReport,
    UpdateRole,
    Value,
)

__version__: str = core_version()

__all__ = [
    "__version__", "core_version", "Database", "ReadTxn", "WriteTxn", "Rows", "Admin",
    "IRI", "BNode", "Literal", "Triple", "Id", "Namespace", "Term", "Value", "XSD",
    "RDF_LANG_STRING", "Durability", "DeleteMode", "Match", "Hyperedge", "SweepReport",
    "UpdateReport", "CommitResult", "OpResult", "Op", "Insert", "Set", "Update", "Annotate",
    "Delete", "Remove", "StatementId", "InsertUnasserted", "DeclarePredicate", "CreateGraph",
    "AddMember", "RemoveMember", "MoveMember", "MembershipId", "Nest", "Unnest", "DropGraph",
    "Role", "DeclareHedgeType", "CreateHedge", "UpdateRole", "RoleId",
    "Error", "IoError", "CorruptedError", "FullError", "FormatMismatchError", "LockedError",
    "ConflictError", "NotFoundError", "ReferencedByStatementError", "FillsRoleError",
    "GraphCycleError", "GraphDepthExceededError", "HyperedgeShapeMismatchError",
    "CascadeLimitExceededError", "WriteTimeoutError", "ReadOnlyError", "InvalidError",
    "InternalError", "TransactionClosedError", "ForkedHandleError",
]
