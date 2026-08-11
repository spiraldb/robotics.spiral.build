# Spiral Robotics

Spiral Robotics is a robotics development platform inspired by
[LeRobot](https://github.com/huggingface/lerobot) and [Rerun](https://github.com/rerun-io/rerun).

Unlike LeRobot and Rerun, Spiral Robotics is a one-stop-shop for the end-to-end robotics development pipeline, meaning that you can manupulate physical robots, collect data, train models, and deploy models all in one place.

SpiralDB (by design) is the only storage layer for Spiral Robotics, this means that all the telemetry, training epsiodes, and model checkpoints are stored in SpiralDB.

## Development

The easiest way to setup dev env is to use [nix](https://nixos.org/download.html).

```console
nix develop .
```

Start the studio by:
```
cd crates/spiral-robotics-studio
dx serve --release
```

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
