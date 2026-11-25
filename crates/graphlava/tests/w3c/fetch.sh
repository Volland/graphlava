#!/usr/bin/env sh
# Downloads the W3C rdf-tests suites at the commit pinned in PINNED.md into
# <workspace>/target/w3c-tests/ (idempotent: does nothing when the pinned
# commit is already unpacked). Used by `cargo test --test w3c` and CI.
set -eu

COMMIT=3cbb83bd6d325bcddb1e2496037adb770a9cd769

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../../.." && pwd)
dest="${GRAPHLAVA_W3C_DIR:-$root/target/w3c-tests}"
stamp="$dest/.commit"

if [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$COMMIT" ] && [ -d "$dest/sparql" ]; then
  echo "w3c rdf-tests $COMMIT already present in $dest"
  exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --retry 3 -o "$tmp/rdf-tests.tar.gz" \
  "https://codeload.github.com/w3c/rdf-tests/tar.gz/$COMMIT"
mkdir -p "$tmp/x"
tar -xzf "$tmp/rdf-tests.tar.gz" -C "$tmp/x"
rm -rf "$dest"
mkdir -p "$(dirname "$dest")"
mv "$tmp/x/rdf-tests-$COMMIT" "$dest"
echo "$COMMIT" > "$stamp"
echo "w3c rdf-tests $COMMIT unpacked into $dest"
