use std::path::Path;

#[path = "schema/build.rs"]
mod schema_build;

fn main() {
    schema_build::SchemaVersionGenerator::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("schema")
            .as_path(),
    )
    .generate();
}
