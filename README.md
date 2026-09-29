# Spiral Robotics

Spiral Robotics is a Rust-native robotics toolkit and browser studio inspired by
[LeRobot](https://github.com/huggingface/lerobot) and
[Rerun](https://github.com/rerun-io/rerun). It sets up, calibrates, and operates
SO-ARM101 arms; records synchronized robot and camera data directly in SpiralDB;
queries and curates that data; trains vision-conditioned ACT policies; and
safely executes trained policies through the follower's bounded rollout path.
Training and Rollout are parked until Spiral decodes MCAP video into training
images: Studio refuses to start a run, and Rollout runs existing checkpoints.

Studio stores its control-plane state in persistent backend-local SQLite
(SeaORM and versioned migrations). SpiralDB stores datasets and model artifacts.
Settings can transfer project data and local Studio state as one `.spiral-project`
file. See [SQLite storage and project bundles](docs/studio-sqlite.md).

The backend HTTP API and curl examples are documented in
[Studio HTTP API](docs/http-api.md).

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
[datasets and recordings](docs/datasets.md).

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

Agent orchestration belongs to the frontend: imported robot discovery and
Curation's category proposals own their prompts, model responses, and tool
selection in the browser. Curation's per-row model work runs in Spiral, as the
`spql.ai` functions its views compile to. `GET /api/providers` tells the frontend which
model provider is configured; it constructs provider requests and interprets
responses. `/api/proxy` relays external HTTP requests, attaches the key Studio
holds for the destination's origin, and streams the responses back.
Native data operations, robot validation, and persistence remain on the backend.
Browser orchestration ends when its owning task is disposed; this does not undo
completed data operations.

Build a distributable server and browser bundle from the workspace root with:

```console
dx bundle --fullstack --release --debug-symbols false \
  @client --package spiral-robotics-frontend \
  @server --package spiral-robotics-backend
```

Keep the bundle's `public` directory beside the server binary.

Keep `--debug-symbols false` on browser builds: DX enables WASM debug symbols by
default, and the bundled Binaryen currently fails when processing that DWARF.
The Cargo profile's `debug = 0` does not disable DX's separate bundling option.

### Desktop app

`crates/desktop` packages Studio as a macOS app with [Tauri](https://tauri.app).
The backend runs inside the app on a loopback port the operating system picks,
and one window shows the same web interface `dx serve` builds. The app keeps
its SQLite state in `~/Library/Application Support/com.spiraldb.robotics/`
and shares the Spiral sign-in with the `spiraldb` CLI. A second launch shows
the running app's window.

Build it from `crates/desktop` in the `local-driver` shell, which supplies
`cargo tauri`:

```console
cd crates/desktop
CARGO_TARGET_DIR=../../target/desktop cargo tauri build --bundles app
```

The app lands in `target/desktop/release/bundle/macos/`. The desktop build
keeps its own target directory, so it never touches a running `dx serve`.
It bundles this checkout's local driver, which links Homebrew's OpenSSL and
loads its video runtime from this checkout's build directory, so the app runs
only on the Mac that built it. Camera recording still runs `ffmpeg` from
`PATH`, which an app opened from Finder does not see.

Every push to `main` releases the app through
`.github/workflows/desktop-release.yml`. It builds the driver from the SpiralDB
branch named in its `SPIRALDB_REF`, reading the private repository with the
`SPIRALDB_DEPLOY_KEY` deploy key, with static OpenSSL and the video runtime and
LGPL FFmpeg libraries beside it in `Contents/Frameworks`. A driver is built
once per SpiralDB commit and cached. The ad-hoc signed DMG is published as a
`desktop-v0.1.<run>` release of this repository, which only organization
members can download.

Interface primitives (buttons, fields, facts, disclosures, trees, notices)
live in `crates/ui` as `spiral-robotics-ui`, with their stylesheet; the
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

The frontend's stylesheet, `crates/frontend/assets/tailwind.css`, is generated
and untracked. `dx` builds it; before a plain `cargo check` or `cargo test` on a
fresh clone, build it once in the Nix shell:

```console
tailwindcss -i crates/frontend/tailwind.css -o crates/frontend/assets/tailwind.css
```

Enter the OpenRouter API key and the Hugging Face token in **Settings → Keys**.
The backend saves them in its SQLite database, beside the saved connection, and
the browser only learns which ones are saved. The relay attaches each key to
requests bound for its provider's origin, and Studio passes both to the Spiral
driver: the OpenRouter key as `spiral.openrouter_api_key` for the `spql.ai`
model functions Curation compiles to, and the token as
`spiral.huggingface_token`. A saved key takes effect on the next request and
the next database connection; open projects keep running. Keys belong to
Studio, independently of the selected project, and project exports never
contain them.

Robot discovery calls `openai/gpt-5.6-terra` through OpenRouter, with `medium`
reasoning effort and fast mode requested using `service_tier: "priority"`.
Priority processing costs more than standard processing; the provider
determines the actual served tier. The process environment's `LLM_BASE_URL`
remains an optional developer override of the endpoint.

Start the Python policy compute runtime separately (on the same machine or a
remote compute host). It owns PyTorch for rollout inference and, once training
is unparked, for training:

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

On first launch, Studio opens **Settings → SpiralDB**. Choose **Sign in with
Spiral**, confirm the code on the Spiral sign-in page, then pick a project. Studio
signs in the same way `spiraldb login` does and shares that session: if the CLI
is already signed in, Studio starts signed in, and signing out of Studio signs
the CLI out too. The backend holds the session and keeps its short-lived access
token fresh; the browser never sees a token.
The hosted services are the default endpoints; for the local stack use
`http://127.0.0.1:4279` and `http://127.0.0.1:4295`. The backend remembers the
endpoints and project in its private local SQLite database. Project-only switches
apply immediately while Studio is idle. Service-endpoint changes also apply live
while Studio is idle, after the old repository pool drains; signing out or
switching organization closes the open projects.
The SQLite file defaults to `.studio/studio.sqlite3` in this repository;
the entire `.studio/` directory is Git-ignored.
`SPIRAL_ROBOTICS_STUDIO_DB` overrides the SQLite file path for development
and deployment isolation.

Studio currently has no product simulation mode. Use `--no-cameras` only when
camera discovery is intentionally unavailable; configured robot connections
remain physical. Settings may switch the SpiralDB project only while robots and
project activity are stopped. Project preferences live in Studio's SQLite
database.
