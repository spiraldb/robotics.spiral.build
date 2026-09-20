# Spiral Robotics

Spiral Robotics is a Rust-native robotics toolkit and browser studio inspired by
[LeRobot](https://github.com/huggingface/lerobot) and
[Rerun](https://github.com/rerun-io/rerun). It sets up, calibrates, and operates
SO-ARM101 arms; records synchronized robot and camera data directly in SpiralDB;
queries that data; trains vision-conditioned ACT policies; and safely
executes trained policies through the follower's bounded rollout path.

Studio stores its control-plane state in persistent backend-local SQLite
(SeaORM and versioned migrations). SpiralDB stores datasets and model artifacts.
Settings can transfer project data and local Studio state as one `.spiral-project`
file. See [SQLite storage and project bundles](docs/studio-sqlite.md).

The backend HTTP contract and independent-client examples are documented in
[Studio HTTP API](docs/http-api.md). A running backend serves its schemas at
`/api/openapi.json`.

## Development

Use [Nix](https://nixos.org/download.html) for development. `flake.nix` and
`flake.lock` own the Rust toolchain, build tools, standalone ADBC driver, and
their environment variables. With direnv, `.envrc` loads the same default shell.

```console
nix develop .
```

The default and CI shells select the pinned published Spiral ADBC library.
That archive is currently stale: hosted persistence requires a driver built
from current upstream. For this workspace's local release build, enter
`nix develop .#local-driver` instead. This shell selects the local library
automatically; see [database setup](docs/database-boundary.md) for its location.

Run CI commands with `nix develop .#ci --command <command>`.

The MCAP dataset inspector currently requires the `local-driver` shell;
the pinned release driver predates its query functions. See
[MCAP import design](docs/mcap-import-design.md).

Start the studio from the workspace root with:

```console
dx serve --fullstack --release --debug-symbols false \
  @client --package spiral-robotics-frontend \
  @server --package spiral-robotics-backend
```

Studio renders entirely in the browser. The native server serves the built HTML,
JavaScript, WASM, and assets alongside `/api`; it does not compile or render the
frontend. `--fullstack` tells DX to build and watch both processes, without enabling
Dioxus SSR or hydration. The frontend and backend each own their executable;
DX builds them together using the `@client` and `@server` package selections.
Build a distributable server and browser bundle from the workspace root with:

```console
dx bundle --fullstack --release --debug-symbols false \
  @client --package spiral-robotics-frontend \
  @server --package spiral-robotics-backend
```

Keep the bundle's `public` directory beside the server binary. Workflow displays
Spiral’s structured query plans using Dioxus Flow in the browser. See
[SQL workflow authoring](docs/workflow-authoring.md).

Keep `--debug-symbols false` on browser builds: DX enables WASM debug symbols by
default, and the bundled Binaryen currently fails when processing that DWARF.
The Cargo profile's `debug = 0` does not disable DX's separate bundling option.

Interface primitives (buttons, fields, facts, disclosures, trees, reordering,
notices) live in `crates/ui` as `spiral-robotics-ui`, with their stylesheet; the
frontend composes them and owns layout. Their unit tests run natively:

```console
cargo test -p spiral-robotics-ui
```

Frontend unit tests run as WASM in a browser using `wasm-bindgen-test`, including
the real browser timers and DOM APIs. In the Nix shell, run:

```console
cargo test -p spiral-robotics-frontend --lib --target wasm32-unknown-unknown --locked
```

The shell supplies the matching test runner, Firefox, and GeckoDriver on Linux
and macOS. Tests run in headless Firefox locally and in CI. For interactive
debugging, set `NO_HEADLESS=1` and open the URL printed by the runner in Firefox.

The frontend also supports native type-checking so rust-analyzer can analyze it
alongside the backend without changing the workspace target. It uses the same
browser implementation; only WASM registers its unit tests, and the native
Studio server does not depend on the frontend.

For a developer-local Workflow Assistant, put the backend-only credential in
the repository root's ignored `.env.local` file. Studio reads this file
automatically when a corresponding process value is absent, so it works
without direnv or manually exporting secrets:

```dotenv
LLM_API_KEY=your-provider-key
```

The default is `openai/gpt-5.6-terra` through OpenRouter, with `medium` reasoning
effort and fast mode requested using `service_tier: "priority"`. Priority
processing costs more than standard processing; the provider determines the
actual served tier. `LLM_MODEL` and
`LLM_BASE_URL` remain optional developer overrides, and explicit process values
take precedence over `.env.local`. Keep the file owner-readable only
(`chmod 600 .env.local`). Never commit it. Production deployments should
continue to inject the same values through their secret manager rather than a
repository-local file.

Start the Python policy compute runtime separately (on the same machine or a
remote compute host). It owns PyTorch for both training and rollout inference:

```console
cd training
uv sync --extra dev
uv run spiral-training-runtime --port 50051
```

For a fresh Lambda Stack machine, clone the repository and run
`./training/bootstrap-lambda.sh`. See [training/README.md](training/README.md)
for the runtime and Lambda workflow.

In Studio, open **Settings → Training**, enter the runtime's `http://` endpoint,
and choose **Save and connect**. New training and rollout sessions use that
endpoint; an active session keeps its existing connection.

Both Studio and the policy runtime refuse non-loopback listeners by default.
Use SSH forwarding for a remote worker. `--allow-remote-access` is reserved for
deployments placed behind an authenticated TLS proxy.

To connect Studio to a local SpiralDB stack:

```console
SPIRAL__DEV=1 \
dx serve --fullstack --debug-symbols false \
  @client --package spiral-robotics-frontend \
  @server --package spiral-robotics-backend
```

On first launch, Studio opens **Settings → SpiralDB** so you can enter the
Spiral API endpoint, SpFS endpoint, and project ID. The hosted services are the
defaults; for the local stack use `http://127.0.0.1:4279` and
`http://127.0.0.1:4295`. The backend remembers these three bootstrap pointers
in its private local SQLite database. Project-only switches apply immediately while Studio
is idle. Service-endpoint and credential changes also apply live while Studio
is idle, after the old repository pool drains.
`--spiral-project` or `SPIRAL_PROJECT_ID` may still provide an initial project
override.
The SQLite file defaults to `.studio/studio.sqlite3` in this repository;
the entire `.studio/` directory is Git-ignored.
`SPIRAL_ROBOTICS_STUDIO_DB` overrides the SQLite file path for development
and deployment isolation. The old `SPIRAL_ROBOTICS_BACKEND_STATE` JSON is a
one-time upgrade source; see the storage guide above.

Studio currently has no product simulation mode. Use `--no-cameras` only when
camera discovery is intentionally unavailable; configured robot connections
remain physical. Settings may switch the SpiralDB project only while robots and
project activity are stopped. The Studio table prefix (`studio` by default,
configured with `--studio-table-prefix`) remains fixed for the server lifetime;
project-owned training and video preferences remain inside Spiral.
