//! Display: what only says something. A chip is one word of state, a fact is
//! a label and its value, and a notice or empty state is what a surface says
//! when it has nothing else to show.

mod chip;
mod facts;
mod notice;

pub use chip::{Chip, ChipProps, ChipTone};
pub use facts::{Fact, FactLayout, FactList, FactListProps, FactProps};
pub use notice::{EmptyState, EmptyStateProps, Notice, NoticeProps, NoticeTone};
