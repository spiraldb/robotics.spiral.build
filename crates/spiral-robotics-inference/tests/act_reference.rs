#![cfg(feature = "cpu")]

use sha2::{Digest, Sha256};
use spiral_robotics_inference::act::{REFERENCE_WEIGHTS_SHA256, load_cpu, reference_model_files};
use spiral_robotics_inference::{ImageView, InferenceInput, Policy};

const EXPECTED_FIRST_ACTION: [f32; 14] = [
    -0.007_579_928_3,
    -0.489_719_87,
    0.903_965_95,
    -0.083_694_875,
    -0.415_729_88,
    -0.032_622_99,
    0.198_886_72,
    -0.054_157_84,
    -0.322_813_18,
    0.680_633_8,
    -0.130_204_41,
    -0.142_818_81,
    -0.052_808_926,
    0.725_361_47,
];
const EXPECTED_MILLIACTION_SHA256: &str =
    "529edcc5d9df02fd8b925ac6ab18cef9c1c8164f9b6f7dfe52558ee0eefda5ee";

#[test]
#[ignore = "requires the pinned 207 MB ACT checkpoint; set SPIRAL_ACT_MODEL_DIR"]
fn pinned_checkpoint_matches_reference_output() {
    let model_dir = std::env::var_os("SPIRAL_ACT_MODEL_DIR")
        .expect("SPIRAL_ACT_MODEL_DIR must point to the pinned ACT model directory");
    let files = reference_model_files(model_dir).expect("pinned model files must verify");
    let mut policy = load_cpu(&files).expect("checkpoint must load");
    assert_eq!(
        policy.identity().weights_sha256(),
        Some(REFERENCE_WEIGHTS_SHA256)
    );
    let specification = policy.specification().clone();
    let image_spec = &specification.image_inputs[0];
    let state = vec![0.0; specification.state_dimensions];
    let pixels = vec![0; image_spec.width * image_spec.height * image_spec.channels];
    let image = ImageView::packed_rgb8(
        &image_spec.name,
        image_spec.width,
        image_spec.height,
        &pixels,
    )
    .unwrap();

    let chunk = policy
        .infer(InferenceInput {
            state: &state,
            images: &[image],
        })
        .expect("inference must succeed");

    assert_eq!(chunk.steps(), 100);
    assert_eq!(chunk.action_dimensions(), 14);
    assert!(chunk.as_slice().iter().all(|value| value.is_finite()));
    assert_eq!(
        milliaction_sha256(chunk.as_slice()),
        EXPECTED_MILLIACTION_SHA256,
        "all 1,400 values must match the independent PyTorch reference to 1e-3"
    );
    for (actual, expected) in chunk.action(0).unwrap().iter().zip(EXPECTED_FIRST_ACTION) {
        assert!(
            (actual - expected).abs() <= 2.0e-5,
            "action mismatch: expected {expected}, got {actual}"
        );
    }
}

fn milliaction_sha256(values: &[f32]) -> String {
    let mut digest = Sha256::new();
    for value in values {
        let quantized = (value * 1_000.0).round() as i32;
        digest.update(quantized.to_le_bytes());
    }
    format!("{:x}", digest.finalize())
}
