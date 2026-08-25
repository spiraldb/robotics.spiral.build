use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub(crate) fn generate_protocol_version(
    protocol_directory: &Path,
    storage_schema_descriptor: &Path,
) {
    let descriptor_path = protocol_directory.join("version.json");
    let changelog_path = protocol_directory.join("CHANGELOG.md");
    println!("cargo:rerun-if-changed={}", descriptor_path.display());
    println!("cargo:rerun-if-changed={}", changelog_path.display());
    println!(
        "cargo:rerun-if-changed={}",
        storage_schema_descriptor.display()
    );

    let descriptor = read_json(&descriptor_path, "training protocol version.json");
    require_keys(
        &descriptor,
        &["format_version", "id", "major", "minor", "storage_schema"],
        "training protocol",
    );
    assert_eq!(required_u32(&descriptor, "format_version"), 1);
    let api_id = required_string(&descriptor, "id");
    let api_major = required_u32(&descriptor, "major");
    let api_minor = required_u32(&descriptor, "minor");
    assert!(!api_id.is_empty(), "training API ID must not be empty");
    assert!(api_major > 0, "training API major version must be positive");

    let supported = &descriptor["storage_schema"];
    require_keys(
        supported,
        &["minimum", "maximum"],
        "training protocol storage_schema",
    );
    let schema_minimum = required_u32(supported, "minimum");
    let schema_maximum = required_u32(supported, "maximum");

    let storage = read_json(storage_schema_descriptor, "storage schema version.json");
    require_keys(&storage, &["format_version", "current"], "storage schema");
    assert_eq!(required_u32(&storage, "format_version"), 1);
    let current_schema = required_u32(&storage, "current");
    assert!(
        current_schema > 0,
        "storage schema version must be positive"
    );
    assert!(
        schema_minimum <= current_schema && current_schema <= schema_maximum,
        "current storage schema must be inside the training runtime support range"
    );

    let changelog =
        fs::read_to_string(&changelog_path).expect("reading training protocol changelog");
    assert!(
        changelog.contains(&format!("## Training API {api_major}.{api_minor}")),
        "training protocol changelog must describe the current API version"
    );

    let generated = format!(
        "pub const TRAINING_BACKEND_API_ID: &str = {api_id:?};\n\
         pub const TRAINING_BACKEND_API_VERSION: u32 = {api_major};\n\
         pub const TRAINING_BACKEND_API_REVISION: u32 = {api_minor};\n\
         pub const TRAINING_CURRENT_SPIRAL_SCHEMA_VERSION: u32 = {current_schema};\n\
         pub const TRAINING_BACKEND_MIN_SPIRAL_SCHEMA_VERSION: u32 = {schema_minimum};\n\
         pub const TRAINING_BACKEND_MAX_SPIRAL_SCHEMA_VERSION: u32 = {schema_maximum};\n"
    );
    let output = PathBuf::from(std::env::var("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    fs::write(output.join("training_protocol_version.rs"), generated)
        .expect("writing generated training protocol version");
}

fn read_json(path: &Path, label: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap_or_else(|_| panic!("reading {label}")))
        .unwrap_or_else(|_| panic!("{label} must be valid JSON"))
}

fn required_u32(value: &Value, key: &str) -> u32 {
    value[key]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_else(|| panic!("protocol field {key:?} must be a u32"))
}

fn required_string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("protocol field {key:?} must be a string"))
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
