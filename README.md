# rerobot

`rerobot` is a Rust-native robotics toolkit inspired by
[LeRobot](https://github.com/huggingface/lerobot). The utilities map connected
motor controllers and cameras and record motor calibration without requiring
Python.

## Development

Enter the pinned Nix development environment:

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
