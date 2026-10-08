//! The preview: what each model node reads over the sample this session.
//!
//! A model node previews by reading its own entries — the ones its published
//! view folds into its columns — over the stored answers of one episode at a
//! time, bound as `$1`, after having the backend fill them. What each
//! episode answered, or why it failed, is held for the session under the
//! column the node stores its answers in (`reads`), so a node configured
//! otherwise has read nothing. `pass` runs one node's preview as the
//! curation's run, on a run the backend holds (`held_run`), and restores it
//! from the answers stored with the dataset when the curation opens;
//! `progress` says where it stands over the episodes that reach the node.
mod held_run;
mod pass;
mod progress;
mod reads;

pub(super) use pass::{PreviewPass, PreviewPlan};
pub(super) use progress::PreviewProgress;
pub(super) use reads::{PreviewReads, PreviewRows};

#[cfg(test)]
mod tests;
