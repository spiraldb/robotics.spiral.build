use std::path::Path;

#[path = "../../crates/backend/storage/schema/build.rs"]
mod schema_build;

fn main() {
    schema_build::SchemaVersionGenerator::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/backend/storage/schema")
            .as_path(),
    )
    .generate();
}
