#!/usr/bin/env bash
# Fetch the official LUBM UBA 1.7 generator and generate N universities.
#   bench/scripts/fetch-lubm.sh [universities=1] [seed=0]
# Output: bench/data/lubm/u<N>/University*_*.owl and bench/data/lubm/univ-bench.owl
set -euo pipefail
N="${1:-1}"
SEED="${2:-0}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$HERE/data/lubm"
JAVA="${JAVA:-$(command -v java || echo /opt/homebrew/opt/openjdk/bin/java)}"
mkdir -p "$ROOT/dl"
cd "$ROOT"
if [ ! -f dl/uba1.7.zip ]; then
  curl -sSfL --retry 3 -o dl/uba1.7.zip http://swat.cse.lehigh.edu/projects/lubm/uba1.7.zip
fi
if [ ! -f univ-bench.owl ]; then
  curl -sSfL --retry 3 -o univ-bench.owl http://swat.cse.lehigh.edu/onto/univ-bench.owl
fi
rm -rf dl/uba && mkdir -p dl/uba && (cd dl/uba && unzip -q ../uba1.7.zip)
CP="$(find "$ROOT/dl/uba" -type d -name classes | head -1)"
if [ -z "$CP" ]; then
  # Some mirrors ship a jar or sources only: compile the sources.
  SRC="$(find "$ROOT/dl/uba" -type d -name src | head -1)"
  [ -n "$SRC" ] || { echo "UBA archive layout not recognised" >&2; exit 1; }
  CP="$ROOT/dl/uba/classes"; mkdir -p "$CP"
  JAVAC="${JAVAC:-$(dirname "$JAVA")/javac}"
  "$JAVAC" -nowarn -d "$CP" $(find "$SRC" -name '*.java')
fi
OUT="$ROOT/u$N"
rm -rf "$OUT" && mkdir -p "$OUT"
(cd "$OUT" && "$JAVA" -cp "$CP" edu.lehigh.swat.bench.uba.Generator \
   -univ "$N" -index 0 -seed "$SEED" -onto http://swat.cse.lehigh.edu/onto/univ-bench.owl)
# UBA joins paths with a backslash: on Unix the files land next to the output
# directory as "u<N>\\University*.owl". Move them in.
for f in "$ROOT"/u"$N"\\*.owl; do
  [ -e "$f" ] || continue
  mv "$f" "$OUT/${f##*\\}"
done
rm -f "$ROOT"/u"$N"\\* 2>/dev/null || true
echo "LUBM u$N: $(ls "$OUT" | wc -l | tr -d ' ') files in $OUT"
