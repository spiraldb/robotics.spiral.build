# Spiral Robotics

Spiral Robotics is a Rust-native robotics toolkit and browser studio inspired by
[LeRobot](https://github.com/huggingface/lerobot) and
[Rerun](https://github.com/rerun-io/rerun). It sets up, calibrates, and operates
SO-ARM101 arms; records synchronized robot and camera data directly in SpiralDB;
queries that data; trains vision-conditioned ACT policies; and safely
executes trained policies through the follower's bounded rollout path.

SpiralDB is Studio's only persistent storage layer. Studio can transfer the
complete active project from Settings as one `.spiral-project` file, while Data
imports and exports the selected dataset's episodes as `.spiral-episodes`.

## Development

The easiest way to setup dev env is to use [nix](https://nixos.org/download.html).

```console
nix develop .
```

Start the studio with:

```console
cd crates/spiral-robotics-studio
dx serve --release
```

Start the Python policy compute runtime separately (on the same machine or a
remote compute host). It owns PyTorch for both training and rollout inference:

```console
cd training
uv sync --extra dev
uv run spiral-training-runtime --host 0.0.0.0 --port 50051
```

For a fresh Lambda Stack machine, clone the repository and run
`./training/bootstrap-lambda.sh`. See [training/README.md](training/README.md)
for the runtime and Lambda workflow.

In Studio, open **Settings → Training**, enter the runtime's `http://` endpoint,
and choose **Save and connect**. New training and rollout sessions use that
endpoint; an active session keeps its existing connection.

To connect Studio to a local SpiralDB stack:

```console
SPIRALDB_ADBC_DRIVER=/absolute/path/to/libadbc_driver_spiraldb.dylib \
SPIRAL__DEV=1 \
dx serve
```

On first launch, Studio opens **Settings → SpiralDB** so you can enter the
Spiral API endpoint, SpFS endpoint, and project ID. The hosted services are the
defaults; for the local stack use `http://127.0.0.1:4279` and
`http://127.0.0.1:4295`. The backend remembers these three bootstrap pointers
in one private JSON file. Project-only switches apply immediately while Studio
is idle. A service-endpoint change is activated on the next Studio launch
because the ADBC driver selects both endpoints when it starts.
`--spiral-project` or `SPIRAL_PROJECT_ID` may still provide an initial project
override.
`SPIRAL_ROBOTICS_BACKEND_STATE` overrides the JSON file path for development
and deployment isolation.

Studio currently has no product simulation mode. Use `--no-cameras` only when
camera discovery is intentionally unavailable; configured robot connections
remain physical. Settings may switch the SpiralDB project only while robots and
project activity are stopped. The namespace remains fixed for the server
lifetime; project-owned training and video preferences remain inside Spiral.
