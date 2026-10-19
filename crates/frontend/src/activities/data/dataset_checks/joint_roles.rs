//! Browser-owned joint role discovery: which streams hold each arm's
//! measured and commanded joints and its gripper, over the shared discovery
//! loop. It needs no robot description; when the dataset has one, its
//! bindings are the measured joints, so the model is not asked for them
//! twice. The backend only persists the result.
use super::report_view::SectionAbout;
use crate::activities::data::DatasetClient;
use crate::activities::data::discovery::{DiscoveryEpisode, DiscoveryTask, ToolTurn};
use dioxus::prelude::*;
use serde_json::{Value, json};
use spiral_robotics_common::{
    datasets::{
        ArmRoles, DatasetJointRoles, DatasetJointRolesSave, DatasetRobot, DatasetSelection,
        RecordedValues,
    },
    providers::ProviderMessage,
};
use spiral_robotics_viewport::RobotModel;
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

pub(super) struct JointRoleDiscovery {
    data: DatasetClient,
    /// The dataset's robot model, whose bindings are its measured joints.
    robot: Option<DatasetRobot>,
}

impl JointRoleDiscovery {
    pub(super) fn new(data: DatasetClient, robot: Option<DatasetRobot>) -> Self {
        Self { data, robot }
    }

    /// Resolve and save the roles of `dataset`'s joints.
    pub(super) async fn discover(
        &self,
        dataset: &DatasetSelection,
    ) -> Result<DatasetJointRoles, String> {
        let episode = DiscoveryEpisode::open(self.data.clone(), dataset.clone()).await?;
        let messages = vec![
            ProviderMessage::text("system", include_str!("joint_roles_prompt.txt")),
            ProviderMessage::text(
                "user",
                json!({
                    "streams": episode.topics(),
                    "robot_bindings": self.bindings()?,
                    "documentation": episode.documentation(),
                    "lerobot_features": episode.lerobot_features().await?,
                })
                .to_string(),
            ),
        ];
        episode.converse(self, messages).await
    }

    /// The robot model's bindings, each with its joint's limit, or `null`
    /// without a model.
    fn bindings(&self) -> Result<Value, String> {
        let Some(robot) = &self.robot else {
            return Ok(Value::Null);
        };
        let limits = RobotModel::joint_limits(robot.urdf())?;
        robot
            .bindings()
            .iter()
            .map(|binding| {
                let mut entry = serde_json::to_value(binding).map_err(|error| error.to_string())?;
                entry["limit"] = json!(limits.get(binding.joint()));
                Ok(entry)
            })
            .collect()
    }

    /// Check every role against the episode's messages, and against the
    /// robot model's bindings when there is one.
    async fn prepare(
        &self,
        episode: &DiscoveryEpisode,
        roles: DatasetJointRoles,
    ) -> Result<DatasetJointRoles, String> {
        roles.validate()?;
        let mut samples = BTreeMap::new();
        let mut measured = BTreeSet::new();
        for arm in roles.arms() {
            if arm.action() == Some(arm.state()) {
                return Err(format!(
                    "The {} arm's action must be its commanded positions, not its state",
                    arm.name()
                ));
            }
            let state = arm.state();
            let places = (state.indices().iter().map(|index| format!("[{index}]")))
                .chain(state.members().iter().cloned());
            for place in places {
                if !measured.insert((state.topic(), state.path(), place)) {
                    return Err(format!(
                        "The {} arm shares a joint with another arm",
                        arm.name()
                    ));
                }
            }
            self.check_bound(arm)?;
            for (role, values) in Self::named(arm) {
                let key = (values.topic(), values.schema());
                let evidence = match samples.entry(key) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => {
                        entry.insert(episode.stream_evidence(key.0, key.1).await?)
                    }
                };
                let reads = evidence["samples"].as_array().is_some_and(|rows| {
                    rows.iter().any(|row| {
                        row.get("value")
                            .and_then(|value| values.read(value))
                            .is_some()
                    })
                });
                if !reads {
                    return Err(format!(
                        "The {} arm's {role} does not read a number at every index from the supplied samples",
                        arm.name()
                    ));
                }
            }
        }
        Ok(roles)
    }

    /// With a robot model, an arm's measured joints are joints it binds.
    fn check_bound(&self, arm: &ArmRoles) -> Result<(), String> {
        let Some(robot) = self
            .robot
            .as_ref()
            .filter(|robot| !robot.bindings().is_empty())
        else {
            return Ok(());
        };
        let state = arm.state();
        match state
            .indices()
            .iter()
            .find(|index| state.binding(robot, **index).is_none())
        {
            Some(index) => Err(format!(
                "The {} arm's state index {index} is not a joint the robot model binds; use its bindings",
                arm.name()
            )),
            None => Ok(()),
        }
    }

    /// Every field an arm's roles read, named for errors.
    fn named(arm: &ArmRoles) -> Vec<(&'static str, &RecordedValues)> {
        let mut named = vec![("state", arm.state())];
        named.extend(arm.action().map(|action| ("action", action)));
        if let Some(gripper) = arm.gripper() {
            named.push(("gripper state", gripper.state()));
            named.push(("gripper action", gripper.action()));
        }
        named
    }
}

impl DiscoveryTask for JointRoleDiscovery {
    const NAME: &'static str = "Joint discovery";
    type Outcome = DatasetJointRoles;

    fn tools() -> Vec<Value> {
        let roles = serde_json::to_value(schemars::schema_for!(DatasetJointRoles))
            .expect("a schema serializes");
        vec![
            json!({"type":"function", "function":{"name":"finish", "description":"Save every arm's joint roles, or arms: [] when no stream records joint positions.", "parameters":roles}}),
        ]
    }

    async fn answer(
        &self,
        episode: &DiscoveryEpisode,
        name: &str,
        arguments: Value,
    ) -> ToolTurn<DatasetJointRoles> {
        if name != "finish" {
            return ToolTurn::Answer(Err("Unknown joint discovery tool".into()));
        }
        let roles = match serde_json::from_value::<DatasetJointRoles>(arguments) {
            Ok(roles) => roles,
            Err(error) => return ToolTurn::Answer(Err(error.to_string())),
        };
        let saved = async {
            let roles = self.prepare(episode, roles).await?;
            let inspection = self
                .data
                .inspect_dataset(episode.dataset().clone())
                .await
                .map_err(|error| error.to_string())?;
            self.data
                .save_joint_roles(&DatasetJointRolesSave::new(
                    inspection.dataset().clone(),
                    roles.clone(),
                ))
                .await
                .map_err(|error| error.to_string())?;
            Ok(roles)
        };
        match saved.await {
            Ok(roles) => ToolTurn::Finished(roles),
            Err(error) => ToolTurn::Answer(Err(error)),
        }
    }
}

/// The joint streams the checks read, as the report shows them: which
/// streams were taken for each arm, so an operator can see what the joint
/// checks measured.
pub(super) struct IdentifiedJoints<'a>(&'a DatasetJointRoles);

impl<'a> IdentifiedJoints<'a> {
    pub(super) fn new(roles: &'a DatasetJointRoles) -> Self {
        Self(roles)
    }

    /// Which streams the joint checks read: a sentence, and by arm on demand.
    pub(super) fn about(&self) -> SectionAbout {
        let arms = self.0.arms();
        let names = arms.iter().map(ArmRoles::name).collect::<Vec<_>>();
        let joints = arms
            .iter()
            .map(|arm| arm.state().count().to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(" or ");
        let who = match names.as_slice() {
            [only] => format!("One arm ({only}): {joints} joints"),
            [left, right] => format!("Two arms ({left}, {right}): {joints} joints each"),
            _ => format!("{} arms: {joints} joints each", names.len()),
        };
        let commands = arms.iter().all(|arm| arm.action().is_some());
        let grippers = arms.iter().all(|arm| arm.gripper().is_some());
        let summary = match (commands, grippers) {
            (true, true) => format!("{who}, with commands and grippers"),
            (true, false) => format!("{who}, with commands"),
            (false, true) => format!("{who}, with grippers"),
            (false, false) => who,
        };
        let rows = arms
            .iter()
            .map(|arm| {
                (
                    arm.name().to_owned(),
                    Self::read(arm.state()),
                    arm.action().map_or_else(|| "—".to_owned(), Self::read),
                    arm.gripper().map(|gripper| {
                        (
                            format!(
                                "{} → {}",
                                Self::read(gripper.state()),
                                Self::read(gripper.action())
                            ),
                            gripper.range().map_or_else(
                                || "ends unknown".to_owned(),
                                |(open, closed)| format!("open {open} · closed {closed}"),
                            ),
                        )
                    }),
                )
            })
            .collect::<Vec<_>>();
        SectionAbout::new(summary).with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "Joint streams by arm",
                thead { tr { th { "Arm" } th { "Measured positions" } th { "Commands" } th { "Gripper" } } }
                tbody {
                    for (arm, state, action, gripper) in rows {
                        tr { key: "{arm}",
                            td { class: "data-checks-arm", "{arm}" }
                            td { class: "data-checks-read", "{state}" }
                            td { class: "data-checks-read", "{action}" }
                            td { class: "data-checks-read",
                                match gripper {
                                    Some((read, range)) => rsx! {
                                        span { class: "data-checks-read-line", "{read}" }
                                        span { class: "data-checks-read-note", "{range}" }
                                    },
                                    None => rsx! { "—" },
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    /// Where numbers are read from: `/topic field[0–5].element`, or the
    /// members a LeRobot feature names, `action {shoulder_pan.pos, …}`.
    fn read(values: &RecordedValues) -> String {
        if !values.members().is_empty() {
            let field = values.field().join(".");
            let members = values.members().join(", ");
            return match field.as_str() {
                "" => format!("{} {{{members}}}", values.topic()),
                _ => format!("{} {field}.{{{members}}}", values.topic()),
            };
        }
        let indices = values.indices();
        let picked = match indices {
            [] => String::new(),
            [only] => format!("[{only}]"),
            [first, .., last] if indices.windows(2).all(|pair| pair[1] == pair[0] + 1) => {
                format!("[{first}–{last}]")
            }
            _ => format!(
                "[{}]",
                indices
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        let element = values
            .element()
            .iter()
            .map(|name| format!(".{name}"))
            .collect::<String>();
        format!(
            "{} {}{picked}{element}",
            values.topic(),
            values.field().join(".")
        )
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    fn a_role_reads_as_its_topic_field_and_positions() {
        let values: RecordedValues = serde_json::from_value(json!({
            "topic": "/studio/joints/follower", "schema": "JointSample",
            "field": ["state_position", "values"], "element": ["value"], "indices": [0, 1, 2, 3, 4]
        }))
        .unwrap();
        assert_eq!(
            IdentifiedJoints::read(&values),
            "/studio/joints/follower state_position.values[0–4].value"
        );
    }

    #[wasm_bindgen_test]
    fn a_named_vector_reads_as_its_topic_and_members() {
        let values: RecordedValues = serde_json::from_value(json!({
            "topic": "observation.state", "schema": null, "field": [], "element": [], "indices": [],
            "members": ["shoulder_pan.pos", "shoulder_lift.pos"]
        }))
        .unwrap();
        assert_eq!(
            IdentifiedJoints::read(&values),
            "observation.state {shoulder_pan.pos, shoulder_lift.pos}"
        );
    }

    #[wasm_bindgen_test]
    fn finish_asks_for_every_arm_and_allows_none() {
        let tools = JointRoleDiscovery::tools();
        let schema = &tools[0]["function"]["parameters"];
        assert_eq!(schema["required"], json!(["arms"]));
        assert!(schema["$defs"]["RecordedValues"].is_object());
        let none: DatasetJointRoles = serde_json::from_value(json!({"arms": []})).unwrap();
        assert!(none.arms().is_empty());
    }
}
