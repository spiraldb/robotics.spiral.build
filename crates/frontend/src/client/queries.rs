//! Shared lifecycle for fetched state: scoped cancellation, invalidation,
//! deadlines, and completion-based polling. Timers never cancel a slow read.
use super::CommandError;
use dioxus::prelude::*;
use std::future::Future;

#[derive(Clone, Copy)]
pub(crate) enum QueryDomain {
    Project,
    RobotSetup,
    Telemetry,
    Recording,
    Training,
    Rollout,
    Workflow,
    Assistant,
    Settings,
    Catalog,
    Data,
    Curation,
}

#[derive(Clone, Copy)]
pub(crate) struct QueryRefresh {
    revisions: [Signal<u64>; 12],
}
impl QueryRefresh {
    pub(crate) fn use_provider() -> Self {
        use_context_provider(|| Self {
            revisions: std::array::from_fn(|_| Signal::new(0)),
        })
    }
    pub(crate) fn current() -> Self {
        consume_context()
    }
    pub(crate) fn invalidate(self, domains: &[QueryDomain]) {
        for domain in domains {
            let mut revision = self.revisions[*domain as usize];
            let next = *revision.peek() + 1;
            revision.set(next);
        }
    }
    fn revision(self, domain: QueryDomain) -> Signal<u64> {
        self.revisions[domain as usize]
    }
}

/// How long a policy waits before reading again.
///
/// A fixed interval never changes. A measured one is consulted after every
/// completed request, so a read can slow down — or stop entirely — when what
/// it watches is idle or off screen, without its caller rebuilding the policy.
#[derive(Clone, Copy)]
enum QueryInterval {
    OnDemand,
    Fixed(u32),
    Measured(ReadSignal<Option<u32>>),
}

impl QueryInterval {
    /// Whether this interval ever repeats a read. Constant per call site, so
    /// the timer hook it guards is too.
    const fn repeats(self) -> bool {
        !matches!(self, Self::OnDemand)
    }
    /// How long to wait before the next read, read reactively.
    fn milliseconds(self) -> Option<u32> {
        match self {
            Self::OnDemand => None,
            Self::Fixed(interval_ms) => Some(interval_ms),
            Self::Measured(interval_ms) => interval_ms(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct QueryPolicy {
    domain: QueryDomain,
    interval: QueryInterval,
}
impl QueryPolicy {
    pub(crate) const fn new(domain: QueryDomain, interval_ms: u32) -> Self {
        Self {
            domain,
            interval: QueryInterval::Fixed(interval_ms),
        }
    }

    /// A read whose cadence answers what it finds: the signal is read after
    /// every completed request, and `None` parks the read until it names an
    /// interval again.
    pub(crate) const fn measured(
        domain: QueryDomain,
        interval_ms: ReadSignal<Option<u32>>,
    ) -> Self {
        Self {
            domain,
            interval: QueryInterval::Measured(interval_ms),
        }
    }

    /// Fetch once, then keep the result until an input or command invalidates it.
    pub(crate) const fn on_demand(domain: QueryDomain) -> Self {
        Self {
            domain,
            interval: QueryInterval::OnDemand,
        }
    }

    /// Reactive inputs and command invalidations cancel obsolete reads. A timer
    /// waits for completion before starting its delay, so requests never pile up.
    pub(crate) fn use_resource<
        T: 'static,
        E: From<CommandError> + 'static,
        F: Future<Output = Result<T, E>> + 'static,
    >(
        self,
        fetch: impl FnMut() -> F + 'static,
    ) -> Resource<Result<T, E>> {
        self.use_when(|| true, fetch)
    }

    pub(crate) fn use_when<
        T: 'static,
        E: From<CommandError> + 'static,
        F: Future<Output = Result<T, E>> + 'static,
    >(
        self,
        mut available: impl FnMut() -> bool + 'static,
        mut fetch: impl FnMut() -> F + 'static,
    ) -> Resource<Result<T, E>> {
        let revision = QueryRefresh::current().revision(self.domain);
        let resource = use_resource(move || {
            let _ = revision();
            let enabled = available();
            let request = enabled.then(&mut fetch);
            async move {
                match request {
                    Some(request) => Self::with_deadline(request).await,
                    None => std::future::pending().await,
                }
            }
        });
        if self.interval.repeats() {
            Self::use_timer(resource, self.interval);
        }
        resource
    }

    fn use_timer<T: 'static, E: 'static>(
        mut resource: Resource<Result<T, E>>,
        interval: QueryInterval,
    ) {
        // This timer depends on the read's lifecycle. Restarting a read cancels
        // its old timer; completing it starts a new delay. Neither a slow read
        // nor a paused resource has a running refresh timer, so the caller's
        // interval is the whole policy.
        use_resource(move || {
            let ready = *resource.state().read() == UseResourceState::Ready;
            let completed_task = resource.task();
            let timer = ready
                .then(|| interval.milliseconds())
                .flatten()
                .map(gloo_timers::future::TimeoutFuture::new);
            async move {
                if let Some(timer) = timer {
                    timer.await;
                    if resource.finished() && resource.task() == completed_task {
                        resource.restart();
                    }
                }
            }
        });
    }

    async fn with_deadline<T, E: From<CommandError>>(
        request: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        use futures_util::future::{Either, select};
        match select(
            Box::pin(request),
            Box::pin(gloo_timers::future::TimeoutFuture::new(10_000)),
        )
        .await
        {
            Either::Left((result, _)) => result,
            Either::Right(_) => Err(CommandError::new(
                spiral_robotics_common::command_failure::CommandFailureCode::Transport,
                "The backend read timed out.",
            )
            .into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        task::{Poll, Waker},
    };

    #[derive(Default)]
    struct ControlledRead {
        answer: Cell<Option<u32>>,
        waker: RefCell<Option<Waker>>,
        dropped: Cell<bool>,
    }
    impl ControlledRead {
        fn complete(&self, value: u32) {
            self.answer.set(Some(value));
            if let Some(waker) = self.waker.borrow_mut().take() {
                waker.wake();
            }
        }
    }
    struct ReadFuture(Rc<ControlledRead>);
    impl Future for ReadFuture {
        type Output = Result<u32, CommandError>;
        fn poll(
            self: std::pin::Pin<&mut Self>,
            context: &mut std::task::Context<'_>,
        ) -> Poll<Self::Output> {
            if let Some(value) = self.0.answer.get() {
                Poll::Ready(Ok(value))
            } else {
                *self.0.waker.borrow_mut() = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
    impl Drop for ReadFuture {
        fn drop(&mut self) {
            self.0.dropped.set(true);
        }
    }
    #[derive(Clone, Default)]
    struct QueryHarness(Rc<RefCell<QueryObservation>>);
    #[derive(Default)]
    struct QueryObservation {
        refresh: Option<QueryRefresh>,
        resource: Option<Resource<Result<u32, CommandError>>>,
        available: Option<Signal<bool>>,
        reads: Vec<Rc<ControlledRead>>,
    }
    impl QueryHarness {
        fn render(self) -> Element {
            let refresh = QueryRefresh::use_provider();
            let available = use_signal(|| true);
            let observations = self.0.clone();
            let resource = QueryPolicy::new(QueryDomain::Workflow, 20).use_when(
                move || *available.read(),
                move || {
                    let read = Rc::new(ControlledRead::default());
                    observations.borrow_mut().reads.push(read.clone());
                    ReadFuture(read)
                },
            );
            {
                let mut state = self.0.borrow_mut();
                state.refresh = Some(refresh);
                state.resource = Some(resource);
                state.available = Some(available);
            }
            rsx! {}
        }
        fn refresh(&self) {
            self.0
                .borrow()
                .refresh
                .unwrap()
                .invalidate(&[QueryDomain::Workflow]);
        }
        fn result(&self) -> Option<Result<u32, CommandError>> {
            self.0.borrow().resource.unwrap().peek().clone()
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn invalidation_cancels_the_old_read_and_only_the_new_result_is_visible() {
        let harness = QueryHarness::default();
        let mut dom = VirtualDom::new_with_props(QueryHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let old = harness.0.borrow().reads[0].clone();
        dom.in_runtime(|| harness.refresh());
        dom.render_immediate_to_vec();
        assert!(old.dropped.get());
        let new = harness.0.borrow().reads.last().unwrap().clone();
        assert!(!Rc::ptr_eq(&old, &new));
        old.complete(1);
        dom.render_immediate_to_vec();
        assert!(harness.result().is_none());
        new.complete(2);
        dom.render_immediate_to_vec();
        assert_eq!(harness.result(), Some(Ok(2)));
        drop(dom);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn unavailable_readiness_cancels_reads_and_recovers_through_the_same_owner() {
        let harness = QueryHarness::default();
        let mut dom = VirtualDom::new_with_props(QueryHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        let first = harness.0.borrow().reads[0].clone();
        let mut available = harness.0.borrow().available.unwrap();
        dom.in_runtime(|| available.set(false));
        dom.render_immediate_to_vec();
        assert!(first.dropped.get());
        assert!(harness.result().is_none());
        dom.in_runtime(|| available.set(true));
        dom.render_immediate_to_vec();
        harness.0.borrow().reads.last().unwrap().complete(7);
        dom.render_immediate_to_vec();
        assert_eq!(harness.result(), Some(Ok(7)));
        let resource = harness.0.borrow().resource.unwrap();
        dom.in_runtime(|| harness.refresh());
        dom.render_immediate_to_vec();
        let pending = harness.0.borrow().reads.last().unwrap().clone();
        assert!(!resource.finished());
        drop(dom);
        assert!(pending.dropped.get());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn an_expired_old_timer_neither_overlaps_nor_cancels_a_slow_invalidated_read() {
        let harness = QueryHarness::default();
        let mut dom = VirtualDom::new_with_props(QueryHarness::render, harness.clone());
        dom.rebuild_in_place();
        dom.render_immediate_to_vec();
        // Comfortably past the harness's 20 ms interval, whether or not the
        // test tab is in the foreground.
        let interval = 50;
        gloo_timers::future::TimeoutFuture::new(interval).await;
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().reads.len(), 1);
        harness.0.borrow().reads[0].complete(1);
        dom.render_immediate_to_vec();
        dom.in_runtime(|| harness.refresh());
        dom.render_immediate_to_vec();
        let pending = harness.0.borrow().reads[1].clone();
        gloo_timers::future::TimeoutFuture::new(interval).await;
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().reads.len(), 2);
        assert!(!pending.dropped.get());
        pending.complete(2);
        dom.render_immediate_to_vec();
        assert_eq!(harness.result(), Some(Ok(2)));
        gloo_timers::future::TimeoutFuture::new(interval).await;
        dom.render_immediate_to_vec();
        assert_eq!(harness.0.borrow().reads.len(), 3);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn browser_deadline_drops_a_stalled_read_and_returns_a_transport_error() {
        let read = Rc::new(ControlledRead::default());
        let result = QueryPolicy::with_deadline(ReadFuture(read.clone())).await;
        assert_eq!(
            result,
            Err(CommandError::new(
                spiral_robotics_common::command_failure::CommandFailureCode::Transport,
                "The backend read timed out.",
            ))
        );
        assert!(read.dropped.get());
    }
}
