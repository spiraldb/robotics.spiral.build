# Robot setup and calibration

## User flow

1. The user opens Settings.
2. The user creates or imports one robot instance.
3. The user selects the SO-ARM101 role and serial port; Studio connects the arm
   as soon as it has a usable saved configuration.
4. The user assigns cameras while viewing a preview.
5. The user records the arm's calibration travel.
6. Studio validates and saves the complete configuration.

## Requirements

- **SETUP-001** Robot setup must live in Settings, not in the routine Live
  workspace.
- **SETUP-002** The user must be able to select the robot type, port, and camera
  assignments.
- **SETUP-003** Selecting a candidate camera must show its live preview.
- **SETUP-004** Camera identity changes after replug or reboot must be reported
  as an unavailable assignment, not a permanently blank image.
- **SETUP-005** Missing macOS camera permission must produce an explicit error.
- **SETUP-006** Calibration must collect the observed travel for the exact motor
  set.
- **SETUP-007** A calibration must bind exact motor names, bus IDs, and
  directions.
- **SETUP-008** A connection or motion command must fail when calibration does
  not match the current motor set.
- **SETUP-009** The user must be able to export one complete robot configuration
  as JSON.
- **SETUP-010** Importing a robot JSON document must create a new robot and must
  not overwrite an existing robot.
- **SETUP-011** Reapply calibration must update recorded positions only through
  an explicit, validated operation.
- **SETUP-012** A successful camera assignment must update the active Live
  camera panels without a browser refresh or backend restart.
- **SETUP-013** Creating a robot with a port, importing a robot configuration,
  or assigning the first port to a newly created robot must automatically try
  to connect it. A missing or unavailable bus must leave the durable robot in
  place and expose its disconnected or error state for recovery.

## Related features

- [Live control](control.md) uses the connected and calibrated instance.
- [Recording](../recording/README.md) captures the configured cameras and arms.
- [Rollout](../rollout/README.md) accepts only a compatible calibrated follower.
