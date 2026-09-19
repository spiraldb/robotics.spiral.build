//! Recording preview geometry. Saved camera bytes live only in MCAP.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VideoSize {
    width: u32,
    height: u32,
}

impl VideoSize {
    pub(super) const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub(super) const fn width(self) -> u32 {
        self.width
    }

    pub(super) const fn height(self) -> u32 {
        self.height
    }
}
