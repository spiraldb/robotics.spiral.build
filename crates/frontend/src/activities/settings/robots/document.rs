//! Local robot document authoring. Saves carry the baseline seen by this editor.
use serde_json::{Map, Value, json};
use spiral_robotics_common::robots::document::{RobotDocument, RobotDocumentSave, So101};
use spiral_robotics_common::robots::{CameraBinding, CameraStatus, PortSettings, RobotSummary};

pub(super) struct RobotDraft {
    document: Map<String, Value>,
    baseline: Option<RobotDocument>,
}
impl RobotDraft {
    pub(super) fn new(id: &str, robot_type: &str) -> Self {
        let motors: Map<String, Value> = So101::motors()
            .into_iter()
            .map(|(name, id)| {
                (
                    name.into(),
                    json!({"id": id, "direction": "normal", "model": So101::motor_model()}),
                )
            })
            .collect();
        Self {
            document: json!({
                "schema_version": 1, "robot_id": id.trim(),
                "robot_type": robot_type, "motors": motors,
            })
            .as_object()
            .unwrap()
            .clone(),
            baseline: None,
        }
    }
    pub(super) fn import(bytes: &[u8]) -> Result<Self, String> {
        let document: RobotDocument =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        Ok(Self {
            document: document.object().clone(),
            baseline: None,
        })
    }
    pub(super) fn from_robot(robot: &RobotSummary) -> Result<Self, String> {
        let baseline = robot
            .document()
            .ok_or("Robot configuration is not loaded")?;
        Ok(Self {
            document: baseline.object().clone(),
            baseline: Some(baseline.clone()),
        })
    }
    pub(super) fn with_port(mut self, port: PortSettings) -> Self {
        let object = &mut self.document;
        let mut fields = object
            .get("port")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        fields.insert("path".into(), Value::from(port.path().trim()));
        fields.insert("baud_rate".into(), Value::from(port.baud_rate()));
        object.insert("port".into(), Value::Object(fields));
        self
    }
    pub(super) fn with_cameras(
        mut self,
        bindings: &[CameraBinding],
        discovered: &[CameraStatus],
    ) -> Result<Self, String> {
        let object = &mut self.document;
        let existing = object.get("cameras").and_then(Value::as_object);
        let mut cameras = Map::new();
        for binding in bindings {
            let role = binding.name().trim();
            if cameras.contains_key(role) {
                return Err(format!("Camera role {role:?} is used more than once"));
            }
            let mut fields = existing
                .and_then(|cameras| cameras.get(role))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            fields
                .entry("type")
                .or_insert_with(|| Value::from("opencv"));
            fields.insert("id".into(), Value::from(binding.id().trim()));
            if let Some(camera) = discovered
                .iter()
                .find(|camera| camera.id() == binding.id().trim())
            {
                fields.insert("width".into(), Value::from(camera.width()));
                fields.insert("height".into(), Value::from(camera.height()));
                fields.insert("fps".into(), Value::from(camera.fps()));
            }
            cameras.insert(role.into(), Value::Object(fields));
        }
        object.insert("cameras".into(), Value::Object(cameras));
        Ok(self)
    }
    pub(super) fn save(self) -> Result<RobotDocumentSave, String> {
        let document = RobotDocument::from_object(self.document);
        document.validate()?;
        Ok(RobotDocumentSave::new(document, self.baseline))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn new_and_imported_drafts_validate_before_saving_without_an_existing_baseline() {
        let created = RobotDraft::new(" arm ", "so101_follower").save().unwrap();
        assert_eq!(created.document().id().unwrap(), "arm");
        assert!(created.expected().is_none());
        assert_eq!(
            created.document().object()["motors"]
                .as_object()
                .unwrap()
                .len(),
            6
        );
        let encoded = serde_json::to_vec(created.document()).unwrap();
        let imported = RobotDraft::import(&encoded).unwrap().save().unwrap();
        assert_eq!(imported.document(), created.document());
        assert!(imported.expected().is_none());
        assert!(RobotDraft::new("bad id", "so101_follower").save().is_err());
        assert!(RobotDraft::import(br#"{"robot_id":"arm", "robot_id":"other"}"#).is_err());
        assert!(
            RobotDraft::from_robot(&RobotSummary::new("arm", None, Default::default())).is_err()
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn port_edits_preserve_calibration_extensions_and_the_rendered_baseline() {
        let original = RobotDocument::from_object(json!({"robot_id":"arm", "port":{"path":"/dev/old", "baud_rate":1000000, "vendor_flag":true}, "calibration":{"extension":42}}).as_object().unwrap().clone());
        let robot =
            RobotSummary::new("arm", None, Default::default()).with_document(original.clone());
        let save = RobotDraft::from_robot(&robot)
            .unwrap()
            .with_port(PortSettings::new(" /dev/new ", 2000000))
            .save()
            .unwrap();
        assert_eq!(save.expected(), Some(&original));
        assert_eq!(save.document().object()["port"]["path"], "/dev/new");
        assert_eq!(save.document().object()["port"]["vendor_flag"], true);
        assert_eq!(
            save.document().object()["calibration"],
            original.object()["calibration"]
        );
        assert_eq!(original.object()["port"]["path"], "/dev/old");
    }

    trait CameraEditFixture {
        fn with_camera_assignments(
            &self,
            bindings: &[CameraBinding],
            cameras: &[CameraStatus],
        ) -> Map<String, Value>;
    }
    impl CameraEditFixture for Map<String, Value> {
        fn with_camera_assignments(
            &self,
            bindings: &[CameraBinding],
            cameras: &[CameraStatus],
        ) -> Map<String, Value> {
            let document = RobotDocument::from_object(
                json!({"robot_id": "arm", "cameras": self, "extension": 42})
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            let robot =
                RobotSummary::new("arm", None, Default::default()).with_document(document.clone());
            let request = RobotDraft::from_robot(&robot)
                .unwrap()
                .with_cameras(bindings, cameras)
                .unwrap()
                .save()
                .unwrap();
            assert_eq!(request.expected(), Some(&document));
            assert_eq!(request.document().object()["extension"], 42);
            request.document().object()["cameras"]
                .as_object()
                .unwrap()
                .clone()
        }
    }
    fn status(id: &str) -> CameraStatus {
        CameraStatus::new(
            id,
            "Test camera",
            spiral_robotics_common::robots::CameraStreamFormat::new(1280, 720, 30),
        )
    }

    fn binding(name: &str, id: &str) -> CameraBinding {
        CameraBinding::new(name, id, None)
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn camera_assignments_keep_unknown_fields_and_refresh_resolution() {
        let existing = Map::from_iter([(
            "wrist".to_owned(),
            serde_json::json!({
                "type": "opencv",
                "id": "/dev/video9",
                "width": 320,
                "rotation": 180
            }),
        )]);

        let cameras = existing
            .with_camera_assignments(&[binding("wrist", "/dev/video0")], &[status("/dev/video0")]);

        assert_eq!(cameras["wrist"]["id"], "/dev/video0");
        assert_eq!(cameras["wrist"]["width"], 1280);
        assert_eq!(cameras["wrist"]["height"], 720);
        assert_eq!(cameras["wrist"]["fps"], 30);
        assert_eq!(cameras["wrist"]["rotation"], 180, "unknown fields survive");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_camera_mount_survives_being_reassigned() {
        // Where a camera is bolted is written by hand into the robot file; the
        // studio has no control that sets it, so every camera edit the studio
        // does make has to leave it exactly where it was.
        let existing = Map::from_iter([(
            "wrist".to_owned(),
            serde_json::json!({
                "id": "/dev/video0",
                "mount": { "frame": "gripper_link", "xyz": [0.02, 0.0, 0.04] }
            }),
        )]);

        let cameras = existing.with_camera_assignments(&[binding("wrist", "/dev/video9")], &[]);

        assert_eq!(cameras["wrist"]["id"], "/dev/video9");
        assert_eq!(cameras["wrist"]["mount"]["frame"], "gripper_link");
        assert_eq!(cameras["wrist"]["mount"]["xyz"][2], 0.04);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn an_unknown_device_still_records_the_assignment() {
        let cameras = Map::new().with_camera_assignments(&[binding("top", "/dev/video7")], &[]);

        assert_eq!(cameras["top"]["id"], "/dev/video7");
        assert_eq!(cameras["top"]["type"], "opencv");
        assert!(cameras["top"].get("width").is_none());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn removing_a_camera_drops_it_from_the_stored_object() {
        let existing = Map::from_iter([
            ("wrist".to_owned(), serde_json::json!({"id": "/dev/video0"})),
            ("top".to_owned(), serde_json::json!({"id": "/dev/video1"})),
        ]);

        let cameras = existing.with_camera_assignments(&[binding("top", "/dev/video1")], &[]);

        assert_eq!(cameras.len(), 1);
        assert!(cameras.contains_key("top"));
    }
}
