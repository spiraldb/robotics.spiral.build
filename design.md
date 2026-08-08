1. we're building a rust-based robotics system very similar to lerobot.
2. use nix flake to setup dev env.
3. try not to use python dependencies.
4. use any rust crate as you see fit.

## Trade offs
- For now, we will only support so-arm-101, support both leader and follower.

## Main GUI app

- use dioxus for the main web ui, backend connect to the robot.
- in the web ui, we should be able to init a robot instance, assign the port, cameras, and do the calibration. Everything about this setup (robot) will be save in a single json file. Each arm is considered as a single robot instance. 
- We should be able to preview the camera feed in the web ui.
- Because we know the robot is a pair of so-arm-101, we can visualize the robot in the web ui. We should be able to control the arms, joints.
- We should be able to choose different modes, like teleoperation, etc.

### Binaries

- `rerobot-find-port`
- `rerobot-find-cameras`
- `rerobot-calibrate`

Unlike lerobot, we should write everything about this robot in a single file, in json or yaml format.
For example, the port, cameras, the calibration, etc, should be written in a single file.

