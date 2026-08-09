# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

The primary user is an **ML engineer building robot policies**. Their real job
is training and evaluating models; rerobot is the hardware layer they need to
stop fighting. They judge it on how quickly and reliably they can get from
"arms on the bench" to "usable episodes", and on whether it stays out of the
way once a policy is running.

They work with a pair of SO-ARM101 arms — one leader, one follower — wired to a
machine at the bench, with one or more webcams pointed at the workspace.

Secondary, and not yet a design target: other SO-101 owners who will arrive
when the project is opened. See **Distribution** below.

## Product Purpose

rerobot sets up, calibrates, and drives SO-ARM101 arms — from a browser or from
the terminal — without requiring Python. It exists to make the LeRobot
workflow available to people who do not want a Python stack between them and
their motors.

The durable scope is the **full LeRobot loop**: episode recording, dataset
export, and policy playback/inference are all in scope. Recording, export, and
trajectory replay exist today; policy inference does not. Future work should
assume a record → train → replay lifecycle exists, even where it is not yet
built.

Success is an ML engineer collecting a usable dataset and replaying a policy on
real arms without writing glue code and without installing Python.

## Positioning

- **Rust-native, no Python.** The mechanism a neighboring tool cannot
  truthfully copy without rewriting itself: the motor bus, camera capture,
  calibration, kinematics, and the web interface are all Rust, shipped as
  binaries plus a WASM bundle.
- **One arm is one robot instance is one JSON file.** Serial port, cameras,
  motors, and calibration live in a single versioned file. No sidecars, no
  hidden state directory, no calibration cache to invalidate. Any other tool
  can read or write that file, and the studio notices when one does.
- **One take is one file that says everything about itself.** The same property,
  applied to recordings: an episode's first line names its arms and their joints,
  its middle lines are the samples, its last line carries the totals and the
  operator's judgement. No index, no database, no manifest to fall out of step —
  a reader needs the file and nothing else, and the studio re-reads the directory
  rather than assuming it is the only writer.
- **The robot description is the single source of truth.** Geometry, joint
  limits, and the drawn model all come from a URDF file rather than from
  drawing code, so link lengths and reach exist in exactly one place.

## Operating Context

The deployment shape is a **split**: a low-end device is physically wired to
the arms and runs the backend; the client connects to it over the network and
performs the expensive work.

- **The backend device is cheap and close to the hardware.** It owns the
  motor bus, the cameras, and the real-time control loop. It is not a
  workstation, and work must not be moved onto it casually.
- **The client is remote and does the heavy lifting** — 3D rendering, video
  decode, and anything else costly. It is a web app today, and "today" is
  load-bearing: other clients are anticipated.
- **The wire API is therefore a product boundary, not an implementation
  detail.** It is the contract between a constrained device and an
  interchangeable client, and it must survive both changing.
- Network latency, reconnection, and degraded-link behavior are real operating
  conditions, not edge cases. The interface already treats "reconnecting" as a
  first-class state in which controls are paused.

**Where inference runs is deliberately not fixed.** A policy may execute on the
backend device, on the client, or on a separate inference server. No part of
the system may assume one of those, and the boundary has to carry actions in
any of those arrangements.

The physical scene: an operator supports the arm by hand during calibration and
teleoperation, within reach of the hardware, with a browser open beside it.
Releasing torque lets an arm fall under its own weight.

## Capabilities and Constraints

**Shipped today**

- Three CLIs — `rerobot-find-port`, `rerobot-find-cameras`, `rerobot-calibrate`
  — covering the same ground as the studio's setup screens, for scripting and
  for machines without a browser.
- A single-screen web studio: robot instances, serial-port assignment
  (including *Identify* by unplugging), camera assignment by role with live
  preview, and a guided calibration sweep that writes measured ranges into the
  robot's file. On startup the studio tries each usable saved robot once, while
  leaving an unavailable arm disconnected with its error visible.
- Four modes: **Idle** (read only), **Manual** (command one arm's joints),
  **Teleoperation** (mirror a hand-guided leader onto a follower, mapped
  through each arm's own calibration), and **Replay** (play a recorded take back
  onto one arm, ramping it to the take's first pose before playing).
- A live 3D scene drawn from URDF, and MJPEG camera previews.
- **Episode recording and dataset interchange**, on a session timeline across the foot
  of the studio. Recording samples every connected arm at the control rate; one
  take is one self-describing file in the episode directory (`--episodes`,
  default `./episodes`), carrying its arms and joints, normalized positions, raw
  encoder counts, and per-frame tracking error. Each take can be selected,
  replayed on the follower, or deleted from its episode card. Download produces
  one LeRobot v3 ZIP containing low-dimensional state/action Parquet data and
  metadata; LeRobot v2/v3 ZIPs can be imported as local replayable episodes.
  Camera streams are preview-only and are not represented as recorded video.
  Recording is deliberately not a mode: a take is captured *during*
  teleoperation, manual control, or an idle arm moved by hand.
- A `--simulate` mode with emulated motors, so calibration, teleoperation,
  recording, and replay all work end to end with no hardware attached.

**Not yet built, but in scope:** policy inference — running a trained model to
produce actions. Replaying a recorded trajectory exists; deciding one does not.

**Constraints**

- **Hardware:** SO-ARM101 only, leader and follower, recorded in `notes.md` as
  a current tradeoff rather than a permanent limit. Live calibration supports
  Feetech protocol-0 motors such as the STS3215.
- **No Python dependencies.** This is a defining constraint, not a preference.
- **Nix flake** provides the dev environment, including the whole web toolchain
  (`dx`, Tailwind CLI, `wasm-bindgen`, `wasm-opt`).
- **Safety rules are enforced by the server, not the interface,** so they hold
  for anything that talks to the API: torque is only enabled deliberately —
  starting teleoperation and starting a replay are the two exceptions, and both
  copy present position into goal first, so the arm holds where it stands rather
  than snapping; joints can only be commanded in normalized units, so an
  uncalibrated arm cannot be driven at all, and a take recorded from an
  uncalibrated arm cannot be turned back into motion; commanded positions ramp at
  a bounded rate, and a replay walks to its take's first pose on the gentle manual
  ramp before playing, giving up with a reason if a joint is obstructed; a
  non-responsive motor bus drops the arm and stops the mode with a reason
  attached, and ends any take in progress rather than recording a hole in it.
- **Camera identity is not stable.** Cameras are recorded by the identifier the
  OS reports, which may not survive a replug or reboot. On macOS the studio
  must be granted camera permission, and reports its absence rather than
  showing an unexplained black preview.
- The studio currently serves on `127.0.0.1:8080` by default, with `--bind` for
  other addresses — worth noting against the remote-client architecture above.

**Terminology:** *robot instance* (one arm), *leader* / *follower*, *joint*,
*calibration*, *travel*, *torque*, *mode*, *workspace* (the directory of robot
JSON files), *studio* (the web interface), *take* / *episode* (one recording, used
interchangeably — *take* in the interface, *episode* in file names and on the
wire), *Follow Δ* (the one name for the gap between where a joint is and where it
was told to be, in degrees, live in the inspector and per-take on the timeline).

## Brand Commitments

- The name is **rerobot**; the web interface is **rerobot studio**, styled
  lowercase.
- **`notes.md` is the single source of truth for conflicting changes, and must
  not be edited unless explicitly asked.** This is recorded in `AGENTS.md` and
  is binding. It holds the committed visual direction; this file does not
  restate or extend it.
- **Distribution:** public open-source eventually, but not on the current
  timeline. Keep the code and configuration honest for strangers without
  building an outward-facing surface yet. Onboarding, docs beyond the README,
  and any project landing surface are deliberately out of scope for now.

## Evidence on Hand

Real, in-repo:

- `crates/rerobot-core/assets/so101.urdf` — the shipped robot description.
- `examples/robot.json`, `examples/calibration-input.json` — real config and
  calibration input.
- `robots/leader.json`, `robots/follower.json` — a working two-arm setup.
- A detailed `README.md` written by the maintainer.

Deliberately absent, and not to be fabricated: there are **no users, no
testimonials, no case studies, no benchmarks, no published performance
numbers, no pricing, and no deployment or endorsement claims.** The project has
not been released. Any future surface must not invent them.

## Product Principles

1. **The wire API is the product, not the web app.** The client is
   interchangeable and currently happens to be a browser. Anything that only
   works because the client is a Dioxus app is a mistake.
2. **Nothing heavy moves onto the arm-side device.** It is wired to the motors
   and it is low-end; the expensive work belongs on the client or elsewhere.
3. **Never assume where inference runs.** Backend, client, and separate
   inference server are all valid, and the design must hold in all three.
4. **One file holds the whole robot.** Anything worth knowing about an arm goes
   in its JSON file, and any tool may be the one that wrote it.
5. **Safety is enforced where it cannot be bypassed** — in the server, on
   behalf of every client, not in the interface of one of them.
