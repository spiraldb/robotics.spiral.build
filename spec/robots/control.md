# Live robot control

## Outcome

Live control lets the operator inspect and move configured arms while the
backend owns all physical consequences.

## User flow

1. Studio shows connected arms and selects one robot and one motor by default.
2. The operator chooses Hold, Release, or Teleop.
3. Studio shows joint state and the camera panels owned by the selected robot.
4. The backend admits one control state and applies bounded hardware effects.

## Visible behavior

- **CONTROL-001** Live must show connection, calibration, motor, camera, and
  control state for each robot.
- **CONTROL-002** Selecting a robot must make its state and controls active.
- **CONTROL-003** When motor details are visible, one motor must be selected by
  default.
- **CONTROL-004** The operator-facing control choices are Hold, Release, and
  Teleop.
- **CONTROL-005** Studio must not show duplicate Hold or Release controls.
- **CONTROL-006** Teleop must map the leader to a compatible follower.
- **CONTROL-007** Camera assignment and status belong to the robot instance.
- **CONTROL-008** Camera imagery may use one independent panel for each assigned
  camera.
- **CONTROL-009** When two cameras are assigned, the default visualization must
  show both without a tab switch.
- **CONTROL-010** Live must not duplicate one camera feed in a generic camera
  section and a camera panel.

## Motion rules

- **CONTROL-011** Only one operation may own consequential motion at a time.
- **CONTROL-012** Hold must copy measured position into the goal before enabling
  torque.
- **CONTROL-013** Release must disable torque and make joint controls read-only.
- **CONTROL-014** Teleop must require a connected, calibrated, compatible leader
  and follower.
- **CONTROL-015** A new admitted motion state must replace the previous state.
- **CONTROL-016** Manual, replay, Timeline sync, and rollout commands must use
  the same bounded hardware path.
- **CONTROL-017** Normalized joint positions must be finite and inside the
  closed interval `[-1, 1]`.
- **CONTROL-018** An unverified torque state must be treated as unsafe.
- **CONTROL-019** A non-responsive motor bus must stop dependent motion modes.
- **CONTROL-020** The normal robot control and recording loop should target
  60 Hz.

## Related features

- Record starts Teleop through [Recording](../recording/README.md).
- Recorded actions move the follower through
  [Timeline and replay](../data/timeline-and-replay.md).
- A checkpoint receives temporary motion authority through
  [Rollout](../rollout/README.md).

