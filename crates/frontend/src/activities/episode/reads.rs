//! What the inspected episode has read of its streams: each shown camera's
//! encoded video and each plotted stream's values over the whole episode.
//!
//! A camera's panel and its timeline filmstrip draw the same video, and a
//! stream's plot and its timeline sparkline the same values, so each is read
//! once per episode and shared. Whichever view asks first starts the read,
//! and the read belongs to the inspection, so closing one view does not
//! abandon a read another is waiting on.
use super::{
    decoder::EncodedCamera, messages::MessageField, plots::PlottedStream, streams::RecordedStream,
    transport::EpisodeTimeRange,
};
use crate::components::scoped_task::ScopedTask;
use dioxus::dioxus_core::{ScopeId, current_scope_id};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::MessageWindow;
use std::{collections::BTreeMap, rc::Rc};

/// One read of a stream, as the views waiting on it see it.
#[derive(Debug)]
pub(super) enum StreamRead<T> {
    /// Being read; what an earlier read of the same stream answered, if
    /// anything, stands in until the new answer lands.
    Reading(Option<Rc<T>>),
    Ready(Rc<T>),
    Failed(String),
}

impl<T> StreamRead<T> {
    /// What to draw: the answer, or the earlier one standing in for it.
    pub(super) fn latest(&self) -> Option<&Rc<T>> {
        match self {
            Self::Reading(previous) => previous.as_ref(),
            Self::Ready(answer) => Some(answer),
            Self::Failed(_) => None,
        }
    }
    pub(super) fn reading(&self) -> bool {
        matches!(self, Self::Reading(_))
    }
    pub(super) fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed(error) => Some(error),
            _ => None,
        }
    }
    fn answered(answer: Result<T, String>) -> Self {
        match answer {
            Ok(answer) => Self::Ready(Rc::new(answer)),
            Err(error) => Self::Failed(error),
        }
    }
}

impl<T> Clone for StreamRead<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Reading(previous) => Self::Reading(previous.clone()),
            Self::Ready(answer) => Self::Ready(answer.clone()),
            Self::Failed(error) => Self::Failed(error.clone()),
        }
    }
}

/// Two reads are the same when they hold the same answer, not an equal one:
/// an answer is read once and handed around.
impl<T> PartialEq for StreamRead<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Reading(left), Self::Reading(right)) => match (left, right) {
                (Some(left), Some(right)) => Rc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            },
            (Self::Ready(left), Self::Ready(right)) => Rc::ptr_eq(left, right),
            (Self::Failed(left), Self::Failed(right)) => left == right,
            _ => false,
        }
    }
}

/// A stream's values for the fields last asked of it.
#[derive(Clone, PartialEq)]
struct ValuesRead {
    fields: Vec<MessageField>,
    read: StreamRead<PlottedStream>,
}

/// Every read of the inspected episode's streams, keyed by stream.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct StreamReads {
    scope: ScopeId,
    range: EpisodeTimeRange,
    cameras: Signal<BTreeMap<String, StreamRead<EncodedCamera>>>,
    values: Signal<BTreeMap<String, ValuesRead>>,
}

impl StreamReads {
    /// The reads of an episode recorded over `range`, owned by the calling
    /// component.
    pub(super) fn use_reads(range: EpisodeTimeRange) -> Self {
        use_hook(|| Self {
            scope: current_scope_id(),
            range,
            cameras: Signal::new(BTreeMap::new()),
            values: Signal::new(BTreeMap::new()),
        })
    }

    /// `camera`'s video, once a view has asked for it.
    pub(super) fn camera(&self, camera: &RecordedStream) -> Option<StreamRead<EncodedCamera>> {
        self.cameras.read().get(camera.key()).cloned()
    }
    /// Read `camera`'s video unless it is read or being read.
    pub(super) fn want_camera(&mut self, camera: &RecordedStream) {
        if !self.cameras.peek().contains_key(camera.key()) {
            self.read_camera(camera);
        }
    }
    /// Read `camera`'s video afresh, as after a failure.
    pub(super) fn read_camera(&mut self, camera: &RecordedStream) {
        let key = camera.key().to_owned();
        self.cameras
            .write()
            .insert(key.clone(), StreamRead::Reading(None));
        let window = MessageWindow::new(
            self.range.bounds(),
            vec![
                vec!["value".into(), "format".into()],
                vec!["value".into(), "data".into()],
            ],
        );
        let request = camera.read().clone().with_window(window);
        let origin = self.range.start_ns();
        let mut cameras = self.cameras;
        ScopedTask::spawn(self.scope, async move {
            let answer = EncodedCamera::read(request, origin).await;
            cameras.write().insert(key, StreamRead::answered(answer));
        });
    }

    /// `stream`'s values for the fields last asked of it, once a view has
    /// asked.
    pub(super) fn values(&self, stream: &RecordedStream) -> Option<StreamRead<PlottedStream>> {
        self.values
            .read()
            .get(stream.key())
            .map(|values| values.read.clone())
    }
    /// Read `fields` of `stream` over the episode unless they are read or
    /// being read. Asking for other fields replaces the read; the earlier
    /// answer stands in until the new one lands.
    pub(super) fn want_values(&mut self, stream: &RecordedStream, fields: Vec<MessageField>) {
        let asked = self
            .values
            .peek()
            .get(stream.key())
            .is_some_and(|values| values.fields == fields);
        if !asked && !fields.is_empty() {
            self.read_values(stream, fields);
        }
    }
    /// Read `fields` of `stream` afresh, as after a failure.
    pub(super) fn read_values(&mut self, stream: &RecordedStream, fields: Vec<MessageField>) {
        let key = stream.key().to_owned();
        let previous = self
            .values
            .peek()
            .get(&key)
            .and_then(|values| values.read.latest().cloned());
        self.values.write().insert(
            key.clone(),
            ValuesRead {
                fields: fields.clone(),
                read: StreamRead::Reading(previous),
            },
        );
        let read = stream.read().clone();
        let range = self.range;
        let mut values = self.values;
        ScopedTask::spawn(self.scope, async move {
            let answer =
                PlottedStream::read(read, fields.clone(), range.bounds(), range.start_ns()).await;
            // A read for fields asked for since answers nobody.
            let current = values
                .peek()
                .get(&key)
                .is_some_and(|values| values.fields == fields);
            if current {
                values.write().insert(
                    key,
                    ValuesRead {
                        fields,
                        read: StreamRead::answered(answer),
                    },
                );
            }
        });
    }
}
