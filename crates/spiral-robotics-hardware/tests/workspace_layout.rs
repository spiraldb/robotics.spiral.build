use std::fs;
use std::path::{Path, PathBuf};

fn collect_rust_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("workspace directory must be readable") {
        let entry = entry.expect("workspace entry must be readable");
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry
            .file_type()
            .expect("workspace entry type must be readable")
            .is_dir()
        {
            if !name.starts_with('.') && !matches!(name.as_ref(), "node_modules" | "target") {
                collect_rust_sources(&path, sources);
            }
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            sources.push(path);
        }
    }
}

#[test]
fn all_first_party_rust_code_lives_in_crates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("spiral-robotics-hardware must live under the workspace crates directory");
    let crates = root.join("crates");
    let mut sources = Vec::new();
    collect_rust_sources(root, &mut sources);

    let misplaced = sources
        .into_iter()
        .filter(|source| !source.starts_with(&crates))
        .collect::<Vec<_>>();
    assert!(
        misplaced.is_empty(),
        "first-party Rust source must live under crates/: {misplaced:?}"
    );

    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("workspace manifest");
    assert!(
        !manifest.contains("\n[package]\n"),
        "the workspace root must remain a virtual manifest"
    );
    assert!(
        !manifest.contains("\n[patch."),
        "the workspace must not carry an in-repository dependency patch"
    );
}
