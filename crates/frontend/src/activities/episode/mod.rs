//! Episodes: one recording of a dataset, opened, named and inspected the same
//! way on every surface that shows one.
//!
//! [`Episode`] is which recording it is and what every heading calls it;
//! [`EpisodeInspection`] shows it whole — cameras, robot, plots and the
//! timeline of its streams, docked on a clock of its own and arranged for the
//! room a surface gives it, or as the operator last arranged the dataset's
//! episodes. [`EpisodeRows`] is the row it gets in any list of recordings,
//! and [`EpisodeStill`] the picture that row leads with, decoded once per
//! project.

mod arrangement;
mod boxes;
mod camera;
mod choices;
mod decoder;
mod filmstrip;
mod heading;
mod identity;
mod inspection;
mod listing;
mod messages;
mod plots;
mod reads;
mod remembered_layout;
mod robot;
mod sparkline;
mod still;
mod streams;
mod timeline;
mod transport;

pub(in crate::activities) use boxes::{BoxedPicture, CameraBoxes, PictureBox};
pub(crate) use decoder::{CameraPlayback, EncodedCamera};
pub(in crate::activities) use heading::{EpisodeHeading, HeadingSubject};
pub(in crate::activities) use identity::{Episode, EpisodeFactsPresentation, SourceLabel};
pub(in crate::activities) use inspection::{EpisodeInspection, FeaturedCamera, PlayRequest};
pub(in crate::activities) use listing::{EpisodeRows, RowFace, RowFaceView};
pub(crate) use still::EpisodeStills;
pub(in crate::activities) use still::{EpisodeStill, StillMoment};
pub(in crate::activities) use streams::StreamCatalog;
pub(in crate::activities) use timeline::EvidenceTrack;
