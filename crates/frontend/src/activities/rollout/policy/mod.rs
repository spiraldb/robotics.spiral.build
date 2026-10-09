//! The Policy control source: a trained checkpoint drives the follower for a set
//! time. The robot connector runs the policy; Studio picks the checkpoint,
//! writes it there, and watches the run.

mod checkpoints;
mod client;
mod control;
mod readiness;
mod section;

pub(super) use client::PolicyClient;
pub(super) use control::PolicyControl;
pub(super) use readiness::PolicyReadiness;
pub(super) use section::PolicySection;
