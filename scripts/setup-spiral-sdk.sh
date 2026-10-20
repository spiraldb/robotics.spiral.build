#!/usr/bin/env bash
# Install the native ScanDAL archive of the Spiral Rust SDK release that Cargo.lock pins.
# The archive is a private release asset: set GH_TOKEN or GITHUB_TOKEN, or sign in with `gh`,
# with read access to spiraldb/spiraldb. Extra arguments go to the release's setup script.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fetch --locked
sys_manifest="$(cargo metadata --locked --format-version 1 | python3 -c '
import json, sys
packages = json.load(sys.stdin)["packages"]
print(next(p["manifest_path"] for p in packages if p["name"] == "spiral-scandal-sys"))
')"
python3 "$(dirname "$sys_manifest")/../../../scripts/setup-native-runtime.py" "$@"
