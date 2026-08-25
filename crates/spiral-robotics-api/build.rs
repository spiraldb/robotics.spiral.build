use std::path::Path;

#[path = "../spiral-robotics-storage/schema/build.rs"]
mod storage_schema_build;

fn main() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("API crate lives in the workspace crates directory");
    storage_schema_build::generate_schema_version(
        crates.join("spiral-robotics-storage/schema").as_path(),
    );
}
