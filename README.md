# Spiral Robotics

Spiral Robotics is a Rust-native robotics toolkit and browser studio inspired by
[LeRobot](https://github.com/huggingface/lerobot) and
[Rerun](https://github.com/rerun-io/rerun). It sets up, calibrates, and operates
SO-ARM101 arms; records synchronized robot and camera data directly in SpiralDB;
queries and curates that data; trains vision-conditioned ACT policies on
Modal GPUs from curated views ([docs/training.md](docs/training.md)); and
safely executes trained policies through the follower's bounded rollout path.

It runs as two programs. **Studio** (`crates/backend` and `crates/frontend`)
serves the browser app, keeps Studio state, and reaches SpiralDB; it runs at
`robotics.spiraldb.com` or on `localhost`. The **robot connector**
(`robot-connector`) runs next to the arms: it owns their serial buses,
cameras, calibration, and control loop, records takes, and rolls out policies.
The browser talks to the robot connector directly — Studio never reaches it —
carries each finished take from it to Studio, and writes the checkpoints Studio
trained to it.

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

The default and CI shells select the pinned published Spiral ADBC library. To
try an unreleased upstream driver, build it and enter `nix develop .#local-driver`,
which selects the local library automatically; see
[database setup](docs/database-boundary.md) for the build instructions.

Run CI commands with `nix develop .#ci --command <command>`.

Start the robot connector next to the arms, from the workspace root in the
Nix shell (which supplies FFmpeg for camera recording):

```console
cargo run -p spiral-robotics-connector-os
```

It listens on `http://127.0.0.1:7070` and keeps its robots, unsent takes, and
policies under the platform's local data directory (`--data-dir` chooses
another). In Studio, **Rollout → Set up** sets the address
the workspace drives, for everyone in it: `localhost` means each person's own
machine, and a network address one robot connector they share.

Start the studio from the workspace root with:

```console
dx serve --fullstack --release --debug-symbols false --port 3000 \
  @client --package spiral-robotics-frontend \
  @server --package spiral-robotics-backend
```

Open `http://localhost:3000`. Spiral's sign-in returns browsers only to
registered addresses, and `http://localhost:3000/callback` is the local one, so
a local Studio must be reached on port 3000.

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
Native data operations and persistence remain on the backend; hardware
remains on the robot connector.
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

### Hosted

[`deploy/hosted`](deploy/hosted/README.md) runs Studio for Spiral colleagues at
`robotics.spiraldb.com`, on one Compute Engine VM behind Cloudflare Access.
Each release, a pushed `v*` tag, publishes its Linux image, the flake's
`hosted-image`, through `.github/workflows/hosted-image.yml`.

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

Studio's provider keys belong to its deployment, not to its users: the backend
reads them from its environment at startup, and no page shows or sets them.

| Variable | Used for |
| --- | --- |
| `OPENROUTER_API_KEY` | Model calls: Break down, Classify, Find similar, robot discovery |
| `HF_TOKEN` | Downloads of models and datasets from Hugging Face |
| `WARP_PROGRESS_URL` | Score progress, on the Modal service in `modal/warp_progress` |
| `OBJECT_TRACKING_URL` | Track objects, on the Modal service in `modal/object_tracking` |
| `TRAINING_SERVICE_URL` | Training, on the Modal service in `training/` |
| `MODAL_SERVICE_TOKEN` | The bearer token every Modal service accepts |

The Nix development shells export the checkout's untracked `.env.local`, so a
local Studio started from them gets the keys without further setup:

```sh
OPENROUTER_API_KEY='sk-or-…'
HF_TOKEN='hf_…'
WARP_PROGRESS_URL='https://spiral-dev--warp-progress-warpprogress-web.modal.run'
OBJECT_TRACKING_URL='https://spiral-dev--spiral-robotics-object-tracking-objecttracking-web.modal.run'
TRAINING_SERVICE_URL='https://spiral-dev--spiral-robotics-training-control.modal.run'
MODAL_SERVICE_TOKEN='…'
```

An unset key turns off only what needs it. The relay attaches each key to
requests bound for its provider's origin, and Studio passes the first two to
the Spiral driver: the OpenRouter key as `spiral.openrouter_api_key` for the
`spql.ai` model functions Curation compiles to, and the token as
`spiral.huggingface_token`. The startup log says which keys are set, never
their values. To change a key, restart the backend.

Robot discovery calls `openai/gpt-5.6-terra` through OpenRouter, with `medium`
reasoning effort and fast mode requested using `service_tier: "priority"`.
Priority processing costs more than standard processing; the provider
determines the actual served tier. The process environment's `LLM_BASE_URL`
remains an optional developer override of the endpoint.

Training runs on the Modal app in
[`modal/training/`](modal/training/README.md), which Studio reaches through
`TRAINING_SERVICE_URL` and `MODAL_SERVICE_TOKEN`.
The same Python project is the policy runtime for rollout inference: start it
next to the robot connector, which calls it (`--policy-runtime`,
`http://127.0.0.1:50051` by default):

```console
cd modal/training
uv sync --extra dev
uv run spiral-training-runtime --port 50051
```

The policy runtime refuses non-loopback listeners by default. Its
`--allow-remote-access` is reserved for deployments placed behind an
authenticated TLS proxy.

Every browser signs in through a registered Spiral application. Choose **Sign in
with Spiral**, approve access on Spiral's consent page, then choose an existing
project in **Settings → SpiralDB**. The application requests `openid email` for
identity and `projects:read catalog:read catalog:write tables:read tables:write`
for project data. Its permissions are also limited by the signed-in user's
permissions. Create projects in Spiral Studio before adding their workspaces here.

Set `SPIRAL_ROBOTICS_CLIENT_ID` to the registration for this environment.
The Nix development shell sets Dev (`client_aRm8njLb18Px`); the hosted image
sets Prod (`client_PiwYy0njXU92`). These IDs are public, not secrets.
`SPIRAL_ROBOTICS_PUBLIC_URL` (or `--public-url`) sets the address browsers reach
Studio at, `http://localhost:3000` by default. Register its exact `/callback` URL:
`http://localhost:3000/callback` for Dev and
`https://robotics.spiraldb.com/callback` for Prod. The two registrations belong to
the Spiral organization and have separate client IDs. No client secret is needed.
`SPIRAL_ROBOTICS_AUTH_URL` defaults to `https://auth.spiraldb.com`.

Each browser gets its own session. The backend keeps that user's tokens in SQLite,
refreshes access tokens, and opens every project with the application's delegated
access. Identity comes from `/userinfo`; the application does not switch
organizations. The browser holds only an HttpOnly session cookie and never sees a
token. Sessions survive restarts. Signing out revokes the refresh token and closes
that browser's open projects. After migrating from the previous sign-in flow,
browsers must sign in again.
The hosted services are the default endpoints; `SPIRAL__SERVER__URL` and
`SPIRAL__SPFS__URL` choose others when the backend starts. The session remembers
its project, and project switches apply immediately while Studio is idle.
The SQLite file defaults to `.studio/studio.sqlite3` in this repository;
the entire `.studio/` directory is Git-ignored.
`SPIRAL_ROBOTICS_STUDIO_DB` overrides the SQLite file path for development
and deployment isolation.

Start the robot connector with `--demo` for a simulated SO-101 leader and
follower filmed by simulated cameras, no hardware needed; see
[the robot connector API](docs/robot-connector-api.md#simulated-robots). Start
it with `--no-cameras` only when camera discovery is intentionally unavailable. Settings may switch the SpiralDB
project only while training is stopped. Project preferences live in Studio's
SQLite database.
