#!/bin/sh
# Downloads DeliTracker 2.32 from Aminet and installs its 68k DeliPlayers and
# DeliGenies where deliplayer looks for them (~/.config/deliplayer/DeliPlayers,
# ~/.config/deliplayer/DeliGenies).
#
#   tools/fetch-deliplayers.sh [DEST]      (default DEST: ~/.config/deliplayer)
#   DT_ARCHIVE=/path/DeliTracker232.lha tools/fetch-deliplayers.sh   (skip download)
#
# Needs curl and lhasa.
set -eu

dest="${1:-$HOME/.config/deliplayer}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

archive="${DT_ARCHIVE:-}"
if [ -z "$archive" ]; then
    archive="$tmp/DeliTracker232.lha"
    echo "downloading DeliTracker232.lha from aminet.net (slow server, ~1.2 MB)..."
    curl -fL --retry 3 -o "$archive" https://aminet.net/mus/play/DeliTracker232.lha
fi

(cd "$tmp" && lhasa x "$archive" >/dev/null)
mkdir -p "$dest"
(cd "$dest" && lhasa x "$tmp/DeliTracker_II/Files/players.lha" >/dev/null)
(cd "$dest" && lhasa x "$tmp/DeliTracker_II/Files/genies.lha" >/dev/null)
n=$(find "$dest/DeliPlayers" -type f ! -name '*.info' | wc -l)
echo "$n DeliPlayers installed in $dest/DeliPlayers"
n=$(find "$dest/DeliGenies" -type f ! -name '*.info' | wc -l)
echo "$n DeliGenies installed in $dest/DeliGenies"
