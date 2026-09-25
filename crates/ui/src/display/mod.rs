//! Display: what only says something. A fact is a label and its value, and a
//! notice or empty state is what a surface says when it has nothing else to
//! show.

mod facts;
mod kind_mark;
mod notice;

pub use facts::{Fact, FactLayout, FactList, FactListProps, FactProps};
pub use kind_mark::{KindMark, KindMarkProps};
pub use notice::{EmptyState, EmptyStateProps, Notice, NoticeProps, NoticeTone};
