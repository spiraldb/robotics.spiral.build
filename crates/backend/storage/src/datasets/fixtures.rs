//! Rollout recordings built whole, for tests that store or decode them.
use spiral_robotics_recording::{CameraRecording, EpisodeFixture, McapRecording, Sample};

pub(crate) struct RolloutFixture;

impl RolloutFixture {
    /// Eight H.264 access units of a 32×32 camera.
    pub(crate) const CAMERA: &[u8] =
        include_bytes!("../../../../frontend/src/activities/episode/fixtures/seek.h264");

    /// Episode `episode`'s MCAP file: twenty joint samples of one arm, 2 ms
    /// apart, with the camera's frames 33 ms apart when `filmed`.
    pub(crate) fn mcap(episode: &str, filmed: bool) -> Vec<u8> {
        let camera = CameraRecording::from_annex_b(
            "top".into(),
            (32, 32, 30),
            (
                Self::CAMERA.to_vec(),
                (0..8).map(|index| index * 33_333_333).collect(),
            ),
        )
        .unwrap();
        let samples = (0..20)
            .map(|index| {
                Sample::new(
                    f64::from(index) * 0.002,
                    vec![vec![Some(f64::from(index) / 20.0)]],
                    vec![vec![i64::from(index)]],
                )
            })
            .collect();
        let recording = EpisodeFixture::new(episode, "demo")
            .with_arms(vec![spiral_robotics_common::recording::EpisodeArm::new(
                "leader",
                None,
                vec!["joint".into()],
            )])
            .with_timing(1_700_000_000_000, 500.0)
            .with_samples(samples)
            .with_duration_seconds(0.3)
            .into_episode();
        let cameras = if filmed { vec![camera] } else { vec![] };
        McapRecording::encode(&recording, &cameras).unwrap()
    }
}
