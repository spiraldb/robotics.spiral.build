//! Episodes: one recording of a dataset, opened, named and played the same
//! way on every surface that shows one.
//!
//! [`Episode`] is which recording it is and what every heading calls it;
//! [`EpisodeInspection`] shows it whole on a clock of its own. Data composes
//! the same parts — the transport, cameras and robot stage — into its dockable
//! workspace around one shared clock.

mod camera;
mod decoder;
mod identity;
mod inspection;
mod messages;
mod robot;
mod streams;
mod transport;

pub(in crate::activities) use camera::EpisodeCamera;
pub(crate) use decoder::{DecodedGroup, EncodedCamera};
pub(in crate::activities) use identity::{
    Episode, EpisodeFactList, EpisodeFactsPresentation, EpisodeLabels, SourceLabel,
};
pub(in crate::activities) use inspection::{EpisodeInspection, FeaturedCamera, PlayRequest};
pub(in crate::activities) use messages::{MessageBuffer, MessageField};
pub(in crate::activities) use robot::{RobotStage, StagePlacement};
pub(in crate::activities) use streams::{RecordedStream, StreamCatalog, StreamKind};
pub(in crate::activities) use transport::{EpisodeTimeRange, EpisodeTransport};
