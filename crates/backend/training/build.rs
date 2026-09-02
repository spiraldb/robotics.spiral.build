use std::path::Path;

#[path = "protocol/build.rs"]
mod protocol_build;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let backend = manifest
        .parent()
        .expect("training crate lives in the backend directory");
    protocol_build::ProtocolVersionGenerator::new(
        manifest.join("protocol").as_path(),
        backend.join("storage/schema/version.json").as_path(),
    )
    .generate();
}
