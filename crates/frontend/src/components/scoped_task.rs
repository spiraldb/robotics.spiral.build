//! Tasks that belong to a scope other than the one that starts them.
//!
//! A command often has to outlive the view that started it — a save, a model
//! call, a frame decode — and still stop with the part of the page it writes
//! to. Dioxus's `Runtime::spawn(scope, task)` looks like that but is not: it
//! never records the task on the scope, so when the scope unmounts the task
//! keeps running, wakes after its next await, reads a signal that was dropped
//! with the scope, and panics — which halts every task on the page.
use dioxus::dioxus_core::{Runtime, ScopeId, Task};
use dioxus::prelude::spawn;
use std::future::Future;

/// Spawns futures owned by a chosen scope.
pub(crate) struct ScopedTask;

impl ScopedTask {
    /// Run `task` as `scope`'s own, so it is dropped when `scope` is.
    pub(crate) fn spawn(scope: ScopeId, task: impl Future<Output = ()> + 'static) -> Task {
        Runtime::current().in_scope(scope, || spawn(task))
    }
}
