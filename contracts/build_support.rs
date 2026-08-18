use std::fs;
use std::path::PathBuf;

use serde_json::Value;

pub enum BoundaryTarget {
    SpiralSchema,
    TrainingApi,
}

pub fn generate_boundary_constants(target: BoundaryTarget) {
    let manifest = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR"),
    );
    let descriptor_path = manifest.join("../../contracts/boundaries.json");
    let changelog_path = manifest.join("../../contracts/CHANGELOG.md");
    println!("cargo:rerun-if-changed={}", descriptor_path.display());
    println!("cargo:rerun-if-changed={}", changelog_path.display());
    let descriptor: Value = serde_json::from_str(
        &fs::read_to_string(&descriptor_path).expect("reading contracts/boundaries.json"),
    )
    .expect("contracts/boundaries.json must be valid JSON");

    require_keys(
        &descriptor,
        &["format_version", "spiral_schema", "training_api"],
        "boundary",
    );
    assert_eq!(required_u32(&descriptor, "format_version"), 1);
    require_keys(&descriptor["spiral_schema"], &["current"], "spiral_schema");
    let spiral_schema = required_u32(&descriptor["spiral_schema"], "current");
    assert!(spiral_schema > 0, "Spiral schema version must be positive");

    let training = &descriptor["training_api"];
    require_keys(
        training,
        &["id", "major", "minor", "spiral_schema"],
        "training_api",
    );
    require_keys(
        &training["spiral_schema"],
        &["minimum", "maximum"],
        "training_api.spiral_schema",
    );
    let api_id = required_string(training, "id");
    let api_major = required_u32(training, "major");
    let api_minor = required_u32(training, "minor");
    let schema_minimum = required_u32(&training["spiral_schema"], "minimum");
    let schema_maximum = required_u32(&training["spiral_schema"], "maximum");
    assert!(!api_id.is_empty(), "training API ID must not be empty");
    assert!(api_major > 0, "training API major version must be positive");
    assert!(
        schema_minimum <= spiral_schema && spiral_schema <= schema_maximum,
        "current Spiral schema must be inside the training runtime support range"
    );
    let changelog = fs::read_to_string(&changelog_path).expect("reading contracts/CHANGELOG.md");
    assert!(
        changelog.contains(&format!("## Training API {api_major}.{api_minor}")),
        "boundary changelog must describe the current training API"
    );
    assert!(
        changelog.contains(&format!("## Spiral schema {spiral_schema}")),
        "boundary changelog must describe the current Spiral schema"
    );

    let (filename, generated) = match target {
        BoundaryTarget::SpiralSchema => (
            "spiral_boundary.rs",
            format!(
                "/// Schema version written into every completed episode row.\n\
                 ///\n\
                 /// Generated from `contracts/boundaries.json`.\n\
                 pub const SCHEMA_VERSION: u32 = {spiral_schema};\n"
            ),
        ),
        BoundaryTarget::TrainingApi => (
            "training_boundary.rs",
            format!(
                "pub const TRAINING_BACKEND_API_ID: &str = {api_id:?};\n\
                 pub const TRAINING_BACKEND_API_VERSION: u32 = {api_major};\n\
                 pub const TRAINING_BACKEND_API_REVISION: u32 = {api_minor};\n\
                 pub const TRAINING_CURRENT_SPIRAL_SCHEMA_VERSION: u32 = {spiral_schema};\n\
                 pub const TRAINING_BACKEND_MIN_SPIRAL_SCHEMA_VERSION: u32 = {schema_minimum};\n\
                 pub const TRAINING_BACKEND_MAX_SPIRAL_SCHEMA_VERSION: u32 = {schema_maximum};\n"
            ),
        ),
    };
    let output = PathBuf::from(std::env::var("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    fs::write(output.join(filename), generated).expect("writing generated boundary constants");
}

fn required_u32(value: &Value, key: &str) -> u32 {
    value[key]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_else(|| panic!("boundary field {key:?} must be a u32"))
}

fn required_string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("boundary field {key:?} must be a string"))
}

fn require_keys(value: &Value, expected: &[&str], label: &str) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("boundary field {label:?} must be an object"));
    assert!(
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key)),
        "boundary field {label:?} must contain exactly {expected:?}"
    );
}
