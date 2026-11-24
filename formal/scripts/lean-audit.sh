#!/usr/bin/env bash
# Fails if any Lean proof in formal/ is incomplete or rests on an unexpected axiom.
set -euo pipefail
cd "$(dirname "$0")/.."
if grep -rnwE 'sorry|admit' MetaCore Main.lean; then
  echo "error: sorry/admit in formal model" >&2; exit 1
fi
if grep -rnE '^\s*axiom\s' MetaCore Main.lean; then
  echo "error: custom axiom in formal model" >&2; exit 1
fi
out=$(lake env lean MetaCore/Audit.lean 2>&1)
echo "$out"
if echo "$out" | grep -qE 'sorryAx|error'; then
  echo "error: audit found sorryAx or an error" >&2; exit 1
fi
bad=$(echo "$out" | grep -oE "depends on axioms: \[[^]]*\]" | tr -d '[],' | tr ' ' '\n' \
  | grep -vxE 'depends|on|axioms:|propext|Classical\.choice|Quot\.sound' | grep -v '^$' || true)
if [ -n "$bad" ]; then echo "error: unexpected axioms: $bad" >&2; exit 1; fi
echo "lean audit: ok"
