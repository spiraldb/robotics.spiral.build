//! What a run trains on: one published curated view, and what the dataset
//! that view belongs to offers the model — its cameras, and the joints a
//! policy's state and action carry.

use std::collections::{BTreeMap, BTreeSet};

use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetJointRoles, DatasetSelection, DatasetTopic};
use spiral_robotics_common::training::{TrainingModel, TrainingSource};
use spiral_robotics_ui::{Fact, FactList, Field, FieldLayout, Notice, NoticeTone, Select};

use super::state::TrainingController;
use crate::activities::curation::CuratedView;
use crate::activities::data::DatasetClient;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::shell::activity::WorkspaceActivity;
use crate::shell::project::ProjectSelection;

/// What every caution about joints tells the operator to do.
const RESOLVE_JOINT_ROLES: &str = "Resolve its joint roles in Data.";

/// What Setup reads to draft a run: the published views, and the dataset of
/// the view chosen. Both are read again each time Training opens, so a view
/// published or joints identified meanwhile are seen.
#[derive(Clone, Copy)]
pub(super) struct TrainingDataReads {
    views: Resource<Result<TrainableViews, String>>,
    inputs: Resource<Result<Option<DatasetInputs>, String>>,
}

impl TrainingDataReads {
    /// The reads `controller`'s draft needs. A newly chosen view reads every
    /// camera of its dataset.
    pub(super) fn use_reads(controller: TrainingController) -> Self {
        let activity = WorkspaceActivity::current();
        let selection = ProjectSelection::current();
        let views = QueryPolicy::on_demand(QueryDomain::Catalog).use_when(
            move || activity.is_active() && selection.live(),
            TrainableViews::read,
        );
        let chosen_dataset = use_memo(move || {
            let config = controller.config();
            let source = config.source();
            (!source.view().is_empty()).then(|| source.dataset().clone())
        });
        let inputs = QueryPolicy::on_demand(QueryDomain::Catalog).use_when(
            move || activity.is_active() && selection.live(),
            move || {
                let dataset = chosen_dataset();
                async move {
                    match dataset {
                        Some(dataset) => DatasetInputs::read(dataset).await.map(Some),
                        None => Ok(None),
                    }
                }
            },
        );
        let reads = Self { views, inputs };
        use_effect(move || {
            let config = controller.config();
            if let Some(Ok(inputs)) = reads.inputs(config.source().dataset()) {
                controller.default_cameras(inputs.cameras().names());
            }
        });
        reads
    }

    pub(super) fn views(self) -> Option<Result<TrainableViews, String>> {
        self.views.read().clone()
    }

    /// What `dataset` offers as far as it is read: `None` while it is,
    /// though another dataset's answer still shows.
    pub(super) fn inputs(
        self,
        dataset: &DatasetSelection,
    ) -> Option<Result<DatasetInputs, String>> {
        match &*self.inputs.read() {
            Some(Ok(Some(read))) if read.dataset() == dataset => Some(Ok(read.clone())),
            Some(Err(error)) => Some(Err(error.clone())),
            _ => None,
        }
    }
}

/// The published views a run can train on: those whose run published the
/// statement a run reads.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TrainableViews(Vec<CuratedView>);

impl TrainableViews {
    async fn read() -> Result<Self, String> {
        let views = CuratedView::all().await?;
        Ok(Self(
            views
                .into_iter()
                .filter(|view| view.statement().is_some())
                .collect(),
        ))
    }

    /// What a run of the view called `name` trains on, if it is listed.
    pub(super) fn source(&self, name: &str) -> Option<TrainingSource> {
        self.0
            .iter()
            .find(|view| view.view() == name)
            .map(|view| TrainingSource::new(view.dataset().clone(), view.view()))
    }

    /// The views' names under the name of the dataset each belongs to.
    fn by_dataset(&self) -> BTreeMap<String, Vec<String>> {
        let mut groups = BTreeMap::<String, Vec<String>>::new();
        for view in &self.0 {
            groups
                .entry(view.dataset().display_name())
                .or_default()
                .push(view.view().to_owned());
        }
        groups
    }
}

/// What a dataset offers a run: its cameras, and the joints its state and
/// action carry.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DatasetInputs {
    dataset: DatasetSelection,
    cameras: DatasetCameras,
    joints: Result<JointSummary, String>,
}

impl DatasetInputs {
    async fn read(dataset: DatasetSelection) -> Result<Self, String> {
        let client = DatasetClient::current();
        let (inspection, roles) = futures_util::future::join(
            client.inspect_dataset(dataset.clone()),
            client.joint_roles(&dataset),
        )
        .await;
        let inspection = inspection.map_err(|error| error.to_string())?;
        let roles = roles.map_err(|error| error.to_string())?;
        Ok(Self {
            cameras: DatasetCameras::recorded(inspection.topics()),
            joints: JointSummary::of(roles.as_ref()),
            dataset,
        })
    }
    pub(super) fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub(super) fn cameras(&self) -> &DatasetCameras {
        &self.cameras
    }
    /// The joints a run of it trains on, or why it cannot be trained on.
    pub(super) fn joints(&self) -> Result<&JointSummary, &str> {
        self.joints.as_ref().map_err(String::as_str)
    }
}

/// A dataset's cameras — its video topics — in the order a run reads them.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DatasetCameras(Vec<String>);

impl DatasetCameras {
    fn recorded(topics: &[DatasetTopic]) -> Self {
        let cameras = topics
            .iter()
            .filter(|topic| topic.is_video())
            .map(|topic| topic.name().to_owned())
            .collect::<BTreeSet<_>>();
        Self(cameras.into_iter().collect())
    }
    pub(super) fn names(&self) -> &[String] {
        &self.0
    }
    /// `chosen` with `camera` turned on or off, in this order. The last
    /// chosen camera stays on: a run reads at least one.
    pub(super) fn toggled(&self, chosen: &[String], camera: &str, on: bool) -> Vec<String> {
        let toggled = self
            .0
            .iter()
            .filter(|name| {
                if name.as_str() == camera {
                    on
                } else {
                    chosen.contains(name)
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        if toggled.is_empty() {
            chosen.to_vec()
        } else {
            toggled
        }
    }
}

/// The joints a run's state and action carry, arm by arm.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct JointSummary {
    state: usize,
    action: usize,
    /// Each arm's joints, as "left 6+gripper".
    arms: Vec<String>,
}

impl JointSummary {
    /// What `roles` give a run to train on, or the fix a dataset needs
    /// before it can be trained on.
    fn of(roles: Option<&DatasetJointRoles>) -> Result<Self, String> {
        let roles = roles.ok_or_else(|| {
            format!("This dataset’s joints aren’t identified. {RESOLVE_JOINT_ROLES}")
        })?;
        let joints = roles
            .policy_joints()
            .map_err(|problem| format!("{problem} {RESOLVE_JOINT_ROLES}"))?;
        let arms = roles
            .arms()
            .iter()
            .map(|arm| {
                let joints = arm.state().indices().len().max(1);
                match arm.gripper() {
                    Some(_) => format!("{} {joints}+gripper", arm.name()),
                    None => format!("{} {joints}", arm.name()),
                }
            })
            .collect();
        Ok(Self {
            state: joints.state_names().len(),
            action: joints.action_names().len(),
            arms,
        })
    }
    fn sizes(&self) -> String {
        format!("State {} · Action {}", self.state, self.action)
    }
    fn arms(&self) -> String {
        self.arms.join(", ")
    }
}

/// The view a run trains on, the dataset it belongs to, and the joints that
/// dataset gives a policy. `inputs` is the chosen view's dataset, `None`
/// while it is read.
#[component]
pub(super) fn TrainingDataSection(
    controller: TrainingController,
    views: Option<Result<TrainableViews, String>>,
    inputs: Option<Result<DatasetInputs, String>>,
    locked: bool,
) -> Element {
    let config = controller.config();
    let source = config.source().clone();
    let listed = matches!(&views, Some(Ok(views)) if views.source(source.view()).is_some());
    // Only a policy reads the joints.
    let policy = matches!(config.model(), TrainingModel::Act(_));
    let joints = match &inputs {
        Some(Ok(inputs)) => inputs.joints().ok().cloned(),
        _ => None,
    };
    let problem = match &inputs {
        Some(Err(error)) => Some(format!("Couldn’t read the dataset. {error}")),
        Some(Ok(inputs)) if policy => inputs.joints().err().map(str::to_owned),
        _ => None,
    };
    rsx! {
        section { class: "training-form-section",
            header { class: "training-form-heading", h3 { "Training data" } }
            match views {
                None => rsx! {
                    p { class: "training-setup-note", role: "status", "Listing published views…" }
                },
                Some(Err(error)) => rsx! {
                    Notice { tone: NoticeTone::Danger, "Couldn’t list the published views. {error}" }
                },
                Some(Ok(views)) if views.0.is_empty() => rsx! {
                    p { class: "training-setup-note", "No published views yet. Publish one in Curation." }
                },
                Some(Ok(views)) => {
                    let groups = views.by_dataset();
                    let chosen = source.view().to_owned();
                    rsx! {
                        Field { label: "View", layout: FieldLayout::Inline,
                            Select {
                                value: chosen.clone(),
                                disabled: locked,
                                onchange: move |name: String| {
                                    if let Some(source) = views.source(&name) {
                                        controller.choose_source(source);
                                    }
                                },
                                if !listed {
                                    option { value: "", disabled: true, selected: true, "Choose a view" }
                                }
                                for (dataset, names) in groups {
                                    optgroup { key: "{dataset}", label: "{dataset}",
                                        for name in names {
                                            option { key: "{name}", value: "{name}", selected: name == chosen, "{name}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if listed {
                FactList { class: "training-setup-facts",
                    Fact { label: "Dataset", "{source.dataset().display_name()}" }
                    if policy {
                        if let Some(summary) = &joints {
                            Fact { label: "Joints",
                                span { "{summary.sizes()}" }
                                small { class: "training-joint-arms", "{summary.arms()}" }
                            }
                        } else if inputs.is_none() {
                            Fact { label: "Joints",
                                span { class: "training-setup-quiet", role: "status", "Reading joint roles…" }
                            }
                        }
                    }
                }
                if let Some(problem) = problem {
                    Notice { tone: NoticeTone::Caution, "{problem}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn arm(name: &str, commanded: bool) -> serde_json::Value {
        let values = |topic: &str| {
            json!({"topic": topic, "schema": "JointState", "field": ["position"],
                "element": [], "indices": [0, 1, 2, 3, 4, 5]})
        };
        let gripper = json!({
            "state": {"topic": "/gripper", "schema": "Gripper", "field": ["opening"], "element": [], "indices": []},
            "action": {"topic": "/gripper-command", "schema": "Gripper", "field": ["opening"], "element": [], "indices": []},
            "open": 1.0, "closed": 0.0,
        });
        json!({
            "name": name,
            "state": values("/joints"),
            "action": if commanded { values("/commands") } else { serde_json::Value::Null },
            "gripper": gripper,
        })
    }

    fn roles(arms: Vec<serde_json::Value>) -> DatasetJointRoles {
        serde_json::from_value(json!({ "arms": arms })).expect("joint roles")
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn two_arms_with_grippers_carry_fourteen_state_and_action_values() {
        let summary = JointSummary::of(Some(&roles(vec![arm("left", true), arm("right", true)])))
            .expect("both arms are commanded");
        assert_eq!(summary.sizes(), "State 14 · Action 14");
        assert_eq!(summary.arms(), "left 6+gripper, right 6+gripper");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn joints_a_run_cannot_train_on_name_the_fix() {
        let uncommanded =
            JointSummary::of(Some(&roles(vec![arm("left", true), arm("right", false)])))
                .expect_err("the right arm records no commands");
        assert!(uncommanded.starts_with("The right arm records no commanded positions."));
        assert!(uncommanded.ends_with(RESOLVE_JOINT_ROLES));
        let unresolved = JointSummary::of(None).expect_err("no roles");
        assert!(unresolved.ends_with(RESOLVE_JOINT_ROLES));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn cameras_keep_the_datasets_order_and_at_least_one() {
        let video = |name: &str| {
            DatasetTopic::new(name.to_owned(), vec!["foxglove.CompressedVideo".to_owned()])
        };
        let cameras = DatasetCameras::recorded(&[
            video("/top-camera"),
            DatasetTopic::new("/joints".to_owned(), vec!["JointState".to_owned()]),
            video("/left-wrist-camera"),
            video("/top-camera"),
        ]);
        assert_eq!(cameras.names(), ["/left-wrist-camera", "/top-camera"]);
        let top = vec!["/top-camera".to_owned()];
        assert_eq!(
            cameras.toggled(&top, "/left-wrist-camera", true),
            ["/left-wrist-camera", "/top-camera"]
        );
        assert_eq!(cameras.toggled(&top, "/top-camera", false), top);
    }
}
