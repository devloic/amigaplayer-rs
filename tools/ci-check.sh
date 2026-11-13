#!/bin/bash
# Regression check: build, unit tests, then the whole refcmp list against the baseline.
# Fails (exit 1) on a build or test failure, or when refcmp finds a flagged file that the
# baseline does not (refcmp's own exit status). Class changes among already flagged files are
# printed in the summary but do not fail the run.
#
#   tools/ci-check.sh [-j N] [--quick]       (--quick: skip the full refcmp, run the tests only)
#
# The application is built twice: the default build (no 68k player support) and the build with
# `--features host-68k`, which refcmp needs: its reference side runs the original 68k players
# (`--68k-players`). The 68k binary is built WITHOUT the `native` feature, so without the
# recompiled players even in a checkout that has crates/players-native/gen (the baseline is
# measured without them; see the end of tools/refcmp.py's docstring).
#
# The build goes to a directory of its own, not target/release: that is a symlink into a cache
# shared by every worktree, and another build overwrites the binary a run is comparing.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$HERE")"
WORK="${CI_WORK:-/media/lolo/kingston2tb/claude-scratch/deliplayerrust/ci}"
JOBS="$(( $(nproc) / 2 ))"
QUICK=
while [ $# -gt 0 ]; do
  case "$1" in
    -j) JOBS="$2"; shift ;;
    --quick) QUICK=1 ;;
    *) echo "usage: $0 [-j N] [--quick]" >&2; exit 2 ;;
  esac
  shift
done
mkdir -p "$WORK/bin"
export CARGO_TARGET_DIR="$WORK/target" CARGO_BUILD_BUILD_DIR="$WORK/builddir"
cd "$ROOT"
echo "== build (default: no 68k players)"
cargo build --release
echo "== build (--features host-68k: the refcmp binary)"
cargo build --release --features host-68k
cp "$CARGO_TARGET_DIR/release/amigaplayer-rs" "$WORK/bin/amigaplayer-rs"
echo "== cargo test (default)"
cargo test --release
echo "== cargo test (--features host-68k)"
cargo test --release --features host-68k
[ -n "$QUICK" ] && { echo "ok (quick: refcmp skipped)"; exit 0; }
echo "== refcmp --all (baseline tools/refcmp-baseline.tsv)"
# no sound to the speakers; the list and caches are those of the listen page's scratch folder
SDL_AUDIO_DRIVER=dummy python3 -I "$HERE/refcmp.py" --all -j "$JOBS" \
  --bin "$WORK/bin/amigaplayer-rs" --out "$WORK/refcmp-out" --cache "$WORK/refcmp-cache"
echo "ok"
