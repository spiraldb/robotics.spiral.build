# Settings

Settings contains configuration that is not part of routine Live operation.

## Robot setup

- **UX-SET-001** A user must be able to add one SO-ARM101 robot instance, select
  its serial port, assign cameras, preview each candidate camera, and calibrate
  its motors.
- **UX-SET-002** One robot instance must represent one arm.
- **UX-SET-003** Studio must attempt to reconnect configured robots when the
  backend starts.
- **UX-SET-004** Robot setup must not be mixed into the normal Live workbench.
- **UX-SET-005** A user must be able to export one complete robot configuration
  as JSON.
- **UX-SET-006** Importing a robot JSON document must create a new robot and must
  not overwrite an existing robot.
- **UX-SET-007** Calibration must collect the observed travel for the exact
  motor set and save one coherent calibration.
- **UX-SET-008** Reapply calibration must update recorded positions only through
  an explicit, validated operation.

## Services

- **UX-SET-009** Settings must configure and test the SpiralDB destination.
- **UX-SET-010** Settings must configure and test the selected compute runtime.
- **UX-SET-011** A runtime setting change must apply to new sessions and must
  not retarget an active training or rollout session.
- **UX-SET-012** Stored service credentials must remain in the backend and must
  not be returned to the browser.

Durable ownership is defined in [Persistence](../system/persistence.md).

