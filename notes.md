1. we're building a rust-based robotics system very similar to lerobot.
2. use nix flake to setup dev env.
3. try not to use python dependencies.
4. use any rust crate as you see fit.
5. if you find a bug/inconsistency in upstream spiral, stop and ask, don't workaround it.

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


### SpiralDB

The table definition looks like this:

```sql
CREATE TABLE spiral_robotics.episodes (
    episode_id       VARCHAR NOT NULL,
    sample_hz        FLOAT NOT NULL,
    frame_count      UINT64 NOT NULL,
    duration_ns      UINT64 NOT NULL,
    discarded        BOOLEAN NOT NULL
) ENGINE 'arbor';

CREATE TABLE spiral_robotics.joint_samples (
    episode_id       VARCHAR NOT NULL,
    stream_id        VARCHAR NOT NULL,
    sample_index     UINT64 NOT NULL,
    episode_time_ns  UINT64 NOT NULL,
    state            vector(6) NOT NULL,
    action           vector(6),
    velocity         vector(6),
    effort           vector(6)
) ENGINE 'arbor';

CREATE TABLE spiral_robotics.video_sources (
    episode_id         VARCHAR NOT NULL,
    camera_role        VARCHAR NOT NULL,
    segment_index      UINT32 NOT NULL,
    timeline_start_ns  UINT64 NOT NULL,
    timeline_end_ns    UINT64 NOT NULL,
    source             blob NOT NULL,
    width              UINT32 NOT NULL,
    height             UINT32 NOT NULL,
    fps                 UINT32 NOT NULL
) ENGINE 'arbor';

CREATE DATASET spiral_robotics.videos
USING spql.mp4.dataset
FROM spiral_robotics.video_sources
WITH (
    blob = 'source',
    index_tier = 'dependency'
);

REFRESH DATASET spiral_robotics.videos;
```

We'd like to get the sample at episode 37, frame index 42.
```sql
SELECT
    j.state AS joint_position,
    spql.image.to_tensor(
        spql.video.decode(f.frame, 'ffmpeg')
    ) AS wrist_image

FROM spiral_robotics.joint_samples AS j

JOIN spiral_robotics.videos.assets AS v
  ON j.episode_id = v.episode_id,

spql.video.frames(
    v.media,
    spql.video.frame_indices([42])
) AS f

WHERE j.episode_id = '37'
  AND j.stream_id = 'follower'
  AND j.sample_index = 42
  AND v.camera_role = 'wrist'
  AND v.segment_index = 0;
```

It returns one row:
joint_position: [6]
wrist_image:    [height, width, 3]
Then:

```python
relation = db.sql(query)
batch = next(relation.to_torch())
```
