use std::collections::BTreeSet;

#[test]
fn core_exposes_only_domain_modules_and_has_no_outward_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    for forbidden in [
        "spiral-robotics-api",
        "axum",
        "dioxus",
        "tokio",
        "adbc",
        "arrow",
        "nokhwa",
        "serialport",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "core must not depend on outer crate or framework {forbidden:?}"
        );
    }

    let dependencies = manifest
        .split_once("[dependencies]")
        .expect("dependency section")
        .1
        .lines()
        .take_while(|line| !line.trim_start().starts_with('['))
        .filter_map(|line| {
            let line = line.split('#').next()?.trim();
            (!line.is_empty()).then(|| line.split('=').next().unwrap().trim())
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        dependencies,
        BTreeSet::new(),
        "the innermost domain crate must not depend on framework or parser details"
    );

    let library = include_str!("../src/lib.rs");
    let public_modules = library
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("pub mod ")
                .and_then(|module| module.strip_suffix(';'))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        public_modules,
        [
            "arm",
            "calibration",
            "episode",
            "error",
            "identity",
            "operation",
            "query",
            "workflow"
        ]
    );
}

#[test]
fn every_core_source_is_free_of_delivery_frameworks_and_ambient_io() {
    let sources = [
        ("arm.rs", include_str!("../src/arm.rs")),
        ("calibration.rs", include_str!("../src/calibration.rs")),
        ("episode.rs", include_str!("../src/episode.rs")),
        ("error.rs", include_str!("../src/error.rs")),
        ("identity.rs", include_str!("../src/identity.rs")),
        ("lib.rs", include_str!("../src/lib.rs")),
        ("operation.rs", include_str!("../src/operation.rs")),
        ("query.rs", include_str!("../src/query.rs")),
        ("workflow.rs", include_str!("../src/workflow.rs")),
    ];
    let actual = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".rs"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual,
        sources.iter().map(|(name, _)| (*name).to_owned()).collect(),
        "new core modules must be added to this exhaustive architecture audit"
    );

    for (name, source) in sources {
        for forbidden in [
            "axum::",
            "dioxus::",
            "tokio::",
            "serialport::",
            "reqwest::",
            "std::env",
            "std::fs",
            "std::net",
            "std::process",
            "spiral_robotics_api",
            "glam::",
            "roxmltree::",
        ] {
            assert!(
                !source.contains(forbidden),
                "core source {name} depends on outer/ambient detail {forbidden:?}"
            );
        }
        assert!(
            !source.contains("serde")
                && !source.contains("Serialize")
                && !source.contains("Deserialize"),
            "core source {name} regained edge serialization"
        );
    }
}
