# Spiral Robotics

https://github.com/user-attachments/assets/62bfac14-96e2-48a6-bd39-38ec80c7fbc2

Spiral Robotics is a demo app showcasing what you can build with [Spiral](https://spiraldb.com).
[Reach out to us](mailto:team@spiraldb.com) to get access.

Spiral Robotics is a Rust-native robotics toolkit and browser studio inspired by
[LeRobot](https://github.com/huggingface/lerobot) and
[Rerun](https://github.com/rerun-io/rerun). It sets up, calibrates, and operates
SO-ARM101 arms; records synchronized robot and camera data directly in SpiralDB;
queries and curates that data; and trains vision-conditioned ACT policies on
Modal GPUs from curated views ([docs/training.md](docs/training.md)). Running a
trained policy on a robot is not supported yet.

It runs as two programs. **Studio** (`crates/backend` and `crates/frontend`)
serves the browser app, keeps Studio state, and reaches SpiralDB; it runs at
`robotics.spiral.build` or on `localhost`. The **robot connector**
(`robot-connector`) runs next to the arms: it owns their serial buses,
cameras, calibration, and control loop, and records episodes.
The connector links itself to Studio over plain HTTP, and the browser reaches
it only through Studio, which relays the browser's requests and serves the
live data the connector posts. The connector uploads each finished episode to
Studio, and the browser stores it in its dataset.

Studio stores its control-plane state in persistent backend-local SQLite
(SeaORM and versioned migrations). SpiralDB stores datasets and model artifacts.
Settings can transfer project data and local Studio state as one `.spiral-project`
file. See [SQLite storage and project bundles](docs/studio-sqlite.md).

The backend HTTP API and curl examples are documented in
[Studio HTTP API](docs/http-api.md).

## Development

Use [Nix](https://nixos.org/download.html) for development. `flake.nix` and
`flake.lock` own the Rust toolchain, build tools, and
their environment variables. With direnv, `.envrc` loads the same default shell.

```console
nix develop .
```

Studio builds on a published Spiral Rust SDK release. The SDK links that
release's verified native libspiral archive, which the default shell installs
and caches on entry. Outside the shell, install it with:

```console
scripts/setup-spiral-sdk.sh
```

See [database setup](docs/database-boundary.md) for the release and its archive.

Run CI commands with `nix develop .#ci --command <command>`.

Start the robot connector next to the arms, from the workspace root in the
Nix shell (which supplies FFmpeg for camera recording):

```console
cargo run -p spiral-robotics-connector-os -- --studio http://localhost:3000 --token rc_…
```

The token comes from **Rollout → Set up**, which issues one per workspace and
shows this command with it. The connector links itself to that Studio — it
listens on no port, so a robot needs no public address — and keeps its robots
and unsent episodes under the platform's local data directory
(`--data-dir` chooses another).

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
selection in the browser. Curation's per-row model work runs in the backend's
fill runs: Break down and the judgments of its subtasks call OpenRouter, and
Score progress and Track objects their Modal services. `GET /api/providers` tells the frontend which
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

Studio runs at `robotics.spiral.build` on one Compute Engine VM, defined by
`robotics.tf` and `config/robotics-studio/` in `spiraldb/infra`. A Cloudflare
Tunnel carries its traffic, and Studio's own Spiral sign-in gates every API
request. Each release, a pushed `v*` tag, publishes its Linux image, the flake's
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
requests bound for its provider's origin, Curation's fill runs send the
OpenRouter key with their own model calls, and Studio passes the Hugging Face
token to the Spiral driver as `spiral.huggingface_token`. The startup log says which keys are set, never
their values. To change a key, restart the backend.

Robot discovery calls `openai/gpt-5.6-terra` through OpenRouter, with `medium`
reasoning effort and fast mode requested using `service_tier: "priority"`.
Priority processing costs more than standard processing; the provider
determines the actual served tier. The process environment's `LLM_BASE_URL`
remains an optional developer override of the endpoint.

Training runs on the Modal app in
[`modal/training/`](modal/training/README.md), which Studio reaches through
`TRAINING_SERVICE_URL` and `MODAL_SERVICE_TOKEN`.

Every browser signs in through a registered Spiral application. Choose **Sign in
with Spiral**, approve access on Spiral's consent page, then choose an existing
project in **Settings → SpiralDB**. The application requests `openid email` for
identity and `projects:read catalog:read catalog:write tables:read tables:write`
for project data. Its permissions are also limited by the signed-in user's
permissions. Create projects in Spiral Studio before adding their workspaces here.

Set `SPIRAL_ROBOTICS_CLIENT_ID` to the registration for this environment.
The Nix development shell sets Dev (`client_sQWdRwk7ZTBd`); the hosted image
sets Prod (`client_3CMizYLaPa3h`). These IDs are public, not secrets.
`SPIRAL_ROBOTICS_PUBLIC_URL` (or `--public-url`) sets the address browsers reach
Studio at, `http://localhost:3000` by default. Prod registers
`https://robotics.spiral.build/callback`; Dev registers `http://localhost/callback`,
which matches any port. Both registrations belong to the project
`xiangpeng-robotics-618551` and need no client secret. Each is installed in the
Spiral organization on every project: a registered application's tokens reach only
the projects its installation selects, so without one Studio sees no project.
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

Add simulated arms from Rollout → Set up for an SO-101 leader and follower
filmed by simulated cameras, no hardware needed; see
[the robot connector API](docs/robot-connector-api.md#simulated-robots). Start
it with `--no-cameras` only when camera discovery is intentionally unavailable. Settings may switch the SpiralDB
project only while training is stopped. Project preferences live in Studio's
SQLite database.
