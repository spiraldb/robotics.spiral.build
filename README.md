# rerobot

`rerobot` is a Rust-native robotics toolkit inspired by
[LeRobot](https://github.com/huggingface/lerobot). It sets up, calibrates, and
drives SO-ARM101 arms — from a browser or from the terminal — without requiring
Python.

One arm is one robot instance is one JSON file. That file holds the arm's
serial port, its cameras, its motors, and its calibration; there are no
sidecars and no hidden state directories.

## Layout

| Crate | What it is |
| --- | --- |
| `rerobot` | Hardware: the Feetech motor bus, camera capture, serial discovery, and the three CLIs |
| `crates/rerobot-core` | Hardware-free domain types shared by everything: calibration, the SO-101 joint model, and the studio's wire format |
| `crates/rerobot-studio` | The web studio's backend: an HTTP server that owns the arms and the cameras |
| `crates/rerobot-ui` | The web studio's interface, a Dioxus app compiled to WebAssembly |

## Development

Enter the pinned Nix development environment, which provides the Rust
toolchain, `wasm-pack`, the Tailwind CLI, and `dx`:

```console
nix develop path:.
```

Build and test the workspace:

```console
cargo build --bins
cargo test --all-targets
```

The flake provides a pinned development shell for Apple Silicon macOS and
x86-64/AArch64 Linux. Cargo builds remain available on other platforms
supported by the Rust dependencies.

## Web studio

Build the interface once, then start the server:

```console
./scripts/build-ui.sh
```

```console
cargo run --bin rerobot-studio
```

The studio opens on <http://127.0.0.1:8080> and reads and writes robot files in
`./robots`. Pass `--workspace PATH` to keep them somewhere else, `--bind
ADDR:PORT` to serve on a different address, and `--no-cameras` to skip camera
discovery entirely.

To try the interface without hardware, run it against emulated motors:

```console
cargo run --bin rerobot-studio -- --simulate
```

Simulated arms drift slowly through their travel, so calibration sweeps and
teleoperation both work end to end with nothing plugged in.

### What the studio does

**Setup** creates robot instances and prepares them. Assign a serial port from
the detected list, or use *Identify by unplugging*: the studio records the
ports, you pull the arm's USB cable, and it reports the one that disappeared.
Assign cameras by role — `wrist`, `top`, whatever you like. Then calibrate: the
studio releases torque, you move every joint to both of its limits while a bar
per joint fills in, and saving writes the measured ranges into the robot's file.

**Control** draws each arm live from its measured joint angles — a side view of
the shoulder, elbow, and wrist chain plus a plan view of the base rotation —
and offers three modes:

- **Idle** reads the motors but never drives them.
- **Manual** commands one arm's joints from sliders.
- **Teleoperation** mirrors a hand-guided leader arm onto a follower arm,
  mapping through each arm's own calibration so two differently built arms
  still agree on what "halfway" means.

**Cameras** previews every detected camera as an MJPEG stream. Streams start
when a preview is on screen and release the device shortly after it leaves.

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

- Torque is never enabled implicitly, and enabling it copies each motor's
  present position into its goal register first, so an arm cannot snap to a
  target left over from an earlier session.
- Joints can only be commanded in normalized units, which are meaningless
  without a calibration — so an uncalibrated arm cannot be driven at all, and a
  command can never leave the travel that was actually measured.
- Commanded positions ramp toward their target at a bounded rate
  (`--max-step`, in encoder counts per control cycle), so neither a slider jump
  nor a teleoperation transient can throw an arm across its workspace.
- If an arm's motor bus stops responding, it is dropped and any mode that
  depended on it stops with the reason attached.

Releasing torque lets an arm fall under its own weight. Support it first.

### Developing the interface

`scripts/build-ui.sh --dev` skips optimization for a much faster rebuild. The
server reads the bundle from disk on every request, so a rebuild plus a browser
reload is enough — the server does not need restarting.

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
