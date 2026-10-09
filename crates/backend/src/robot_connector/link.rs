//! One workspace's link to its robot connector, as the connector's requests
//! make it: the health its polls report, the browser requests waiting for
//! its polls and for its answers, and the live streams someone watches.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use spiral_robotics_common::robot_connector::{RelayedRequestHead, RobotConnectorHealth};
use tokio::sync::{Notify, oneshot, watch};
use tokio::time::{Duration, Instant};

use super::RobotConnectors;
use super::live::LiveStreams;
use crate::OperationError;
use crate::sync::MutexRecovery as _;

/// How long a poll waits for a request to hand out before it answers that
/// none came.
const POLL_WAIT: Duration = Duration::from_secs(25);
/// How long a connector counts as linked after its last poll ended: a poll's
/// wait, and time to spare for its next poll to arrive.
const LINKED_AFTER_POLL: Duration = Duration::from_secs(35);
/// How long a relayed request the connector received waits for its answer.
/// Starting a policy waits for the policy to load and warm up.
const ANSWER_WAIT: Duration = Duration::from_secs(10 * 60);

pub(super) struct ConnectorLink {
    presence: watch::Sender<Presence>,
    requests: RelayedRequests,
    live: Arc<LiveStreams>,
    /// Wakes a waiting poll: a request was queued, the watched streams
    /// changed, or the link closed.
    wakeup: Arc<Notify>,
    closed: AtomicBool,
}

impl ConnectorLink {
    pub(super) fn new() -> Arc<Self> {
        let wakeup = Arc::new(Notify::new());
        Arc::new(Self {
            presence: watch::Sender::default(),
            requests: RelayedRequests::new(),
            live: LiveStreams::new(wakeup.clone()),
            wakeup,
            closed: AtomicBool::new(false),
        })
    }

    /// The health the connector's polls report, while it counts as linked:
    /// while a poll waits, and for a while after one ends.
    pub(super) fn health(&self) -> Option<RobotConnectorHealth> {
        self.presence.borrow().linked_health().cloned()
    }

    pub(super) fn live(&self) -> &Arc<LiveStreams> {
        &self.live
    }

    /// The connector's poll, which reports its `health`: the oldest request
    /// waiting for it, or none when its wait passes or the watched streams
    /// change first. Refused once the link is closed.
    pub(super) async fn next_request(
        &self,
        health: RobotConnectorHealth,
    ) -> Result<Option<DeliveredRequest>, OperationError> {
        let _waiting = WaitingPoll::start(self, health);
        let deadline = Instant::now() + POLL_WAIT;
        loop {
            // Listen before looking, so a change in between still wakes this poll.
            let mut wakeup = std::pin::pin!(self.wakeup.notified());
            wakeup.as_mut().enable();
            if self.closed.load(Ordering::Acquire) {
                return Err(RobotConnectors::refused());
            }
            if let Some(request) = self.requests.next_queued() {
                return Ok(Some(request));
            }
            if self.live.unannounced() {
                return Ok(None);
            }
            tokio::select! {
                () = wakeup => {}
                () = tokio::time::sleep_until(deadline) => return Ok(None),
            }
        }
    }

    /// Relay `request` to the connector's next poll and wait for its answer.
    /// A request no poll took fails once the connector stops counting as
    /// linked; one it took waits for its answer up to [`ANSWER_WAIT`].
    pub(super) async fn relay(
        &self,
        request: RelayedRequest,
    ) -> Result<RelayedAnswer, OperationError> {
        let mut answer = self
            .requests
            .enqueue(request)
            .ok_or_else(Self::disconnected)?;
        self.wakeup.notify_waiters();
        let give_up = Instant::now() + ANSWER_WAIT;
        let mut presence = self.presence.subscribe();
        loop {
            let unlinked_at = presence.borrow_and_update().unlinked_at();
            let undelivered = answer.undelivered();
            tokio::select! {
                answer = answer.receive() => return answer.ok_or_else(Self::disconnected),
                () = tokio::time::sleep_until(give_up) => {
                    return Err(OperationError::Unavailable(
                        "The robot connector did not answer within 10 minutes.".to_owned(),
                    ));
                }
                // A poll started or ended: look again.
                _ = presence.changed() => {}
                () = tokio::time::sleep_until(unlinked_at.unwrap_or(give_up)),
                    if undelivered && unlinked_at.is_some() =>
                {
                    let unlinked = presence
                        .borrow()
                        .unlinked_at()
                        .is_some_and(|at| at <= Instant::now());
                    if unlinked && answer.undelivered() {
                        return Err(OperationError::Unavailable(
                            "The robot connector stopped answering".to_owned(),
                        ));
                    }
                }
            }
        }
    }

    /// Complete the relayed request `id` with `answer`; false when no
    /// request `id` awaits one.
    pub(super) fn answer(&self, id: u64, answer: RelayedAnswer) -> bool {
        self.requests.answer(id, answer)
    }

    /// Close the link: its polls are refused, and the browser requests and
    /// viewers waiting on it give up.
    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.requests.close();
        self.live.close();
        self.wakeup.notify_waiters();
    }

    fn disconnected() -> OperationError {
        OperationError::Unavailable(
            "The robot connector was disconnected before it answered.".to_owned(),
        )
    }
}

/// What the connector's polls report, and when it polls.
#[derive(Default)]
struct Presence {
    health: Option<RobotConnectorHealth>,
    polls_waiting: usize,
    last_poll_ended: Option<Instant>,
}

impl Presence {
    fn linked_health(&self) -> Option<&RobotConnectorHealth> {
        let linked = self
            .unlinked_at()
            .is_none_or(|unlinked| Instant::now() < unlinked);
        self.health.as_ref().filter(|_| linked)
    }

    /// When the connector stops counting as linked unless it polls again;
    /// none while a poll waits.
    fn unlinked_at(&self) -> Option<Instant> {
        if self.polls_waiting > 0 {
            return None;
        }
        Some(
            self.last_poll_ended
                .map_or_else(Instant::now, |ended| ended + LINKED_AFTER_POLL),
        )
    }
}

/// A poll in progress, which keeps its connector linked until it ends.
struct WaitingPoll<'a> {
    link: &'a ConnectorLink,
}

impl<'a> WaitingPoll<'a> {
    fn start(link: &'a ConnectorLink, health: RobotConnectorHealth) -> Self {
        link.presence.send_modify(|presence| {
            presence.health = Some(health);
            presence.polls_waiting += 1;
        });
        Self { link }
    }
}

impl Drop for WaitingPoll<'_> {
    fn drop(&mut self) {
        self.link.presence.send_modify(|presence| {
            presence.polls_waiting -= 1;
            presence.last_poll_ended = Some(Instant::now());
        });
    }
}

/// The browser requests relayed to the connector: those queued for its
/// polls, and the answers each awaits, by ID. None once the link closed.
struct RelayedRequests(Mutex<Option<RelayQueue>>);

#[derive(Default)]
struct RelayQueue {
    next_id: u64,
    queued: VecDeque<(u64, RelayedRequest)>,
    awaiting: HashMap<u64, oneshot::Sender<RelayedAnswer>>,
}

impl RelayedRequests {
    fn new() -> Self {
        Self(Mutex::new(Some(RelayQueue::default())))
    }

    /// Queue `request` for the connector's polls; none once the link closed.
    fn enqueue(&self, request: RelayedRequest) -> Option<PendingAnswer<'_>> {
        let mut state = self.0.recover_lock();
        let queue = state.as_mut()?;
        let id = queue.next_id;
        queue.next_id += 1;
        let (sender, receiver) = oneshot::channel();
        queue.awaiting.insert(id, sender);
        queue.queued.push_back((id, request));
        Some(PendingAnswer {
            requests: self,
            id,
            receiver,
        })
    }

    /// The oldest queued request whose browser still waits for it.
    fn next_queued(&self) -> Option<DeliveredRequest> {
        let mut state = self.0.recover_lock();
        let queue = state.as_mut()?;
        while let Some((id, request)) = queue.queued.pop_front() {
            if queue.awaiting.contains_key(&id) {
                return Some(request.deliver(id));
            }
        }
        None
    }

    fn answer(&self, id: u64, answer: RelayedAnswer) -> bool {
        let awaiting = self
            .0
            .recover_lock()
            .as_mut()
            .and_then(|queue| queue.awaiting.remove(&id));
        // A browser that gave up since then no longer takes the answer.
        awaiting.is_some_and(|awaiting| awaiting.send(answer).is_ok())
    }

    /// Whether the request `id` still waits for a poll to take it.
    fn queued(&self, id: u64) -> bool {
        self.0
            .recover_lock()
            .as_ref()
            .is_some_and(|queue| queue.queued.iter().any(|(queued, _)| *queued == id))
    }

    fn give_up(&self, id: u64) {
        if let Some(queue) = self.0.recover_lock().as_mut() {
            queue.awaiting.remove(&id);
        }
    }

    fn close(&self) {
        self.0.recover_lock().take();
    }
}

/// The answer a relayed request awaits; dropping it gives the request up,
/// so a poll no longer hands it out.
struct PendingAnswer<'a> {
    requests: &'a RelayedRequests,
    id: u64,
    receiver: oneshot::Receiver<RelayedAnswer>,
}

impl PendingAnswer<'_> {
    /// The answer; none when the link closed first.
    async fn receive(&mut self) -> Option<RelayedAnswer> {
        (&mut self.receiver).await.ok()
    }

    /// Whether no poll took the request yet.
    fn undelivered(&self) -> bool {
        self.requests.queued(self.id)
    }
}

impl Drop for PendingAnswer<'_> {
    fn drop(&mut self) {
        self.requests.give_up(self.id);
    }
}

/// A browser request on its way to the connector.
pub(super) struct RelayedRequest {
    method: String,
    /// The connector's own path and query, such as `/api/robots`.
    path: String,
    content_type: Option<HeaderValue>,
    body: Bytes,
}

impl RelayedRequest {
    pub(super) fn new(
        method: String,
        path: String,
        content_type: Option<HeaderValue>,
        body: Bytes,
    ) -> Self {
        Self {
            method,
            path,
            content_type,
            body,
        }
    }

    fn deliver(self, id: u64) -> DeliveredRequest {
        DeliveredRequest {
            head: RelayedRequestHead::new(id, self.method, self.path),
            content_type: self.content_type,
            body: self.body,
        }
    }
}

/// A relayed request, as a poll hands it to the connector.
pub(super) struct DeliveredRequest {
    head: RelayedRequestHead,
    content_type: Option<HeaderValue>,
    body: Bytes,
}

impl DeliveredRequest {
    pub(super) fn into_parts(self) -> (RelayedRequestHead, Option<HeaderValue>, Bytes) {
        (self.head, self.content_type, self.body)
    }
}

/// The connector's answer to a relayed request, which the browser gets as
/// it is.
pub(super) struct RelayedAnswer {
    status: StatusCode,
    content_type: Option<HeaderValue>,
    body: Bytes,
}

impl RelayedAnswer {
    pub(super) fn new(status: StatusCode, content_type: Option<HeaderValue>, body: Bytes) -> Self {
        Self {
            status,
            content_type,
            body,
        }
    }
}

impl IntoResponse for RelayedAnswer {
    fn into_response(self) -> Response {
        let mut response = (self.status, Body::from(self.body)).into_response();
        if let Some(content_type) = self.content_type {
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, content_type);
        }
        response
    }
}
