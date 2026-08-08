#!/usr/bin/env bash
# Build the studio's web interface into crates/rerobot-ui/dist.
#
# Two toolchains are involved: wasm-pack compiles and bundles the Rust, and the
# Tailwind CLI generates the stylesheet from the classes used in the .rs files.
# The nix development shell provides both; without nix, wasm-pack must be on
# PATH and Tailwind is fetched through npm.
set -euo pipefail

repository="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ui="$repository/crates/rerobot-ui"
dist="$ui/dist"

profile="--release"
if [[ "${1:-}" == "--dev" ]]; then
  profile="--dev"
  shift
fi

if ! command -v wasm-pack >/dev/null 2>&1; then
  echo "error: wasm-pack is not installed." >&2
  echo "       Enter the dev shell with 'nix develop path:.', or run" >&2
  echo "       'cargo install wasm-pack'." >&2
  exit 1
fi

echo "==> Compiling the WebAssembly bundle"
# wasm-pack fetches a wasm-bindgen that matches the compiled crate, which keeps
# the notoriously version-sensitive pair in step without pinning it here.
(cd "$ui" && wasm-pack build "$profile" --target web --no-typescript --no-pack --out-dir dist)

echo "==> Generating the stylesheet"
tailwind=(tailwindcss)
if ! command -v tailwindcss >/dev/null 2>&1; then
  if command -v npx >/dev/null 2>&1; then
    if [[ ! -d "$ui/node_modules" ]]; then
      echo "    installing the Tailwind CLI with npm"
      (cd "$ui" && npm install --silent --no-audit --no-fund)
    fi
    tailwind=(npx --no-install @tailwindcss/cli)
  else
    echo "error: neither tailwindcss nor npx is installed." >&2
    echo "       Enter the dev shell with 'nix develop path:.'." >&2
    exit 1
  fi
fi
# Written as two calls rather than an optional array element: macOS ships bash
# 3.2, where expanding an empty array under `set -u` is an error.
if [[ "$profile" == "--dev" ]]; then
  (cd "$ui" && "${tailwind[@]}" -i input.css -o dist/app.css)
else
  (cd "$ui" && "${tailwind[@]}" -i input.css -o dist/app.css --minify)
fi

cp "$ui/index.html" "$dist/index.html"
# wasm-pack leaves a .gitignore that would hide the bundle from tooling that
# walks the tree; the repository ignores dist/ already.
rm -f "$dist/.gitignore"

echo "==> Built $dist"
ls -1sh "$dist" | sed '1d;s/^/    /'
echo
echo "Run the studio with:"
echo "    cargo run --bin rerobot-studio"
