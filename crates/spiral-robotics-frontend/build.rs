use std::path::Path;

#[path = "../spiral-robotics-storage/schema/build.rs"]
mod storage_schema_build;

fn main() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("frontend crate lives in the workspace crates directory");
    storage_schema_build::SchemaVersionGenerator::new(
        crates.join("spiral-robotics-storage/schema").as_path(),
    )
    .generate("pub(crate)");
}
