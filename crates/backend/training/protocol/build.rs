use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub(super) struct ProtocolVersionGenerator<'a> {
    protocol_directory: &'a Path,
    storage_schema_descriptor: &'a Path,
}

impl<'a> ProtocolVersionGenerator<'a> {
    pub(super) const fn new(
        protocol_directory: &'a Path,
        storage_schema_descriptor: &'a Path,
    ) -> Self {
        Self {
            protocol_directory,
            storage_schema_descriptor,
        }
    }

    pub(super) fn generate(&self) {
        self.emit_rebuild_inputs();
        let training_protocol = self.read_training_protocol();
        let current_schema = self.read_current_storage_schema();
        assert!(
            (training_protocol.schema_minimum..=training_protocol.schema_maximum)
                .contains(&current_schema),
            "current storage schema must be inside the training runtime support range"
        );
        self.verify_changelog(&training_protocol);
        self.write_generated_constants(&training_protocol);
    }

    fn descriptor_path(&self) -> PathBuf {
        self.protocol_directory.join("version.json")
    }

    fn changelog_path(&self) -> PathBuf {
        self.protocol_directory.join("CHANGELOG.md")
    }

    fn emit_rebuild_inputs(&self) {
        println!(
            "cargo:rerun-if-changed={}",
            self.descriptor_path().display()
        );
        println!("cargo:rerun-if-changed={}", self.changelog_path().display());
        println!(
            "cargo:rerun-if-changed={}",
            self.storage_schema_descriptor.display()
        );
    }

    fn read_training_protocol(&self) -> TrainingProtocolVersion {
        let descriptor =
            JsonDocument::read(&self.descriptor_path(), "training protocol version.json");
        descriptor.require_exact_keys(&[
            "format_version",
            "id",
            "major",
            "minor",
            "storage_schema",
        ]);
        assert_eq!(descriptor.required_u32("format_version"), 1);
        let api_id = descriptor.required_string("id").to_owned();
        let api_major = descriptor.required_u32("major");
        let api_minor = descriptor.required_u32("minor");
        assert!(!api_id.is_empty(), "training API ID must not be empty");
        assert!(api_major > 0, "training API major version must be positive");

        let supported_schemas =
            descriptor.nested("storage_schema", "training protocol storage_schema");
        supported_schemas.require_exact_keys(&["minimum", "maximum"]);
        TrainingProtocolVersion {
            api_id,
            api_major,
            api_minor,
            schema_minimum: supported_schemas.required_u32("minimum"),
            schema_maximum: supported_schemas.required_u32("maximum"),
        }
    }

    fn read_current_storage_schema(&self) -> u32 {
        let storage_schema = JsonDocument::read(
            self.storage_schema_descriptor,
            "storage schema version.json",
        );
        storage_schema.require_exact_keys(&["format_version", "current"]);
        assert_eq!(storage_schema.required_u32("format_version"), 1);
        let current_schema = storage_schema.required_u32("current");
        assert!(
            current_schema > 0,
            "storage schema version must be positive"
        );
        current_schema
    }

    fn verify_changelog(&self, training_protocol: &TrainingProtocolVersion) {
        let changelog =
            fs::read_to_string(self.changelog_path()).expect("reading training protocol changelog");
        assert!(
            changelog.contains(&format!(
                "## Training API {}.{}",
                training_protocol.api_major, training_protocol.api_minor
            )),
            "training protocol changelog must describe the current API version"
        );
    }

    fn write_generated_constants(&self, training_protocol: &TrainingProtocolVersion) {
        let TrainingProtocolVersion {
            api_id,
            api_major,
            api_minor,
            schema_minimum,
            schema_maximum,
        } = training_protocol;
        let generated = format!(
            "pub(crate) const TRAINING_BACKEND_API_ID: &str = {api_id:?};\n\
             pub(crate) const TRAINING_BACKEND_API_VERSION: u32 = {api_major};\n\
             pub(crate) const TRAINING_BACKEND_API_REVISION: u32 = {api_minor};\n\
             #[cfg(any(test, feature = \"test-support\"))]\n\
             pub(crate) const TRAINING_BACKEND_MIN_SPIRAL_SCHEMA_VERSION: u32 = {schema_minimum};\n\
             #[cfg(any(test, feature = \"test-support\"))]\n\
             pub(crate) const TRAINING_BACKEND_MAX_SPIRAL_SCHEMA_VERSION: u32 = {schema_maximum};\n"
        );
        let output_directory =
            PathBuf::from(std::env::var("OUT_DIR").expect("Cargo supplies OUT_DIR"));
        fs::write(
            output_directory.join("training_protocol_version.rs"),
            generated,
        )
        .expect("writing generated training protocol version");
    }
}

struct TrainingProtocolVersion {
    api_id: String,
    api_major: u32,
    api_minor: u32,
    schema_minimum: u32,
    schema_maximum: u32,
}

struct JsonDocument<'a> {
    value: Value,
    label: &'a str,
}

impl<'a> JsonDocument<'a> {
    fn read(path: &Path, label: &'a str) -> Self {
        let source = fs::read_to_string(path).unwrap_or_else(|_| panic!("reading {label}"));
        let value =
            serde_json::from_str(&source).unwrap_or_else(|_| panic!("{label} must be valid JSON"));
        Self { value, label }
    }

    fn required_u32(&self, key: &str) -> u32 {
        self.value[key]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or_else(|| panic!("protocol field {key:?} must be a u32"))
    }

    fn required_string(&self, key: &str) -> &str {
        self.value[key]
            .as_str()
            .unwrap_or_else(|| panic!("protocol field {key:?} must be a string"))
    }

    fn nested<'document>(
        &'document self,
        key: &str,
        label: &'document str,
    ) -> JsonObject<'document> {
        JsonObject {
            value: &self.value[key],
            label,
        }
    }

    fn require_exact_keys(&self, expected: &[&str]) {
        JsonObject {
            value: &self.value,
            label: self.label,
        }
        .require_exact_keys(expected);
    }
}

struct JsonObject<'a> {
    value: &'a Value,
    label: &'a str,
}

impl JsonObject<'_> {
    fn required_u32(&self, key: &str) -> u32 {
        self.value[key]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or_else(|| panic!("protocol field {key:?} must be a u32"))
    }

    fn require_exact_keys(&self, expected: &[&str]) {
        let object = self
            .value
            .as_object()
            .unwrap_or_else(|| panic!("{} must be an object", self.label));
        assert!(
            object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)),
            "{} must contain exactly {expected:?}",
            self.label
        );
    }
}
