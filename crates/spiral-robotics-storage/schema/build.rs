use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub(crate) fn generate_schema_version(schema_directory: &Path) {
    let descriptor_path = schema_directory.join("version.json");
    let changelog_path = schema_directory.join("CHANGELOG.md");
    println!("cargo:rerun-if-changed={}", descriptor_path.display());
    println!("cargo:rerun-if-changed={}", changelog_path.display());

    let descriptor: Value = serde_json::from_str(
        &fs::read_to_string(&descriptor_path).expect("reading storage schema version.json"),
    )
    .expect("storage schema version.json must be valid JSON");
    require_keys(
        &descriptor,
        &["format_version", "current"],
        "storage schema",
    );
    assert_eq!(required_u32(&descriptor, "format_version"), 1);
    let current = required_u32(&descriptor, "current");
    assert!(current > 0, "storage schema version must be positive");

    let changelog = fs::read_to_string(&changelog_path).expect("reading storage schema changelog");
    assert!(
        changelog.contains(&format!("## Spiral schema {current}")),
        "storage schema changelog must describe the current version"
    );

    let generated = format!(
        "/// Schema version written into every completed episode row.\n\
         ///\n\
         /// Generated from `spiral-robotics-storage/schema/version.json`.\n\
         pub const SCHEMA_VERSION: u32 = {current};\n"
    );
    let output = PathBuf::from(std::env::var("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    fs::write(output.join("storage_schema_version.rs"), generated)
        .expect("writing generated storage schema version");
}

fn required_u32(value: &Value, key: &str) -> u32 {
    value[key]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_else(|| panic!("storage schema field {key:?} must be a u32"))
}

fn require_keys(value: &Value, expected: &[&str], label: &str) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{label} must be an object"));
    assert!(
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)),
        "{label} must contain exactly {expected:?}"
    );
}
