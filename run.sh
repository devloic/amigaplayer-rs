#!/bin/sh
# Builds amigaplayer-rs (release), installs the DeliPlayers on first use, and
# starts it. All arguments are passed through:
#
#   ./run.sh                         empty player window
#   ./run.sh ~/mods/ song.mod        open files/folders in the module list
#   ./run.sh --wav out.wav MODULE    render to WAV instead
#
# Set DELIPLAYERS=/path/to/DeliPlayers to use another players directory.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
cd "$here"

cargo build --release --quiet

players="${DELIPLAYERS:-$HOME/.config/amigaplayer-rs/DeliPlayers}"
# the folder of the older deliplayer name, when only that one is there
[ -z "${DELIPLAYERS:-}" ] && [ ! -d "$players" ] && [ -d "$HOME/.config/deliplayer/DeliPlayers" ] &&
    players="$HOME/.config/deliplayer/DeliPlayers"
if [ ! -d "$players" ]; then
    echo "DeliPlayers not found in $players - fetching them from Aminet..."
    tools/fetch-deliplayers.sh "$(dirname "$players")"
fi

exec "$here/target/release/amigaplayer-rs" --players "$players" "$@"
