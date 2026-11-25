#!/usr/bin/env bash
# Fetch the official BSBM tools (Java) and generate a dataset with an update stream.
#   bench/scripts/fetch-bsbm.sh [products=1000]   # 28480 products ~ 10M triples
# Output: bench/data/bsbm/pc<N>/dataset.nt and dataset_update.nt
set -euo pipefail
PC="${1:-1000}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$HERE/data/bsbm"
JAVA="${JAVA:-$(command -v java || echo /opt/homebrew/opt/openjdk/bin/java)}"
mkdir -p "$ROOT/dl"
cd "$ROOT"
if [ ! -f dl/bsbmtools.zip ]; then
  curl -sSfL --retry 3 -o dl/bsbmtools.zip "https://sourceforge.net/projects/bsbmtools/files/bsbmtools/bsbmtools-0.2/bsbmtools-v0.2.zip/download"
fi
rm -rf dl/tools && mkdir -p dl/tools && (cd dl/tools && unzip -q ../bsbmtools.zip)
TOOLS="$(dirname "$(find "$ROOT/dl/tools" -name generate -o -name 'bsbm.jar' | head -1)")"
# The update stream (-ud) needs enough products (the generator refuses small
# counts); it is skipped below 2000 products and the update mix then reports it.
UD=""; [ "$PC" -ge 2000 ] && UD="-ud"
OUT="$ROOT/pc$PC"
rm -rf "$OUT" && mkdir -p "$OUT"
(cd "$TOOLS" && "$JAVA" -cp "$TOOLS/lib/*:$TOOLS/bin:$TOOLS/bsbm.jar" benchmark.generator.Generator \
   -pc "$PC" -s nt -fn "$OUT/dataset" $UD -dir "$OUT/td_data")
[ -f "$TOOLS/dataset_update.nt" ] && mv "$TOOLS/dataset_update.nt" "$OUT/" || true
echo "BSBM pc$PC: $(wc -l < "$OUT/dataset.nt") triples"
