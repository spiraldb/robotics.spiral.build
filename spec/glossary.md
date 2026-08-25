# Glossary

- **Spiral Robotics** is the complete project.
- **Studio** is the browser interface.
- **Studio backend** is the consequential process that owns hardware, safety,
  data access, and durable backend actions.
- **compute runtime** is a selectable local or remote process that performs
  policy training and inference.
- **robot instance** is one physical arm.
- **leader** is the arm moved by the operator during teleoperation.
- **follower** is the arm commanded by teleoperation, replay, or rollout.
- **joint** is one named motor axis in a robot instance.
- **calibration** maps raw motor positions to normalized positions.
- **Hold** means torque is enabled and the follower maintains or accepts a
  bounded position.
- **Release** means torque is disabled and joint controls are read-only.
- **Teleop** means leader motion commands the compatible follower.
- **dataset** is an ordered collection of episodes used for inspection or
  training.
- **episode** is one kept multi-stream trajectory, recorded by Studio or
  ingested from an external source.
- **producer** is a path that creates episodes in the project: recording with
  connected arms, or ingestion.
- **consumer** is a use of stored data: query, curation, training, rollout,
  replay, or export.
- **ingestion** is the backend-owned import of externally produced data, such
  as LeRobot datasets or MCAP files, into the project.
- **stream** is one natural sequence in an episode, such as joint samples or one
  camera.
- **Timeline** is the shared episode clock and its user control.
- **playhead** is the selected time on the Timeline.
- **panel** is one registered content surface in Studio.
- **tile** is a tab group in a panel layout tree.
- **checkpoint** is a durable policy artifact and its data/model contract.
- **rollout** is one bounded execution of a checkpoint on the follower.
- **project** is the durable relational owner of robots, datasets, training
  runs, and rollout runs.
