1. we're building a rust-based robotics system very similar to lerobot.
2. use nix flake to setup dev env.
3. try not to use python dependencies.
4. use any rust crate as you see fit.

### Binaries

- `rerobot-find-port`
- `rerobot-find-cameras`
- `rerobot-calibrate`

Unlike lerobot, we should write everything about this robot in a single file, in json or yaml format.
For example, the port, cameras, the calibration, etc, should be written in a single file.
