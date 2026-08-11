# rerobot

`rerobot` is a Rust-native robotics toolkit inspired by
[LeRobot](https://github.com/huggingface/lerobot). It sets up, calibrates, and
drives SO-ARM101 arms — from a browser or from the terminal — without requiring
operators to manage Python. Rust owns hardware, data, supervision, and the UI;
the Nix shell supplies one isolated PyTorch worker for policy training.

One arm is one robot instance is one JSON file. That file holds the arm's
serial port, its cameras, its motors, and its calibration; there are no
sidecars and no hidden state directories.

## Layout

| Crate | What it is |
| --- | --- |
| `rerobot` | Hardware: the Feetech motor bus, camera capture, serial discovery, and the three CLIs |
| `crates/rerobot-core` | Hardware-free domain types shared by everything: calibration, the SO-101 joint model, the robot description it is drawn from, and the studio's wire format |
| `crates/rerobot-studio` | One Dioxus fullstack app: the WebAssembly interface, server rendering, HTTP API, and hardware ownership |

## Development

Enter the pinned Nix development environment, which provides the Rust
toolchain and the whole web toolchain — `dx`, the Tailwind CLI,
`wasm-bindgen`, and `wasm-opt` — plus the pinned PyTorch/Arrow training
interpreter:

```console
nix develop .
```

Build and test the workspace:

```console
cargo build --bins
cargo test --workspace --features rerobot-studio/server
```

The flake provides a pinned development shell for Apple Silicon macOS and
x86-64/AArch64 Linux. Cargo builds remain available on other platforms
supported by the Rust dependencies.

## Web studio

Run the client and server as one Dioxus fullstack app:

```console
cd crates/rerobot-studio
dx serve
```

DX owns the whole development loop: it compiles the same package with `web` and
`server` features, runs Tailwind from the input declared in `Dioxus.toml`, and
restarts the right side when a source changes. The server
mounts Dioxus's router, which serves the generated assets and server-rendered
document and then hydrates it in the browser. The existing `/api` REST and event
stream remain the explicit boundary used by the UI and other clients.

Create a self-contained release bundle with the server executable beside its
`public` directory:

```console
dx bundle --release --out-dir ../../outputs/rerobot-studio
../../outputs/rerobot-studio/server
```

The dev shell sets `NO_DOWNLOADS`, so DX, Tailwind, wasm-bindgen, and wasm-opt all
come from the versions pinned by the flake rather than being fetched at build
time.

The studio opens on <http://127.0.0.1:8080> and reads and writes robot files in
`./robots`. Use DX's `--addr` and `--port` options during development. A bundled
server also accepts `--workspace PATH`, `--episodes PATH`, `--bind ADDR:PORT`,
and `--no-cameras`.

The studio is not the only writer of those files, so it re-reads the directory
whenever anything in it changes: a robot calibrated with `rerobot-calibrate`, or
a file copied in by hand, shows up without restarting the server. A file that
parses but does not describe a usable robot — two motors sharing a bus ID, say —
is listed with the reason attached rather than quietly ignored.

To try the interface without hardware, pass server arguments through DX:

```console
dx serve --args "--simulate --no-cameras"
```

Simulated arms drift slowly through their travel, so calibration sweeps and
teleoperation both work end to end with nothing plugged in.

To connect Studio to a local SpiralDB stack, point both control-plane and SpFS
clients at it when the backend starts; the ADBC library path alone is not
enough:

```console
SPIRALDB_ADBC_DRIVER=/absolute/path/to/libadbc_driver_spiraldb.dylib \
SPIRAL__SERVER__URL=http://127.0.0.1:4279 \
SPIRAL__SPFS__URL=http://127.0.0.1:4295 \
SPIRAL__DEV=1 \
dx serve
```

Then open **Settings**, enable SpiralDB, select the service catalog, and enter
the exact project and dataset. **Test connection** checks both access to that
project and readiness of its managed catalog; a local `SELECT 1` alone is not a
useful connectivity test. Use a dataset name reserved for rerobot: the first
publication creates that root resource, its relational child tables, and one
immutable managed MP4 Dataset per completed episode.

For a dataset named `rerobot_v3`, a completed `episode_000017` is queryable as
relational data and through Spiral's native MP4 index:

```sql
SELECT episode_id, frames FROM rerobot_v3.episodes;

SELECT v.episode_id, v.stream_id, tracks.*
FROM rerobot_v3_episode_000017_video_sources AS v
CROSS JOIN LATERAL spql.mp4.tracks(v.source) AS tracks;

SELECT COUNT(*) FROM rerobot_v3.video_episode_000017.assets;
```

### What the studio does

The studio is a two-region workbench. The left region owns the task and switches
between **Live**, **Data**, and **Training**; the right region owns visualization. Drag the
separator to change the default 40/60 split. The ratio persists locally, and the
Timeline stays mounted below every visualization so changing tools never loses
the current playhead.

**Live** is where an operator works with physical hardware. Select an arm inside
**Motors & cameras** to see its joints, health, and assigned camera feeds. Hold,
Release, and Teleop are mutually exclusive control modes. Recording always uses
Teleop and supports pause, resume, stop, and discard without changing the
episode identity. Setup in this mode owns ports, camera assignment, and
calibration.

**Data** owns a dataset containing many episodes. Its Episodes tool selects,
replays, or deletes a take; its Query tool runs guarded read-only SQL against the
configured SpiralDB dataset. Query results replace the upper visualization
rather than appearing beside an unrelated live camera.

**Training** starts a locally supervised PyTorch process from a guarded,
read-only query against a hosted Spiral project. Training deliberately rejects
`spiral:memory:` because that catalog is connection-local: the dedicated loader
could not see data created by another Studio connection. The default
state-based ACT run reads ordered
`episode_id`, `stream_id`, `sample_index`, state, and action columns; sequence
boundaries survive Arrow record-batch boundaries and whole episodes are held
out for validation. Rust opens Spiral through ADBC, validates the schema, and
feeds a bounded queue of framed Arrow IPC batches to Python. Typed Arrow numeric
vectors use PyArrow's direct tensor path when compatible; the current JSON
joint-vector columns are normalized into typed float32 Arrow by the bounded
Rust loader, where that compatibility copy can overlap model compute.
Python owns only tensor conversion, the ACT model, optimizer, model-side
validation, metrics, and atomic local checkpoint files. Rust validates those
files and bulk-ingests each configured checkpoint — plus the final checkpoint —
into `<dataset>.training_checkpoints`. Run state reaches the browser as one
initial snapshot followed by bounded incremental updates.

The worker is launched automatically inside `nix develop`. To use a separately
managed CUDA interpreter, pass `--training-python /absolute/path/to/python`;
the Python environment needs only `torch`, `pyarrow`, and `numpy`. Spiral
credentials, URIs, and ADBC drivers are never passed to Python.

**The visualization region** shows the live 3D scene and selected live camera in
Live mode, and the recorded pose/camera or query results in Data mode. The 3D
view supports orbit, pan, zoom, selection, and framing. Camera status reflects
real image load state and reports an actionable error instead of showing a
permanently green but blank frame. Training replaces the episode clock with one
shared optimizer-step axis across learning curves, resource lanes, checkpoint
events, and the measured Spiral-to-tensor data path.

**The Timeline** is shared by both modes. During capture it follows new camera
and joint samples. A paused or completed take can be scrubbed to time-travel the
3D pose and recorded camera. Moving the physical follower is an explicit,
safety-gated option and never happens merely by moving the preview playhead.

### The robot description

The arm the viewport draws is read from
[`crates/rerobot-core/assets/so101.urdf`](crates/rerobot-core/assets/so101.urdf),
a standard URDF built from primitive shapes so it needs no mesh files. The
parser behind it is general — links, joints, origins, axes, limits, and
materials — and the studio's forward kinematics walks whatever tree it finds.
Only the shipped description is SO-101 specific.

Its joint names are the motor names, so a measured angle drives the model with
no translation table. Two things follow from keeping the description as the
single source of truth: the link lengths exist in exactly one place, and the
reach and height readouts beside the sliders are solved from the same file that
draws the picture rather than from a second copy of the geometry.

Cameras are recorded by the identifier the operating system reports —
`/dev/videoN` on Linux, the capture index on macOS and Windows — which is the
same scheme `rerobot-find-cameras` prints. Those identifiers are not guaranteed
to survive a replug or a reboot, so if a preview goes blank after moving a
cable, rescan and reassign the camera.

On macOS the studio must be allowed to use the camera. Discovery and stream
setup both succeed without that permission and simply never deliver a frame, so
the studio checks at startup and says so in the warnings strip rather than
leaving an unexplained black preview.

### Safety

The rules are enforced by the server, not by the interface, so they hold for
anything that talks to the API:

- Torque is enabled without being asked for in exactly one place: starting a
  recording releases the leader and holds the follower, because teleoperation
  does nothing at all otherwise. Everywhere else it takes a deliberate click.
  Either way, enabling it copies each motor's present position into its goal
  register first, so an arm cannot snap to a target left over from an earlier
  session. Stopping the recording ends teleoperation but does not release the
  follower — torque is what keeps an arm from falling, so dropping it is always
  the operator's call.
- Joints can only be commanded in normalized units, which are meaningless
  without a calibration — so an uncalibrated arm cannot be driven at all, and a
  command can never leave the travel that was actually measured.
- Commanded positions ramp toward their target at a bounded rate. Manual moves
  use `--max-step`; the follower uses the faster but still bounded
  `--teleop-max-step` so ordinary hand motion does not build up a large pose
  error. Both are measured in encoder counts per control cycle.
- If an arm's motor bus stops responding, it is dropped and any mode that
  depended on it stops with the reason attached.

Releasing torque lets an arm fall under its own weight. Support it first.

### Control rate

`Control 60/60 Hz` in the header is the measured and requested rate of the
hardware loop that reads motor positions, maps teleoperation, and writes new
goals. It is not the screen refresh rate; browser telemetry is delivered at up
to 20 Hz. The default hardware target is 60 Hz. A sustained measurement below
the target means the serial transactions cannot finish within the 16.7 ms
cycle and may add follow latency. The studio marks that readout as a warning
below 90% of target and shows the follower's largest target error as
`Follow Δ`.

### Developing the interface

Use the fullstack `dx serve` command above while developing. DX watches the
client, server, assets, and Tailwind input together, then hot reloads or restarts
the appropriate half. `dx bundle` is for a deployable server/public pair, not a
second development lifecycle.

## Command-line tools

The three CLIs cover the same ground as the studio's setup screens, for
scripting and for machines without a browser.

## Find a serial port

Run the interactive workflow, then unplug exactly one motor bus when prompted:

```console
cargo run --bin rerobot-find-port
```

To inspect ports without unplugging hardware:

```console
cargo run --bin rerobot-find-port -- --list
```

## Find cameras

Enumerate native webcams and save one recent PNG from each camera under
`outputs/captured_images`:

```console
cargo run --bin rerobot-find-cameras
```

To probe the devices and show their selected stream profiles without running
the timed capture period or writing images:

```console
cargo run --bin rerobot-find-cameras -- --list-only
```

Use `--help` for the full command options. The camera utility currently covers
OS-native, OpenCV-compatible webcams; dedicated Intel RealSense discovery is
not yet included.

## Calibrate motors

`rerobot-calibrate` keeps robot identity, serial port, cameras, motor
definitions, and calibration together in one versioned JSON file. Live
calibration currently supports Feetech protocol-0 motors such as the STS3215
used by SO-100/SO-101 arms.

If `robot.json` already contains its serial port and motors, calibration is one
command:

```console
cargo run --bin rerobot-calibrate -- --config robot.json
```

The command connects to every configured motor, disables and verifies torque,
then displays each motor's current position and automatically accumulated
minimum and maximum in a live table. Support the arm, move every joint through
its complete physical range, and press Enter. The result is validated and
saved immediately; torque remains disabled when the command exits.

You can also create the basic robot file from command-line options on the first
run:

```console
cargo run --bin rerobot-calibrate -- \
  --config robot.json \
  --robot-id my_follower \
  --robot-type so101_follower \
  --port /dev/ttyACM0 \
  --motor shoulder_pan=1 \
  --motor shoulder_lift=2:reversed \
  --motor elbow_flex=3 \
  --motor wrist_flex=4 \
  --motor wrist_roll=5 \
  --motor gripper=6
```

Motor definitions use `NAME=ID[:DIRECTION]`, where direction is `normal` or
`reversed`. Repeating `--motor` updates those entries while preserving omitted
motors and protocol-specific fields already present in the file. Motors without
an explicit `model` field are treated as STS3215-compatible for live
calibration. The serial baud rate defaults to `1000000`.

The stored `range_min` and `range_max` are the observed raw encoder extrema.
The centered raw format also needs a center, which the live workflow derives as
the midpoint between those extrema so there is no separate centering step.

For testing, migration, or another motor-bus adapter, skip the hardware
connection with `--from-json`. The input may contain either precomputed ranges
or a center reading plus sweep samples. The example files demonstrate the
latter:

```console
cp examples/robot.json robot.json
cargo run --bin rerobot-calibrate -- \
  --config robot.json \
  --from-json examples/calibration-input.json
```

The input can instead contain already reduced ranges:

```json
{
  "motors": {
    "shoulder_pan": {
      "center": 2038,
      "range_min": 700,
      "range_max": 3400
    }
  }
}
```

Every configured motor must appear exactly once in direct readings, the center
map, and every sweep sample. The stored calibration retains the observed raw
values in the `unhomed_raw` coordinate space; live calibration does not modify
homing offsets or position-limit registers on the motors.

Use `--dry-run` to print the merged document without writing it. Replacing an
existing calibration requires `--force` in non-interactive use; the live
workflow asks before replacing it. Updated motor and calibration data are
validated before the same robot file is atomically replaced; unrelated fields
are retained and no calibration sidecar is created.
