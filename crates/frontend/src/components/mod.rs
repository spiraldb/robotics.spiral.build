//! Reusable presentational components shared across screens and panels.

pub(crate) mod canvas_controls;
pub(crate) mod charts;
pub(crate) mod document_bar;
pub(crate) mod icons;
pub(crate) mod page_keys;

pub(crate) mod time_series;

pub(crate) mod timeline;

pub(crate) mod document_session;

/// The dataset field five surfaces spell separately. Streams D, E, and F move
/// their remaining callers onto it.
pub(crate) mod dataset_picker;

pub(crate) mod scoped_task;

pub(crate) mod flow_canvas;
