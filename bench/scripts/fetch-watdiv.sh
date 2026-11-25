#!/usr/bin/env bash
# Build the official WatDiv generator (v0.6) in Docker and generate data and
# the basic-testing query instances (L1-5, S1-7, F1-5, C1-3; 20 each).
#   bench/scripts/fetch-watdiv.sh [scale=1]   # sf1 ~ 10^5 triples, sf100 ~ 10^7
# Output: bench/data/watdiv/sf<S>/watdiv.nt and sf<S>/queries/<T>.sparql
set -euo pipefail
SF="${1:-1}"
COUNT="${WATDIV_INSTANCES:-20}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$HERE/data/watdiv"
mkdir -p "$ROOT/dl" "$ROOT/sf$SF/queries"
cd "$ROOT"
if [ ! -f dl/watdiv_v06.tar ]; then
  curl -sSfL --retry 3 -o dl/watdiv_v06.tar https://dsg.uwaterloo.ca/watdiv/watdiv_v06.tar
fi
command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
# Generator plus query templates; the run happens inside the container so the
# host needs neither boost nor a C++ toolchain.
docker run --rm -v "$ROOT:/w" -w /w ubuntu:22.04 bash -euc "
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq && apt-get install -y -qq build-essential libboost-all-dev >/dev/null
  rm -rf /tmp/wd && mkdir -p /tmp/wd && tar -xf dl/watdiv_v06.tar -C /tmp/wd
  cd /tmp/wd/watdiv
  make -s -j1 >/dev/null 2>&1 || true
  BIN=bin/Release/watdiv; [ -x \$BIN ] || { echo 'watdiv did not build' >&2; exit 1; }
  cd bin/Release
  ./watdiv -d ../../model/wsdbm-data-model.txt $SF > /w/sf$SF/watdiv.nt
  for t in \$(ls ../../testsuite/templates 2>/dev/null || true); do :; done
  TDIR=\$(dirname \$(find ../.. -name 'L1*' -path '*templ*' | head -1))
  for f in \$TDIR/*; do
    n=\$(basename \$f); n=\${n%%.*}
    ./watdiv -q ../../model/wsdbm-data-model.txt \$f $COUNT 1 > /w/sf$SF/queries/\$n.sparql
  done
"
echo "WatDiv sf$SF: $(wc -l < "$ROOT/sf$SF/watdiv.nt") triples, $(ls "$ROOT/sf$SF/queries" | wc -l | tr -d ' ') templates"
