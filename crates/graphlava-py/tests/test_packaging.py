"""Requirement "Python packaging" (the parts checkable on one machine; the
per-target wheel matrix runs in CI, see .github/workflows/python-wheels.yml)."""

from __future__ import annotations

import pathlib
import subprocess
import sys

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10
    import tomli as tomllib  # type: ignore[no-redef]

import graphlava as gl

CRATE = pathlib.Path(__file__).resolve().parents[1]


# @tg: verifies:: [[openspec:python-binding#Python packaging#Wheel install and smoke test per target]] {level: e2e}
def test_version_and_smoke(tmp_path: pathlib.Path) -> None:
    assert gl.__version__ == gl.core_version()
    cargo = tomllib.loads((CRATE.parents[1] / "Cargo.toml").read_text())
    assert gl.core_version() == cargo["workspace"]["package"]["version"]
    out = subprocess.run(
        [sys.executable, str(CRATE / "tests" / "smoke.py"), str(tmp_path / "smoke.glv")],
        capture_output=True, text=True,
    )
    assert out.returncode == 0, out.stderr
    pkg = pathlib.Path(gl.__file__).parent
    assert (pkg / "py.typed").exists()
    assert (pkg / "_native.pyi").exists()


# @tg: verifies:: [[openspec:python-binding#Python packaging#Unsupported interpreter]] {level: unit}
def test_requires_python_310() -> None:
    meta = tomllib.loads((CRATE / "pyproject.toml").read_text())
    assert meta["project"]["requires-python"] == ">=3.10"
    from packaging.specifiers import SpecifierSet

    spec = SpecifierSet(meta["project"]["requires-python"])
    assert "3.9.18" not in spec and "3.10.0" in spec and "3.13.0" in spec
    assert meta["tool"]["maturin"]["module-name"] == "graphlava._native"
    cargo = tomllib.loads((CRATE / "Cargo.toml").read_text())
    assert cargo["features"]["abi3"] == ["pyo3/abi3-py310"], "abi3 floor matches requires-python"
