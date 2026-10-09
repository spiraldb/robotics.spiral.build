//! The Policy control source: a trained checkpoint to drive the follower for
//! a set time. Studio lists the checkpoints and keeps the operator's choice;
//! running one on a robot is not supported yet.

mod checkpoints;
mod control;
mod readiness;
mod section;

pub(super) use control::PolicyControl;
pub(super) use readiness::PolicyReadiness;
pub(super) use section::PolicySection;
