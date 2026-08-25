use std::path::Path;

#[path = "schema/build.rs"]
mod schema_build;

fn main() {
    schema_build::generate_schema_version(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("schema")
            .as_path(),
    );
}
