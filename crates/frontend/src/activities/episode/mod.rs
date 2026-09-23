//! Episodes: one recording of a dataset, opened, named and inspected the same
//! way on every surface that shows one.
//!
//! [`Episode`] is which recording it is and what every heading calls it;
//! [`EpisodeInspection`] shows it whole — cameras, robot, plots and the
//! timeline of its streams, docked on a clock of its own and arranged for the
//! room a surface gives it.

mod arrangement;
mod camera;
mod choices;
mod decoder;
mod identity;
mod inspection;
mod messages;
mod plots;
mod robot;
mod streams;
mod timeline;
mod transport;

pub(crate) use decoder::{DecodedGroup, EncodedCamera};
pub(in crate::activities) use identity::{
    Episode, EpisodeFactsPresentation, EpisodeLabels, SourceLabel,
};
pub(in crate::activities) use inspection::{EpisodeInspection, FeaturedCamera, PlayRequest};
pub(in crate::activities) use streams::StreamCatalog;
