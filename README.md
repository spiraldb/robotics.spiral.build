# Spiral Robotics

Spiral Robotics is a Rust-native robotics toolkit and browser studio inspired by
[LeRobot](https://github.com/huggingface/lerobot) and
[Rerun](https://github.com/rerun-io/rerun). It sets up, calibrates, and operates
SO-ARM101 arms; records synchronized robot and camera data; publishes and queries
episodes through SpiralDB; trains vision-conditioned ACT policies; and safely
executes trained policies through the follower's bounded rollout path.

Completed recordings publish to the configured SpiralDB destination
automatically.

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

Start the policy compute runtime separately (on the same machine or a remote
compute host). It owns PyTorch for both training and rollout inference:

```console
cargo run -p spiral-robotics-training-runtime -- --bind 0.0.0.0:50051
```

In Studio, open **Settings → Training**, enter the runtime's `http://` endpoint,
and choose **Save and connect**. New training and rollout sessions use that
endpoint; an active session keeps its existing connection.

To connect Studio to a local SpiralDB stack:

```console
SPIRALDB_ADBC_DRIVER=/absolute/path/to/libadbc_driver_spiraldb.dylib \
SPIRAL__SERVER__URL=http://127.0.0.1:4279 \
SPIRAL__SPFS__URL=http://127.0.0.1:4295 \
SPIRAL__DEV=1 \
dx serve
```

To try the interface without hardware, pass server arguments through DX:

```console
dx serve --args "--simulate --no-cameras"
```
