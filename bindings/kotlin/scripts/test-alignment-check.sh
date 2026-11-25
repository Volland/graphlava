#!/usr/bin/env bash
# Unit test of the alignment gate: a library linked without the 16 KB flag is
# rejected by name; one linked with it passes.
# @tg: verifies:: [[openspec:mobile-bindings#Mobile native packaging#4 KB-aligned library is rejected by CI]] {level: unit}
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
ndk="${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* | sort -V | tail -1)}"
bin=$(ls -d "$ndk"/toolchains/llvm/prebuilt/*/bin | head -1)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo 'int f(void) { return 1; }' > "$tmp/f.c"
"$bin/aarch64-linux-android24-clang" -shared -fPIC "$tmp/f.c" -Wl,-z,max-page-size=4096 -o "$tmp/libfour.so"
"$bin/aarch64-linux-android24-clang" -shared -fPIC "$tmp/f.c" -Wl,-z,max-page-size=16384 -o "$tmp/libsixteen.so"
export READELF="$bin/llvm-readelf"
"$here/check-16k-alignment.sh" "$tmp/libsixteen.so"
if "$here/check-16k-alignment.sh" "$tmp/libfour.so" 2> "$tmp/err"; then
  echo "FAIL: a 4 KB-aligned library passed the gate" >&2; exit 1
fi
grep -q "libfour.so" "$tmp/err" || { echo "FAIL: the gate did not name the library" >&2; exit 1; }
echo "alignment gate ok"
