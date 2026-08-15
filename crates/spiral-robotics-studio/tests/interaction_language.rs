use std::fs;
use std::path::{Path, PathBuf};

fn files_with_extension(root: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()))
        {
            let path = entry.expect("directory entry should be readable").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|value| value == extension) {
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn product_surfaces_cannot_bypass_the_interaction_language() {
    let studio = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates = studio.parent().expect("Studio should live in crates/");
    let roots = [
        studio.join("src/frontend"),
        crates.join("spiral-robotics-panels/src"),
    ];
    let forbidden = [
        ("button {", "raw Dioxus button"),
        ("r#type: \"file\"", "raw file picker"),
        ("confirm_with_message", "native browser confirmation"),
        ("window.confirm", "native browser confirmation"),
        (
            "variant: ActionVariant::Quiet",
            "an explicit default action variant",
        ),
    ];

    let mut violations = Vec::new();
    for root in roots {
        for path in files_with_extension(&root, "rs") {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
            for (line_index, line) in source.lines().enumerate() {
                if line.contains("fn ") && line.contains("Button(") {
                    violations.push(format!(
                        "{}:{} names a component after its presentation instead of its action",
                        path.display(),
                        line_index + 1,
                    ));
                }
                for (pattern, description) in forbidden {
                    if line.contains(pattern) {
                        violations.push(format!(
                            "{}:{} uses {description}",
                            path.display(),
                            line_index + 1,
                        ));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Use spiral-robotics-ui primitives instead:\n{}",
        violations.join("\n"),
    );
}

#[test]
fn legacy_action_styles_cannot_return() {
    let studio = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let crates = studio.parent().expect("Studio should live in crates/");
    let styles = [
        studio.join("tailwind.css"),
        crates.join("spiral-robotics-panels/assets/panels.css"),
    ];
    let forbidden = [
        ".studio-button",
        ".icon-button",
        ".btn {",
        ".btn-primary",
        ".btn-danger",
        ".inline-safety-confirm",
        "-button",
    ];

    let mut violations = Vec::new();
    for path in styles {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
        for pattern in forbidden {
            if source.contains(pattern) {
                violations.push(format!("{} contains {pattern}", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Legacy action styling bypasses the shared state system:\n{}",
        violations.join("\n"),
    );
}
