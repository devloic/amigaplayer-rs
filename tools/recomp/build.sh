#!/bin/sh
# Builds m68k-recomp into target-recomp/release/m68k-recomp at the repo root (the internal
# disk, apart from the player's target/). Extra arguments go to cargo, e.g. `build.sh test`.
set -e
cd "$(dirname "$0")"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(cd ../.. && pwd)/target-recomp}"
if [ $# -eq 0 ]; then set -- build; fi
sub=$1
shift
cargo "$sub" --release "$@"
