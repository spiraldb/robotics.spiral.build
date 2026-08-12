# Spiral Robotics

Spiral Robotics is a Rust-native robotics toolkit and browser studio inspired by
[LeRobot](https://github.com/huggingface/lerobot) and
[Rerun](https://github.com/rerun-io/rerun). It sets up, calibrates, and operates
SO-ARM101 arms; records synchronized robot and camera data; publishes and queries
episodes through SpiralDB; trains vision-conditioned ACT policies; and safely
replays recorded actions. Policy inference and robot rollout are not implemented
yet.

Completed recordings use a local recovery buffer before explicit publication to
SpiralDB. Hugging Face is available as an optional publishing mirror.

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

Vision ACT uses SpiralDB's native Python relation package (`to_torch()`). Until
that wheel is part of nixpkgs, point Studio at the Python environment from the
matching SpiralDB checkout:

```console
SPIRAL_ROBOTICS_TRAINING_PYTHON=/path/to/spiraldb/lang/python/spiral-engine/.venv/bin/python \
dx serve --release
```

That environment must contain PyTorch, PyArrow, NumPy, SafeTensors, and a
matching SpiralDB build with the video/image relation operators. Studio verifies a real
`to_torch()` conversion before enabling training. Spiral—not Python—selects,
decodes, resizes, and normalizes every requested camera frame.

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
