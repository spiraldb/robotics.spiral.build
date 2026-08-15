//! Small task-state helpers shared by interactive frontend flows.

use std::future::Future;

use dioxus::prelude::{ReadableExt, Signal, WritableExt};

/// Run one asynchronous operation for a boolean pending signal.
///
/// The guard and reset live together so every caller gets the same duplicate
/// submission protection, including paths that return an error.
pub(crate) async fn single_flight<T>(
    mut pending: Signal<bool>,
    task: impl Future<Output = T>,
) -> Option<T> {
    if pending.peek().to_owned() {
        return None;
    }
    pending.set(true);
    let result = task.await;
    pending.set(false);
    Some(result)
}
