use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub(crate) struct SchemaVersionGenerator<'a> {
    schema_directory: &'a Path,
}

impl<'a> SchemaVersionGenerator<'a> {
    pub(crate) fn new(schema_directory: &'a Path) -> Self {
        Self { schema_directory }
    }

    pub(crate) fn generate(&self, visibility: &str) {
        let descriptor_path = self.schema_directory.join("version.json");
        let changelog_path = self.schema_directory.join("CHANGELOG.md");
        println!("cargo:rerun-if-changed={}", descriptor_path.display());
        println!("cargo:rerun-if-changed={}", changelog_path.display());

        let descriptor: Value = serde_json::from_str(
            &fs::read_to_string(&descriptor_path).expect("reading storage schema version.json"),
        )
        .expect("storage schema version.json must be valid JSON");
        descriptor.require_keys(&["format_version", "current"], "storage schema");
        assert_eq!(descriptor.required_u32("format_version"), 1);
        let current = descriptor.required_u32("current");
        assert!(current > 0, "storage schema version must be positive");

        let changelog =
            fs::read_to_string(&changelog_path).expect("reading storage schema changelog");
        assert!(
            changelog.contains(&format!("## Spiral schema {current}")),
            "storage schema changelog must describe the current version"
        );

        let generated = format!(
            "/// Schema version written into every completed episode row.\n\
             ///\n\
             /// Generated from `spiral-robotics-storage/schema/version.json`.\n\
             {visibility} const SCHEMA_VERSION: u32 = {current};\n"
        );
        let output = PathBuf::from(std::env::var("OUT_DIR").expect("Cargo supplies OUT_DIR"));
        fs::write(output.join("storage_schema_version.rs"), generated)
            .expect("writing generated storage schema version");
    }
}

trait SchemaDescriptor {
    fn required_u32(&self, key: &str) -> u32;
    fn require_keys(&self, expected: &[&str], label: &str);
}

impl SchemaDescriptor for Value {
    fn required_u32(&self, key: &str) -> u32 {
        self[key]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or_else(|| panic!("storage schema field {key:?} must be a u32"))
    }

    fn require_keys(&self, expected: &[&str], label: &str) {
        let object = self
            .as_object()
            .unwrap_or_else(|| panic!("{label} must be an object"));
        assert!(
            object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)),
            "{label} must contain exactly {expected:?}"
        );
    }
}
