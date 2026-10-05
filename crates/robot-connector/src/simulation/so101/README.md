# SO-101 model

The demo bench's arms are drawn with TheRobotStudio's SO-101 description,
from [`Simulation/SO101`](https://github.com/TheRobotStudio/SO-ARM100/tree/5f6d2b876a53a4872e405b991dd925556c9e38a4/Simulation/SO101)
at commit `5f6d2b87`, under the Apache License 2.0 in `LICENSE`.

Changes from the originals:

- `so101_follower.urdf` is `so101_new_calib.urdf` and `so101_leader.urdf` is
  `so101_leader_new_calib.urdf`, with the `3d_printed` material renamed
  `accent` so Studio paints each arm in its own colour.
- Every mesh in `assets/` was simplified to about 3.5% of its triangles with
  MeshLab's quadric edge collapse (pymeshlab, minimum 240 faces), from 19 MB to
  660 KB, so the model is quick to send to a browser.
