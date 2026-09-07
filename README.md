# Spiral Robotics

Spiral Robotics is a Rust-native robotics toolkit and browser studio inspired by
[LeRobot](https://github.com/huggingface/lerobot) and
[Rerun](https://github.com/rerun-io/rerun). It sets up, calibrates, and operates
SO-ARM101 arms; records synchronized robot and camera data directly in SpiralDB;
queries that data; trains vision-conditioned ACT policies; and safely
executes trained policies through the follower's bounded rollout path.

SpiralDB is Studio's only persistent storage layer. Studio can transfer the
complete active project from Settings as one `.spiral-project` file.

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

Start the studio with:

```console
cd crates/studio
dx serve --release
```

For a developer-local Workflow Assistant, put the backend-only credential in
the repository root's ignored `.env.local` file. Studio reads this file
automatically when a corresponding process value is absent, so it works
without direnv or manually exporting secrets:

```dotenv
LLM_API_KEY=your-provider-key
```

The default is `openai/gpt-5.6-terra` through OpenRouter. `LLM_MODEL` and
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
dx serve
```

On first launch, Studio opens **Settings → SpiralDB** so you can enter the
Spiral API endpoint, SpFS endpoint, and project ID. The hosted services are the
defaults; for the local stack use `http://127.0.0.1:4279` and
`http://127.0.0.1:4295`. The backend remembers these three bootstrap pointers
in one private JSON file. Project-only switches apply immediately while Studio
is idle. Service-endpoint and credential changes also apply live while Studio
is idle, after the old repository pool drains.
`--spiral-project` or `SPIRAL_PROJECT_ID` may still provide an initial project
override.
`SPIRAL_ROBOTICS_BACKEND_STATE` overrides the JSON file path for development
and deployment isolation.

Studio currently has no product simulation mode. Use `--no-cameras` only when
camera discovery is intentionally unavailable; configured robot connections
remain physical. Settings may switch the SpiralDB project only while robots and
project activity are stopped. The namespace remains fixed for the server
lifetime; project-owned training and video preferences remain inside Spiral.
