#!/bin/bash
# Checks the library side of the repository, end to end (no network, no audio device):
#   1. the open-replayer library with default features off: build, tests (unit, integration,
#      doctests), examples
#   2. the application (default features) still builds
#   3. the C API: build the cdylib/staticlib, compile examples/c/play.c, link it both ways,
#      render a synthetic module and compare the samples with the Rust example's
#   4. the WebAssembly crate: wasm32-unknown-unknown build, wasm-strip when available, and a
#      node run of the dp_* ABI (crates/amigaplayer-wasm/test/smoke.mjs) when node is available
#
#   tools/lib-check.sh [--no-wasm] [--no-app]
#
# Builds go to a directory of their own, not target/: that may be a symlink into a cache shared
# by other worktrees (override with LIB_WORK).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$HERE")"
WORK="${LIB_WORK:-/media/lolo/kingston2tb/claude-scratch/deliplayerrust/libcheck}"
WASM=1
APP=1
for a in "$@"; do
  case "$a" in
    --no-wasm) WASM= ;;
    --no-app) APP= ;;
    *) echo "usage: $0 [--no-wasm] [--no-app]" >&2; exit 2 ;;
  esac
done
mkdir -p "$WORK"
export CARGO_TARGET_DIR="$WORK/target" CARGO_BUILD_BUILD_DIR="$WORK/builddir"
export SDL_AUDIO_DRIVER=dummy
cd "$ROOT"
T="$CARGO_TARGET_DIR/release"

echo "== 1. library, default features off"
cargo build --release --lib --no-default-features
cargo test --release --no-default-features
cargo build --release --no-default-features --examples

echo "== 2. application"
if [ -n "$APP" ]; then cargo build --release; else echo "(skipped)"; fi

echo "== 3. C API"
cargo build --release -p amigaplayer-capi
"$T/examples/make_demo_mod" "$WORK/demo.mod"
"$T/examples/play_to_wav" "$WORK/demo.mod" "$WORK/demo-rust.wav" 3
for link in shared static; do
  if [ "$link" = shared ]; then
    cc -O2 -Wall -Wextra -Icrates/amigaplayer-capi/include examples/c/play.c -L"$T" -lamigaplayer \
      -Wl,-rpath,"$T" -o "$WORK/play-$link"
  else
    cc -O2 -Wall -Wextra -Icrates/amigaplayer-capi/include examples/c/play.c "$T/libamigaplayer.a" \
      -lpthread -ldl -lm -o "$WORK/play-$link"
  fi
  "$WORK/play-$link" "$WORK/demo.mod" "$WORK/demo-$link.raw" 3
  # the WAV of the Rust example is a 44-byte header and the same samples
  tail -c +45 "$WORK/demo-rust.wav" | cmp - "$WORK/demo-$link.raw"
  echo "C ($link) output == Rust output"
done
# the error path: garbage is refused with a reason, not a crash
head -c 3000 /dev/urandom > "$WORK/garbage.bin"
if "$WORK/play-shared" "$WORK/garbage.bin" "$WORK/garbage.raw" 2>"$WORK/garbage.err"; then
  echo "garbage was accepted" >&2; exit 1
fi
echo "garbage refused: $(head -1 "$WORK/garbage.err")"

echo "== 4. WebAssembly"
if [ -n "$WASM" ]; then
  rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown \
    || { echo "wasm32-unknown-unknown is not installed: rustup target add wasm32-unknown-unknown" >&2; exit 1; }
  cargo build --release --manifest-path crates/amigaplayer-wasm/Cargo.toml --target wasm32-unknown-unknown
  W="$WORK/amigaplayer_wasm.wasm"
  cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/amigaplayer_wasm.wasm" "$W"
  if command -v wasm-strip >/dev/null; then wasm-strip "$W"; fi
  echo "wasm size: $(stat -c %s "$W") bytes"
  if command -v node >/dev/null; then node crates/amigaplayer-wasm/test/smoke.mjs "$W"; else echo "(node not found: smoke test skipped)"; fi
else
  echo "(skipped)"
fi
echo "ok"
