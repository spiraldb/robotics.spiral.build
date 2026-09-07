//! Backend task ownership for cancellable reads and admitted commands.

use std::future::Future;

/// Runs admitted commands to completion even when the HTTP caller disconnects.
#[derive(Clone)]
pub(crate) struct BackendTasks {
    handle: tokio::runtime::Handle,
}

impl BackendTasks {
    pub(crate) fn current() -> Self {
        Self {
            handle: tokio::runtime::Handle::current(),
        }
    }

    pub(crate) async fn execute<Output>(
        &self,
        work: impl Future<Output = Output> + Send + 'static,
    ) -> Result<Output, BackendTaskFailure>
    where
        Output: Send + 'static,
    {
        self.handle
            .spawn(work)
            .await
            .map_err(BackendTaskFailure::Task)
    }
    /// Reads have no durable consequence. Cancel their backend task when its
    /// HTTP future is dropped, and bound execution before the browser deadline.
    pub(crate) async fn read<Output>(
        &self,
        work: impl Future<Output = Output> + Send + 'static,
    ) -> Result<Output, BackendTaskFailure>
    where
        Output: Send + 'static,
    {
        let task = tokio_util::task::AbortOnDropHandle::new(self.handle.spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(8), work).await
        }));
        task.await
            .map_err(BackendTaskFailure::Task)?
            .map_err(|_| BackendTaskFailure::ReadTimeout)
    }
}

/// Failure to finish a command task on the backend runtime.
#[derive(Debug)]
pub(crate) enum BackendTaskFailure {
    Task(tokio::task::JoinError),
    ReadTimeout,
}

impl std::fmt::Display for BackendTaskFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Task(error) => write!(formatter, "backend task failed: {error}"),
            Self::ReadTimeout => {
                formatter.write_str("The backend read timed out. Retry the query.")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn work_from_a_current_thread_runtime_uses_the_captured_runtime() {
        let backend_tasks = BackendTasks::current();
        let value = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("the test current-thread runtime should build")
                .block_on(async move {
                    backend_tasks
                        .execute(async { tokio::task::block_in_place(|| 42) })
                        .await
                        .expect("the task should finish on the captured multi-thread runtime")
                })
        })
        .join()
        .expect("the current-thread runtime should not panic");

        assert_eq!(value, 42);
    }
    struct ReadLifetime(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for ReadLifetime {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[tokio::test]
    async fn dropping_a_read_cancels_its_work_but_dropping_a_command_does_not() {
        let runtime = BackendTasks::current();
        let (started, started_receiver) = tokio::sync::oneshot::channel();
        let (dropped, dropped_receiver) = tokio::sync::oneshot::channel();
        let read_runtime = runtime.clone();
        let caller = tokio::spawn(async move {
            read_runtime
                .read(async move {
                    let _lifetime = ReadLifetime(Some(dropped));
                    let _ = started.send(());
                    std::future::pending::<()>().await;
                })
                .await
        });
        started_receiver.await.unwrap();
        caller.abort();
        let _ = caller.await;
        tokio::time::timeout(std::time::Duration::from_secs(1), dropped_receiver)
            .await
            .unwrap()
            .unwrap();

        let (started, started_receiver) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let (completed, completion) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(async move {
            runtime
                .execute(async move {
                    let _ = started.send(());
                    let value = released.await.unwrap();
                    let _ = completed.send(value);
                })
                .await
        });
        started_receiver.await.unwrap();
        caller.abort();
        let _ = caller.await;
        release.send(42).unwrap();
        assert_eq!(completion.await.unwrap(), 42);
    }
}
