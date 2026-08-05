# rerobot

`rerobot` is a Rust-native robotics toolkit inspired by
[LeRobot](https://github.com/huggingface/lerobot). The first utilities help map
connected motor controllers and cameras without requiring Python.

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

The flake also builds a package containing both binaries:

```console
nix build path:.
```

The flake supports Apple Silicon macOS and x86-64/AArch64 Linux. Cargo builds
remain available on other platforms supported by the Rust dependencies.

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

The same programs can be run directly through Nix:

```console
nix run path:.#rerobot-find-port -- --list
nix run path:.#rerobot-find-cameras -- --list-only
```
