#!/usr/bin/env bash
# Builds the Rust bridge, then the Qt shim against it. Output: shim/build/qml/Retrovert/.
set -euo pipefail
cd "$(dirname "$0")"
profile=${1:-release}
cargo build -p retrovert-ui-bridge --profile "$([ "$profile" = debug ] && echo dev || echo "$profile")"
target=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
cmake -S shim -B shim/build -DCMAKE_BUILD_TYPE=Release -DRV_BRIDGE_DIR="$target/$profile" -Wno-dev >/dev/null
cmake --build shim/build -j
