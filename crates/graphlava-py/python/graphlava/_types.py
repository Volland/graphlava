"""Value types, write operations and enums of the Python binding.

The native module builds and reads these classes; see `tg.md/16-api.md#Python binding`
for the type mapping.
"""

# @tg: implements:: [[openspec:python-binding#Python term mapping]]
# @tg: implements:: [[openspec:python-binding#Python API parity]] {confidence: partial, note: "op classes; the calls are in src/lib.rs"}

from __future__ import annotations

import datetime as _dt
import decimal as _decimal
import enum
from dataclasses import dataclass, field
from typing import ClassVar, NamedTuple, Union

XSD = "http://www.w3.org/2001/XMLSchema#"
XSD_STRING = XSD + "string"
RDF_LANG_STRING = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"


class Id(int):
    """A graphlava ID: the full unsigned 64-bit value, as an ``int``.

    A plain ``int`` is an ID in subject and predicate position and an
    ``xsd:integer`` literal in object position; wrap it in ``Id`` to refer to
    an existing item (a statement, graph, hyperedge, ...) as an object.
    """

    __slots__ = ()

    def __new__(cls, value: int) -> Id:
        if not 0 <= value < 1 << 64:
            raise ValueError(f"an ID is an unsigned 64-bit integer, got {value}")
        return super().__new__(cls, value)

    def __repr__(self) -> str:
        return f"Id(0x{int(self):016x})"

    @property
    def kind(self) -> str:
        """The kind read from the tag bits (``iri``, ``statement``, ``literal``, ...)."""
        v = int(self)
        if v >> 63:
            return "literal"
        return ("null", "iri", "blank", "literal", "statement", "graph", "hyperedge", "system")[v >> 60]

    @property
    def is_inline(self) -> bool:
        """Whether the ID is an inline literal (bit 63 set)."""
        return bool(int(self) >> 63)


@dataclass(frozen=True, slots=True)
class IRI:
    """An IRI term."""

    value: str

    def __str__(self) -> str:
        return self.value


@dataclass(frozen=True, slots=True)
class BNode:
    """A blank node. Labels written by a caller are scoped to one request."""

    label: str

    def __str__(self) -> str:
        return "_:" + self.label


@dataclass(frozen=True, slots=True)
class Literal:
    """Any literal, for datatypes without a native Python mapping.

    ``lang`` makes it an ``rdf:langString``.
    """

    lexical: str
    datatype: str = XSD_STRING
    lang: str | None = None

    def __post_init__(self) -> None:
        if self.lang is not None and self.datatype != RDF_LANG_STRING:
            object.__setattr__(self, "datatype", RDF_LANG_STRING)


@dataclass(frozen=True, slots=True)
class Triple:
    """An RDF 1.2 triple term ``<<( s p o )>>``."""

    s: Term
    p: Term
    o: Term


Value = Union[str, bool, int, float, _decimal.Decimal, _dt.datetime, _dt.date]
"""Python values with a native XSD mapping."""

Term = Union[IRI, BNode, Literal, Triple, Id, Value]
"""Anything a write accepts in a term position."""


class Namespace:
    """``EX = Namespace("http://ex.org/")``; ``EX.alice`` and ``EX["a b"]`` are IRIs."""

    __slots__ = ("_prefix",)

    def __init__(self, prefix: str) -> None:
        self._prefix = prefix

    def __getattr__(self, name: str) -> IRI:
        if name.startswith("__"):
            raise AttributeError(name)
        return IRI(self._prefix + name)

    def __getitem__(self, name: str) -> IRI:
        return IRI(self._prefix + name)

    def __repr__(self) -> str:
        return f"Namespace({self._prefix!r})"


class Durability(str, enum.Enum):
    """Durability level of one write ([[06-transactions-durability#Durability levels]])."""

    SYNC = "sync"
    ORDERED = "ordered"
    RELAXED = "relaxed"


class DeleteMode(str, enum.Enum):
    """How a delete treats annotations and object references."""

    CASCADE = "cascade"
    KEEP_ANNOTATIONS = "keep_annotations"
    DETACH = "detach"


class Match(NamedTuple):
    """One statement match of a core read: subject, predicate, object and
    statement ID (``None`` for an unpromoted inline property)."""

    s: Id
    p: Id
    o: Id
    id: Id | None


@dataclass(frozen=True, slots=True)
class Hyperedge:
    """A stored hyperedge."""

    id: Id
    type: Id
    graph: Id
    roles: list[tuple[Id, Id]]
    promoted: bool


@dataclass(frozen=True, slots=True)
class SweepReport:
    """Counters of one dictionary sweep."""

    marked: int
    examined: int
    deleted: int
    kept_young: int
    kept_rereferenced: int


@dataclass(frozen=True, slots=True)
class UpdateReport:
    """What a SPARQL update changed."""

    inserted: int
    deleted: int
    updated_in_place: int


OpResult = Union[Id, None, bool, list[Id]]
"""One operation result: a statement/graph/hyperedge ID (``None`` for an
unpromoted inline property or an operation with no result), the deleted IDs,
or whether the operation changed anything."""


@dataclass(frozen=True, slots=True)
class CommitResult:
    """Returned by ``submit`` and ``WriteTxn.commit`` once the durability level holds."""

    results: list[OpResult]
    batch: int


# ---- write operations (one per CoreOp) -------------------------------------------


@dataclass(frozen=True, slots=True)
class Insert:
    s: Term
    p: Term
    o: Term
    graph: Term | None = None
    _op: ClassVar[str] = "insert"


@dataclass(frozen=True, slots=True)
class Set:
    s: Term
    p: Term
    o: Term
    _op: ClassVar[str] = "set"


@dataclass(frozen=True, slots=True)
class Update:
    id: int
    o: Term
    expected: Term | None = None
    _op: ClassVar[str] = "update"


@dataclass(frozen=True, slots=True)
class Annotate:
    id: int
    p: Term
    o: Term
    _op: ClassVar[str] = "annotate"


@dataclass(frozen=True, slots=True)
class Delete:
    id: int
    mode: DeleteMode | str = DeleteMode.CASCADE
    _op: ClassVar[str] = "delete"


@dataclass(frozen=True, slots=True)
class Remove:
    s: Term
    p: Term
    o: Term
    mode: DeleteMode | str = DeleteMode.CASCADE
    graph: Term | None = None
    _op: ClassVar[str] = "remove"


@dataclass(frozen=True, slots=True)
class StatementId:
    s: Term
    p: Term
    o: Term
    _op: ClassVar[str] = "statement_id"


@dataclass(frozen=True, slots=True)
class InsertUnasserted:
    s: Term
    p: Term
    o: Term
    _op: ClassVar[str] = "insert_unasserted"


@dataclass(frozen=True, slots=True)
class DeclarePredicate:
    p: Term
    multi: bool = False
    functional: bool = False
    _op: ClassVar[str] = "declare_predicate"


@dataclass(frozen=True, slots=True)
class CreateGraph:
    name: Term
    _op: ClassVar[str] = "create_graph"


@dataclass(frozen=True, slots=True)
class AddMember:
    item: int
    graph: Term
    _op: ClassVar[str] = "add_member"


@dataclass(frozen=True, slots=True)
class RemoveMember:
    item: int
    graph: Term
    _op: ClassVar[str] = "remove_member"


@dataclass(frozen=True, slots=True)
class MoveMember:
    item: int
    from_graph: Term
    to_graph: Term
    _op: ClassVar[str] = "move_member"


@dataclass(frozen=True, slots=True)
class MembershipId:
    item: int
    graph: Term
    _op: ClassVar[str] = "membership_id"


@dataclass(frozen=True, slots=True)
class Nest:
    child: Term
    parent: Term
    _op: ClassVar[str] = "nest"


@dataclass(frozen=True, slots=True)
class Unnest:
    child: Term
    parent: Term
    _op: ClassVar[str] = "unnest"


@dataclass(frozen=True, slots=True)
class DropGraph:
    graph: Term
    recursive: bool = False
    _op: ClassVar[str] = "drop_graph"


@dataclass(frozen=True, slots=True)
class Role:
    """A role of a hyperedge type: cardinality one unless ``many``."""

    role: Term
    many: bool = False
    required: bool = True


@dataclass(frozen=True, slots=True)
class DeclareHedgeType:
    type: Term
    roles: list[Role] = field(default_factory=list)
    attrs: list[Term] = field(default_factory=list)
    _op: ClassVar[str] = "declare_hedge_type"


@dataclass(frozen=True, slots=True)
class CreateHedge:
    type: Term
    roles: list[tuple[Term, Term]] = field(default_factory=list)
    attrs: list[tuple[Term, Term]] = field(default_factory=list)
    graph: Term | None = None
    _op: ClassVar[str] = "create_hedge"


@dataclass(frozen=True, slots=True)
class UpdateRole:
    hedge: int
    role: Term
    filler: Term
    _op: ClassVar[str] = "update_role"


@dataclass(frozen=True, slots=True)
class RoleId:
    hedge: int
    role: Term
    node: Term
    _op: ClassVar[str] = "role_id"


Op = Union[
    Insert, Set, Update, Annotate, Delete, Remove, StatementId, InsertUnasserted,
    DeclarePredicate, CreateGraph, AddMember, RemoveMember, MoveMember, MembershipId,
    Nest, Unnest, DropGraph, DeclareHedgeType, CreateHedge, UpdateRole, RoleId,
]
"""One operation of a one-shot write request."""
