//! Published progress models: a progress run's checkpoint published under a
//! name, so Score progress can score with it. The training service keeps the
//! weights; Studio keeps the name and where they came from.

use serde::{Deserialize, Serialize};

/// Which checkpoint to publish, and the name it is listed by.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelPublication {
    run_id: String,
    step: u64,
    name: String,
}

impl ModelPublication {
    pub fn new(run_id: impl Into<String>, step: u64, name: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            step,
            name: name.into(),
        }
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub const fn step(&self) -> u64 {
        self.step
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A published progress model: the checkpoint and the name it was published
/// under, the selector the scorer reads its weights by, and the camera it
/// was trained on, which is the one it scores.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublishedModel {
    #[serde(flatten)]
    publication: ModelPublication,
    model: String,
    camera: String,
}

impl PublishedModel {
    pub fn new(
        publication: ModelPublication,
        model: impl Into<String>,
        camera: impl Into<String>,
    ) -> Self {
        Self {
            publication,
            model: model.into(),
            camera: camera.into(),
        }
    }
    pub fn name(&self) -> &str {
        self.publication.name()
    }
    /// What Score progress asks the scorer for, verbatim.
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn camera(&self) -> &str {
        &self.camera
    }
    pub fn run_id(&self) -> &str {
        self.publication.run_id()
    }
    pub const fn step(&self) -> u64 {
        self.publication.step()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_published_model_reads_as_its_checkpoint_name_selector_and_camera() {
        let model = PublishedModel::new(
            ModelPublication::new("train-1", 3, "Bottle experts"),
            "spiral-robotics-models/0f3c",
            "/top-camera",
        );
        let wire = serde_json::to_value(&model).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "run_id": "train-1", "step": 3, "name": "Bottle experts",
                "model": "spiral-robotics-models/0f3c", "camera": "/top-camera"
            })
        );
        assert_eq!(
            serde_json::from_value::<PublishedModel>(wire).unwrap(),
            model
        );
    }
}
