#!/usr/bin/env bash
# Dev shortcut for the launcher: ./run.sh [--plugins DIR] [--renderer opengl] [SONG]
set -euo pipefail
cd "$(dirname "$0")"
exec cargo run -q -p retrovert-gui --release -- "$@"
