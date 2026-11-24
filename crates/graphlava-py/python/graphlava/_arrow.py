"""Builds a ``pyarrow.Table`` from decoded result columns (``Rows.to_arrow``)."""

from __future__ import annotations

from typing import Any

from ._types import IRI, BNode, Literal, Triple


def _plain(v: Any) -> Any:
    if isinstance(v, IRI):
        return v.value
    if isinstance(v, BNode):
        return str(v)
    if isinstance(v, Literal):
        return v.lexical
    if isinstance(v, Triple):
        return f"<<( {_plain(v.s)} {_plain(v.p)} {_plain(v.o)} )>>"
    return v


def table(names: list[str], columns: list[list[Any]]) -> Any:
    """One Arrow column per variable. Integers become ``int64`` (or strings when a
    value is outside int64), IRIs and other terms their string form."""
    try:
        import pyarrow as pa  # type: ignore[import-untyped,unused-ignore]
    except ImportError as e:  # pragma: no cover - depends on the environment
        raise ImportError("Rows.to_arrow() needs pyarrow: pip install 'graphlava[arrow]'") from e
    arrays = []
    for col in columns:
        plain = [_plain(v) for v in col]
        if not plain:
            arrays.append(pa.array([], type=pa.null()))
            continue
        try:
            arrays.append(pa.array(plain))
        except (pa.ArrowInvalid, pa.ArrowTypeError, OverflowError):
            arrays.append(pa.array([None if v is None else str(v) for v in plain], type=pa.string()))
    return pa.Table.from_arrays(arrays, names=names)
