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
- We should be able to control the arms, joints.
- We should be able to choose different modes, like teleoperation, etc.
- On load, it should (try to) automatically connect to the robot.

### Setup flow
- User select the robot type, the port, and the cameras. When they select cameras, they should be able to see the camera feed in the web ui.
- Users can export a robot's complete JSON configuration and import one as a new robot instance; imports never overwrite an existing robot.

### UI design
- Design language: notion style.
- Try to make the ui rerun style, where you have a studio like layout. Don't make many tabs that user needs to switch between.
- Make ui compact and information dense.
- Camera feeds are properties of the robot instance, it should be in the properties panel, rather than the main ui.

### 3D viewer
- Because we know the robot is a pair of so-arm-101, we can visualize the robot in the web ui. 
- The robot visualization should be a 3D view, where you can drag/zoom/pan, just like any other 3D viewer.
- For a more future proof design, we should be able to read urdf file, and use it to visualize the robot. Current design will still need to read from urdf file, but the file is hard-coded to support only so-arm-101.

### Record and replay and export

- User often need to record many episodes. They may delete episodes, delete button should appear on the episode item.
- They should be able to replay the selected episodes on follower.
- They can export the recorded episodes download as a single (zip or whatever way to package it) file, ideally it should be in lerobot format.
- They should be able to import a lerobot format dataset, and replay any episode on the follower.

## Cli tools

- `spiral-robotics-find-port`
- `spiral-robotics-find-cameras`
- `spiral-robotics-calibrate`

Unlike lerobot, we should write everything about this robot in a single file, in json or yaml format.
For example, the port, cameras, the calibration, etc, should be written in a single file.
