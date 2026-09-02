//! Execution bridge from Dioxus's local server-function pool to the backend runtime.

use std::future::Future;

/// Handle captured from the backend's multi-threaded Tokio runtime.
#[derive(Clone)]
pub(crate) struct MainRuntime {
    handle: tokio::runtime::Handle,
}

impl MainRuntime {
    pub(crate) fn current() -> Self {
        Self {
            handle: tokio::runtime::Handle::current(),
        }
    }

    pub(crate) async fn execute<Output>(
        &self,
        work: impl Future<Output = Output> + Send + 'static,
    ) -> Result<Output, MainRuntimeFailure>
    where
        Output: Send + 'static,
    {
        self.handle.spawn(work).await.map_err(MainRuntimeFailure)
    }
}

/// Failure to finish a command task on the backend runtime.
#[derive(Debug)]
pub(crate) struct MainRuntimeFailure(tokio::task::JoinError);

impl std::fmt::Display for MainRuntimeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "main runtime command task failed: {}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn work_from_a_current_thread_runtime_uses_the_captured_runtime() {
        let main_runtime = MainRuntime::current();
        let value = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("the test current-thread runtime should build")
                .block_on(async move {
                    main_runtime
                        .execute(async { tokio::task::block_in_place(|| 42) })
                        .await
                        .expect("the task should finish on the captured multi-thread runtime")
                })
        })
        .join()
        .expect("the current-thread runtime should not panic");

        assert_eq!(value, 42);
    }
}
