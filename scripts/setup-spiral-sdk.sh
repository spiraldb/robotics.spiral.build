#!/usr/bin/env bash
# Install the verified native libspiral archive of the SDK release Cargo.lock pins.
# Extra arguments go to the release's setup script.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fetch --locked
sdk_manifest="$(cargo metadata --locked --format-version 1 | python3 -c '
import json, sys
packages = json.load(sys.stdin)["packages"]
print(next(p["manifest_path"] for p in packages if p["name"] == "spiral-sdk"))
')"
# SDK 0.1.16's packaged installer looks one directory too high for its release
# record. Its supported --commit mode installs the same verified native archive.
sdk_source="$(python3 -c '
import json, pathlib, sys
print(json.loads(pathlib.Path(sys.argv[1]).with_name("native-runtime.json").read_text())["source_sha"])
' "$sdk_manifest")"
python3 "$(dirname "$sdk_manifest")/scripts/setup-native-runtime.py" --commit "$sdk_source" "$@"
