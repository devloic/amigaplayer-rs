#!/usr/bin/env bash
# Runs the archive's web player with the open replayers: rebuilds amigaplayer.wasm
# when the Rust sources are newer, copies it into the web app, and starts the web
# app's server.
#
#   tools/run-archive-web.sh              # http://127.0.0.1:8009/
#   tools/run-archive-web.sh --port 8019  # another port
#   tools/run-archive-web.sh --stop       # stop it
#
# ARCHIVE_APP is the web app's checkout (default: the scrapy project). Its own
# run-server.sh does the starting and stopping, replacing whatever holds the port.
# 8009 is the default because favourites, history and settings live in the
# browser's localStorage, which is per origin: on another port the page starts empty.
set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
app="${ARCHIVE_APP:-/media/lolo/seagate12tb/projects/mydev/amiga/scrapy}"
port=8009
stop=0
while [ $# -gt 0 ]; do
    case "$1" in
        --port) port="$2"; shift 2 ;;
        --stop) stop=1; shift ;;
        -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ "$stop" = 1 ]; then
    exec "$app/run-server.sh" --stop --port "$port"
fi

[ -x "$app/run-server.sh" ] || { echo "no web app at $app (set ARCHIVE_APP)" >&2; exit 1; }

# The wasm build stays on the internal disk, away from the USB drives.
target="${DELI_WASM_TARGET:-$HOME/.devcache/cargo-target-wasm}"
wasm="$target/wasm32-unknown-unknown/release/amigaplayer_wasm.wasm"
dest="$app/webapp/static/uade/amigaplayer.wasm"

newer=$(find "$here/src" "$here/crates/amigaplayer-wasm/src" "$here/Cargo.toml" \
             "$here/crates/amigaplayer-wasm/Cargo.toml" -newer "$dest" -print -quit 2>/dev/null || true)
if [ ! -f "$dest" ] || [ -n "$newer" ]; then
    echo "building amigaplayer.wasm..."
    (cd "$here/crates/amigaplayer-wasm" &&
        cargo build --release --target wasm32-unknown-unknown --target-dir "$target")
    cp "$wasm" "$dest"
    echo "copied $(du -h "$dest" | cut -f1) to $dest"
else
    echo "amigaplayer.wasm is up to date"
fi

echo "web player: http://127.0.0.1:$port/"
exec "$app/run-server.sh" --port "$port"
