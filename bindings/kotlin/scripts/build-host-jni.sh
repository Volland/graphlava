#!/usr/bin/env bash
# Builds the JNI library for the JVM host tests: libgraphlava_ffi.a (cargo)
# plus src/main/cpp/graphlava_jni.c, into build/host-jni/. Needs JAVA_HOME.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$here/../.." && pwd)
out="${1:-$here/graphlava/build/host-jni}"
cargo build -p graphlava-ffi --release --locked --manifest-path "$root/Cargo.toml" >&2
mkdir -p "$out"
case "$(uname -s)" in
  Darwin) os=darwin; lib=libgraphlava.dylib; extra=(-dynamiclib -lc -lm) ;;
  *) os=linux; lib=libgraphlava.so; extra=(-shared -fPIC -lpthread -lm -ldl) ;;
esac
cc -std=c11 -O2 -Wall -Wextra -Werror \
  -I"$JAVA_HOME/include" -I"$JAVA_HOME/include/$os" -I"$root/crates/graphlava-ffi/include" \
  "$here/graphlava/src/main/cpp/graphlava_jni.c" "$root/target/release/libgraphlava_ffi.a" \
  "${extra[@]}" -o "$out/$lib"
echo "$out/$lib"
