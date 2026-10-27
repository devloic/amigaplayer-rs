#!/bin/sh
# Downloads Eagleplayer 2.06 from Aminet and installs its 68k Eagleplayers
# where deliplayer looks for them (~/.config/deliplayer/Eagleplayers).
#
#   tools/fetch-eagleplayers.sh [DEST]      (default DEST: ~/.config/deliplayer)
#   EP_ARCHIVE=/path/Eagleplayer.lha tools/fetch-eagleplayers.sh   (skip download)
#
# Needs curl and lhasa.
set -eu

dest="${1:-$HOME/.config/deliplayer}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

archive="${EP_ARCHIVE:-}"
if [ -z "$archive" ]; then
    archive="$tmp/Eagleplayer.lha"
    echo "downloading Eagleplayer.lha from aminet.net (slow server, ~2.9 MB)..."
    curl -fL --retry 3 -o "$archive" https://aminet.net/mus/play/Eagleplayer.lha
fi

(cd "$tmp" && lhasa x "$archive" >/dev/null)
mkdir -p "$dest"
rm -rf "$dest/Eagleplayers"
cp -R "$tmp/Eagleplayer_2.06/Eagleplayers" "$dest/Eagleplayers"
n=$(find "$dest/Eagleplayers" -type f ! -name '*.info' | wc -l)
echo "$n Eagleplayers installed in $dest/Eagleplayers"
