#!/usr/bin/env bash
# Builds GraphLavaFFI.xcframework from crates/graphlava-ffi
# ([[16-api#Mobile bindings]], requirement `Mobile native packaging`):
#   ios-arm64                      aarch64-apple-ios
#   ios-arm64_x86_64-simulator     aarch64-apple-ios-sim + x86_64-apple-ios
#   macos-arm64_x86_64             aarch64-apple-darwin  + x86_64-apple-darwin
# Five slices, each a static library with graphlava.h and a module map
# (module GraphLavaFFI), plus the privacy manifest.
#
#   GRAPHLAVA_SLICES=host  builds only the slices of this Mac and the arm64
#                          simulator (fast local loop); default: all five.
#   GRAPHLAVA_PROFILE=debug  builds unoptimised libraries.
# @tg: implements:: [[openspec:mobile-bindings#Mobile native packaging]]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
profile=${GRAPHLAVA_PROFILE:-release}
slices=${GRAPHLAVA_SLICES:-all}
out="$here/GraphLavaFFI.xcframework"
work="$root/target/xcframework"
export IPHONEOS_DEPLOYMENT_TARGET=15.0
export MACOSX_DEPLOYMENT_TARGET=12.0

flag=--release
[ "$profile" = debug ] && flag=

build() {
  rustup target add "$1" >/dev/null 2>&1 || true
  cargo build -p graphlava-ffi $flag --locked --target "$1" --manifest-path "$root/Cargo.toml" >&2
  echo "$root/target/$1/$profile/libgraphlava_ffi.a"
}

rm -rf "$work" "$out"
mkdir -p "$work/include" "$work/ios" "$work/sim" "$work/macos"
cp "$root/crates/graphlava-ffi/include/graphlava.h" "$work/include/"
cat > "$work/include/module.modulemap" <<'MAP'
module GraphLavaFFI {
    header "graphlava.h"
    link "graphlava_ffi"
    export *
}
MAP

args=()
if [ "$slices" = all ]; then
  cp "$(build aarch64-apple-ios)" "$work/ios/libgraphlava_ffi.a"
  lipo -create "$(build aarch64-apple-ios-sim)" "$(build x86_64-apple-ios)" \
    -output "$work/sim/libgraphlava_ffi.a"
  lipo -create "$(build aarch64-apple-darwin)" "$(build x86_64-apple-darwin)" \
    -output "$work/macos/libgraphlava_ffi.a"
  args+=(-library "$work/ios/libgraphlava_ffi.a" -headers "$work/include")
else
  cp "$(build aarch64-apple-ios-sim)" "$work/sim/libgraphlava_ffi.a"
  cp "$(build aarch64-apple-darwin)" "$work/macos/libgraphlava_ffi.a"
fi
args+=(-library "$work/sim/libgraphlava_ffi.a" -headers "$work/include")
args+=(-library "$work/macos/libgraphlava_ffi.a" -headers "$work/include")
xcodebuild -create-xcframework "${args[@]}" -output "$out" >&2
cp "$here/Sources/GraphLava/PrivacyInfo.xcprivacy" "$out/PrivacyInfo.xcprivacy"
# The packages report the core version of the C header they were built from.
grep -E '^version' "$root/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/' > "$out/VERSION"
echo "built $out ($slices, $profile)"
