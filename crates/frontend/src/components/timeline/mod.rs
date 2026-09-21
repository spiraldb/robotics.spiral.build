//! Controlled timeline shared by Data, Curation, and Live.
//! Callers own the clock and effects; tracks and toolbar slots customize content.
//!
//! Build `TimelineState` from the caller's clock, bind `TimelineEvent` with
//! `TimelineInteraction::new`, then pass that binding both to `Timeline` and
//! its rows. `TimelineContent` accepts ordinary Dioxus elements for the rows,
//! identity, toolbar additions, ruler corner, and notices. Density and numeric
//! tracks are provided; custom rows share `TimelineScrubber` and `ViewWindow`.
//!
//! Event targets must identify the current source revision. Reject events from
//! old targets in the owning controller. Preview/commit/cancel are distinct;
//! only the owner decides which phases may cause external effects. Interval
//! activation returns the interval unchanged so callers choose its behavior.
//! Components neither fetch data nor know about cameras, datasets, or robots.
mod density;
mod interaction;
mod playback;
#[allow(clippy::module_inception)]
mod timeline;
mod tracks;
pub(crate) use density::DensityTrack;
pub(crate) use interaction::{
    KeyboardFocus, SeekPhase, TimelineAction, TimelineEvent, TimelineInteraction, TimelineState,
    TransportKey,
};
pub(crate) use playback::{PlaybackClock, ViewWindow};
pub(crate) use timeline::{Timeline, TimelineContent, TimelineScrubber};
pub(crate) use tracks::{
    NumericTrack, NumericTrackRow, TimelineInterval, TimelineSample, TimelineTrack,
    TimelineTrackState,
};
