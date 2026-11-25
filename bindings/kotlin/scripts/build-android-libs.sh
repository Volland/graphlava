#!/usr/bin/env bash
# Builds libgraphlava.so (C FFI + JNI shim) for arm64-v8a and x86_64 with the
# Android NDK, linked for 16 KB pages (-z max-page-size=16384, Google Play
# from 2025, [[07-platforms#Android]]), into graphlava-android/src/main/jniLibs,
# then runs the alignment gate on them.
# @tg: implements:: [[openspec:mobile-bindings#Mobile native packaging]]
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$here/../.." && pwd)
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
ndk="${ANDROID_NDK_HOME:-$(ls -d "$sdk"/ndk/* | sort -V | tail -1)}"
bin=$(ls -d "$ndk"/toolchains/llvm/prebuilt/*/bin | head -1)
api=24
out="$here/graphlava-android/src/main/jniLibs"
for pair in aarch64-linux-android:arm64-v8a x86_64-linux-android:x86_64; do
  target=${pair%%:*}; abi=${pair##*:}
  rustup target add "$target" >/dev/null 2>&1 || true
  linker="$bin/${target}${api}-clang"
  var=$(echo "CARGO_TARGET_${target}_LINKER" | tr 'a-z-' 'A-Z_')
  env "$var=$linker" AR="$bin/llvm-ar" "CC_${target//-/_}=$linker" \
    RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
    cargo build -p graphlava-ffi --release --locked --target "$target" --manifest-path "$root/Cargo.toml" >&2
  mkdir -p "$out/$abi"
  "$linker" -shared -fPIC -O2 -Wall -Werror -I"$root/crates/graphlava-ffi/include" \
    "$here/graphlava/src/main/cpp/graphlava_jni.c" "$root/target/$target/release/libgraphlava_ffi.a" \
    -Wl,-z,max-page-size=16384 -Wl,--gc-sections -llog -lm -ldl -o "$out/$abi/libgraphlava.so"
  "$bin/llvm-strip" --strip-unneeded "$out/$abi/libgraphlava.so"
done
READELF="$bin/llvm-readelf" "$here/scripts/check-16k-alignment.sh" "$out"/*/libgraphlava.so
