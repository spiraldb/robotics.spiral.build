use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::tempdir;

fn calibrate_command(config: &Path, input: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spiral-robotics-calibrate"));
    command
        .arg("--config")
        .arg(config)
        .arg("--from-json")
        .arg(input);
    command
}

fn write_json(path: &Path, value: &Value) {
    let mut bytes = serde_json::to_vec_pretty(value).expect("fixture must serialize");
    bytes.push(b'\n');
    fs::write(path, bytes).expect("fixture must be written");
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("output file must be readable"))
        .expect("output file must contain valid JSON")
}

fn output_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        output_text(&output.stdout),
        output_text(&output.stderr)
    );
}

fn assert_failure(output: &Output) {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        output_text(&output.stdout),
        output_text(&output.stderr)
    );
}

fn two_motor_input() -> Value {
    json!({
        "motors": {
            "elbow": {"center": 500, "range_min": 100, "range_max": 800},
            "shoulder": {"center": 2100, "range_min": 900, "range_max": 3300}
        }
    })
}

fn expected_two_motor_calibration() -> Value {
    json!({
        "format_version": 1,
        "units": "raw",
        "coordinate_space": "unhomed_raw",
        "motors": {
            "elbow": {
                "id": 2,
                "direction": "reversed",
                "range_min": 100,
                "center": 500,
                "range_max": 800
            },
            "shoulder": {
                "id": 1,
                "direction": "normal",
                "range_min": 900,
                "center": 2100,
                "range_max": 3300
            }
        }
    })
}

#[test]
fn creates_a_unified_robot_file_from_json_input() {
    let directory = tempdir().expect("temporary directory");
    let config = directory.path().join("robot.json");
    let input = directory.path().join("readings.json");
    write_json(&input, &two_motor_input());

    let output = calibrate_command(&config, &input)
        .args([
            "--robot-id",
            "fixture-arm",
            "--robot-type",
            "test-follower",
            "--port",
            "fixture-port",
            "--baud-rate",
            "57600",
            "--motor",
            "shoulder=1",
            "--motor",
            "elbow=2:reversed",
        ])
        .output()
        .expect("calibrate command must start");
    assert_success(&output);

    let document = read_json(&config);
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["robot_id"], "fixture-arm");
    assert_eq!(document["robot_type"], "test-follower");
    assert_eq!(
        document["port"],
        json!({"path": "fixture-port", "baud_rate": 57_600})
    );
    assert_eq!(
        document["motors"],
        json!({
            "elbow": {"id": 2, "direction": "reversed"},
            "shoulder": {"id": 1, "direction": "normal"}
        })
    );
    assert_eq!(document["calibration"], expected_two_motor_calibration());
}

#[test]
fn partial_update_preserves_omitted_motors_and_nested_extras() {
    let directory = tempdir().expect("temporary directory");
    let config = directory.path().join("robot.json");
    let input = directory.path().join("readings.json");
    let original = json!({
        "schema_version": 1,
        "robot_id": "existing-arm",
        "robot_type": "existing-type",
        "port": {
            "path": "persisted-port",
            "baud_rate": 1_000_000,
            "transport": "serial",
            "adapter": {"latency_ms": 3}
        },
        "cameras": {"wrist": {"id": "camera-7", "fps": 30}},
        "extension": {"keep": [1, 2, 3]},
        "motors": {
            "gripper": {
                "id": 8,
                "direction": "reversed",
                "model": "sts3215",
                "soft_limits": {"minimum": 12, "maximum": 34}
            },
            "shoulder": {
                "id": 7,
                "direction": "normal",
                "model": "sts3215",
                "protocol": {"version": 0}
            }
        }
    });
    let readings = json!({
        "motors": {
            "gripper": {"center": 25, "range_min": 10, "range_max": 40},
            "shoulder": {"center": 2000, "range_min": 700, "range_max": 3200}
        }
    });
    write_json(&config, &original);
    write_json(&input, &readings);

    let output = calibrate_command(&config, &input)
        .args([
            "--port",
            "updated-port",
            "--baud-rate",
            "115200",
            "--motor",
            "shoulder=17:reversed",
        ])
        .output()
        .expect("calibrate command must start");
    assert_success(&output);

    let updated = read_json(&config);
    assert_eq!(updated["robot_id"], original["robot_id"]);
    assert_eq!(updated["robot_type"], original["robot_type"]);
    assert_eq!(updated["port"]["path"], "updated-port");
    assert_eq!(updated["port"]["baud_rate"], 115_200);
    assert_eq!(updated["port"]["transport"], original["port"]["transport"]);
    assert_eq!(updated["port"]["adapter"], original["port"]["adapter"]);
    assert_eq!(updated["cameras"], original["cameras"]);
    assert_eq!(updated["extension"], original["extension"]);
    assert_eq!(updated["motors"]["gripper"], original["motors"]["gripper"]);
    assert_eq!(updated["motors"]["shoulder"]["id"], 17);
    assert_eq!(updated["motors"]["shoulder"]["direction"], "reversed");
    assert_eq!(
        updated["motors"]["shoulder"]["model"],
        original["motors"]["shoulder"]["model"]
    );
    assert_eq!(
        updated["motors"]["shoulder"]["protocol"],
        original["motors"]["shoulder"]["protocol"]
    );
    assert_eq!(updated["calibration"]["format_version"], 1);
    assert_eq!(updated["calibration"]["units"], "raw");
    assert_eq!(updated["calibration"]["coordinate_space"], "unhomed_raw");
    assert_eq!(
        updated["calibration"]["motors"]["gripper"]["direction"],
        "reversed"
    );
    assert_eq!(
        updated["calibration"]["motors"]["shoulder"]["direction"],
        "reversed"
    );
    assert_eq!(updated["calibration"]["motors"]["shoulder"]["id"], 17);
}

#[test]
fn dry_run_prints_only_json_and_does_not_modify_the_destination() {
    let directory = tempdir().expect("temporary directory");
    let config = directory.path().join("robot.json");
    let input = directory.path().join("readings.json");
    let original = json!({
        "schema_version": 1,
        "sentinel": {"must_remain": true},
        "motors": {"joint": {"id": 9, "direction": "normal"}}
    });
    let readings = json!({
        "motors": {"joint": {"center": 0, "range_min": -10, "range_max": 20}}
    });
    write_json(&config, &original);
    write_json(&input, &readings);
    let before = fs::read(&config).expect("config must be readable");

    let output = calibrate_command(&config, &input)
        .arg("--dry-run")
        .output()
        .expect("calibrate command must start");
    assert_success(&output);
    assert!(output.stderr.is_empty(), "dry-run wrote to stderr");

    let rendered: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "dry-run stdout was not pure JSON: {error}\n{}",
            output_text(&output.stdout)
        )
    });
    assert_eq!(rendered["sentinel"], original["sentinel"]);
    assert_eq!(rendered["calibration"]["format_version"], 1);
    assert_eq!(rendered["calibration"]["units"], "raw");
    assert_eq!(rendered["calibration"]["coordinate_space"], "unhomed_raw");
    assert_eq!(
        rendered["calibration"]["motors"]["joint"],
        json!({
            "id": 9,
            "direction": "normal",
            "range_min": -10,
            "center": 0,
            "range_max": 20
        })
    );
    assert_eq!(
        fs::read(&config).expect("config must remain readable"),
        before
    );
}

#[test]
fn existing_non_null_calibration_requires_force_and_is_not_modified() {
    let directory = tempdir().expect("temporary directory");
    let config = directory.path().join("robot.json");
    let input = directory.path().join("readings.json");
    let original = json!({
        "schema_version": 1,
        "motors": {"joint": {"id": 1, "direction": "normal"}},
        "calibration": {
            "format_version": 1,
            "units": "raw",
            "coordinate_space": "unhomed_raw",
            "motors": {
                "joint": {
                    "id": 1,
                    "direction": "normal",
                    "range_min": 1,
                    "center": 2,
                    "range_max": 3
                }
            }
        }
    });
    let readings = json!({
        "motors": {"joint": {"center": 20, "range_min": 10, "range_max": 30}}
    });
    write_json(&config, &original);
    write_json(&input, &readings);
    let before = fs::read(&config).expect("config must be readable");

    let output = calibrate_command(&config, &input)
        .output()
        .expect("calibrate command must start");
    assert_failure(&output);
    let stderr = output_text(&output.stderr);
    assert!(stderr.contains("already contains calibration"), "{stderr}");
    assert!(stderr.contains("--force"), "{stderr}");
    assert_eq!(
        fs::read(&config).expect("config must remain readable"),
        before
    );
}

#[test]
fn invalid_input_fails_without_modifying_the_destination() {
    let directory = tempdir().expect("temporary directory");
    let config = directory.path().join("robot.json");
    let input = directory.path().join("invalid-readings.json");
    let original = json!({
        "schema_version": 1,
        "sentinel": "unchanged",
        "motors": {"joint": {"id": 1, "direction": "normal"}}
    });
    let invalid = json!({
        "motors": {"joint": {"center": 10, "range_min": 10, "range_max": 30}}
    });
    write_json(&config, &original);
    write_json(&input, &invalid);
    let before = fs::read(&config).expect("config must be readable");

    let output = calibrate_command(&config, &input)
        .output()
        .expect("calibrate command must start");
    assert_failure(&output);
    assert!(
        output_text(&output.stderr).contains("invalid range"),
        "unexpected stderr: {}",
        output_text(&output.stderr)
    );
    assert_eq!(
        fs::read(&config).expect("config must remain readable"),
        before
    );
}

#[test]
fn null_calibration_is_treated_as_absent_without_force() {
    let directory = tempdir().expect("temporary directory");
    let config = directory.path().join("robot.json");
    let input = directory.path().join("readings.json");
    let original = json!({
        "schema_version": 1,
        "motors": {"joint": {"id": 1, "direction": "reversed", "model": "fixture"}},
        "calibration": null
    });
    let readings = json!({
        "motors": {"joint": {"center": 50, "range_min": 10, "range_max": 80}}
    });
    write_json(&config, &original);
    write_json(&input, &readings);

    let output = calibrate_command(&config, &input)
        .output()
        .expect("calibrate command must start");
    assert_success(&output);

    let updated = read_json(&config);
    assert_eq!(updated["motors"], original["motors"]);
    assert_eq!(updated["calibration"]["format_version"], 1);
    assert_eq!(updated["calibration"]["units"], "raw");
    assert_eq!(updated["calibration"]["coordinate_space"], "unhomed_raw");
    assert_eq!(
        updated["calibration"]["motors"]["joint"],
        json!({
            "id": 1,
            "direction": "reversed",
            "range_min": 10,
            "center": 50,
            "range_max": 80
        })
    );
}
