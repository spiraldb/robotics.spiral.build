//! The preview: what each measurement reads over the sample this session.
//!
//! A measurement previews by running its own step — the one its published
//! view chains — over the rows its parent's preview already holds, supplied
//! to Spiral as `VALUES` (`input`). Nothing above it is run again, so a
//! search or a grouping makes only its own text calls. What each episode
//! answered, or why it failed, is held for the session with what went into
//! it (`reads`). `pass` runs one node's preview as the curation's run, on a
//! run the backend holds (`held_run`), and restores it from the answers
//! stored with the dataset when the curation opens; `progress` says where it
//! stands: an episode read from rows that have changed since, or below a
//! node that has, is stale.
mod held_run;
mod input;
mod pass;
mod progress;
mod reads;

pub(super) use pass::{PreviewPass, PreviewPlan};
pub(super) use progress::PreviewProgress;
pub(super) use reads::{PreviewReads, PreviewRows, ReadInput};

#[cfg(test)]
mod tests;
