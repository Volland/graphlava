#!/usr/bin/env bash
# The 16 KB alignment gate: fails, naming the library, when any LOAD segment
# of a shared library is aligned below 0x4000 (requirement `Mobile native
# packaging`). Usage: check-16k-alignment.sh lib.so... (READELF overrides
# the readelf binary; llvm-readelf from the NDK works on every host).
# @tg: implements:: [[openspec:mobile-bindings#Mobile native packaging]]
set -euo pipefail
readelf="${READELF:-llvm-readelf}"
bad=0
for so in "$@"; do
  aligns=$("$readelf" -lW "$so" | awk '$1=="LOAD" {print $NF}')
  [ -n "$aligns" ] || { echo "error: no LOAD segments in $so" >&2; bad=1; continue; }
  for a in $aligns; do
    if [ $((a)) -lt $((0x4000)) ]; then
      echo "error: $so has a LOAD segment aligned to $a, not 0x4000 (16 KB pages)" >&2
      bad=1
    fi
  done
done
exit $bad
