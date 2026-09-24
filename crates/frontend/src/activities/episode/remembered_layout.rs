//! The arrangement an operator made of one dataset's episodes, kept in this
//! browser so every episode of that dataset opens the same way — in Data and
//! in every Curation node — until they arrange it again.
//!
//! A dataset's episodes record the same cameras, so one arrangement fits all
//! of them; an episode missing a panel shows the rest where they were kept.
//! It is one browser's convenience: a browser that keeps no storage opens
//! every episode in the designed arrangement.
use dioxus_workbench::PanelLayout;
use spiral_robotics_common::datasets::DatasetSelection;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RememberedLayout {
    key: String,
}

impl RememberedLayout {
    pub(super) fn of(dataset: &DatasetSelection) -> Self {
        Self {
            key: format!("spiral-studio.episode-layout.{}", dataset.label()),
        }
    }
    /// The arrangement kept for this dataset, if one was.
    pub(super) fn read(&self) -> Option<PanelLayout> {
        Self::storage()?
            .get_item(&self.key)
            .ok()
            .flatten()
            .and_then(|kept| PanelLayout::decode(&kept))
    }
    pub(super) fn keep(&self, layout: &PanelLayout) {
        if let (Some(storage), Some(encoded)) = (Self::storage(), layout.encode()) {
            let _ = storage.set_item(&self.key, &encoded);
        }
    }
    /// The workbench scope of a kept arrangement: one per dataset, so a room
    /// that changes size keeps what the operator made.
    pub(super) fn scope(&self) -> String {
        self.key.clone()
    }
    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }
}
