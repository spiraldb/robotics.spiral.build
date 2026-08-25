use std::path::Path;

#[path = "protocol/build.rs"]
mod protocol_build;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = manifest
        .parent()
        .expect("training crate lives in the workspace crates directory");
    protocol_build::ProtocolVersionGenerator::new(
        manifest.join("protocol").as_path(),
        crates
            .join("spiral-robotics-storage/schema/version.json")
            .as_path(),
    )
    .generate();
}
