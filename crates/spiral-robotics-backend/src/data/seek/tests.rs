use std::sync::Mutex;

use super::*;

struct FakePort {
    activity: SeekActivity,
    target: Option<SeekTarget>,
    episode: SeekableEpisode,
    sample: RecordedSample,
    current: bool,
    events: Mutex<Vec<String>>,
    committed: Mutex<Vec<PhysicalSeekPlan>>,
}

impl Default for FakePort {
    fn default() -> Self {
        Self {
            activity: SeekActivity {
                recording: None,
                mode: SeekControlMode::Idle,
            },
            target: Some(SeekTarget {
                id: "follower".to_owned(),
                robot_type: Some("so101_follower".to_owned()),
                connected: true,
                calibrated: true,
                problem: None,
            }),
            episode: SeekableEpisode {
                id: "episode_000001".to_owned(),
                arms: vec!["leader".to_owned(), "follower".to_owned()],
                sample_hz: 60.0,
                problem: None,
                unfinished: false,
            },
            sample: RecordedSample {
                seconds: 0.25,
                frame: 15,
                arms: vec![SampledArm {
                    id: "follower".to_owned(),
                    normalized: BTreeMap::from([
                        ("shoulder".to_owned(), Some(0.5)),
                        ("unused".to_owned(), None),
                    ]),
                }],
            },
            current: true,
            events: Mutex::new(Vec::new()),
            committed: Mutex::new(Vec::new()),
        }
    }
}

impl PhysicalSeekPort for FakePort {
    fn activity(&self) -> SeekActivity {
        self.events.lock().unwrap().push("activity".to_owned());
        self.activity.clone()
    }

    fn target(&self, _robot: &str) -> Result<Option<SeekTarget>, OperationError> {
        self.events.lock().unwrap().push("target".to_owned());
        Ok(self.target.clone())
    }

    fn episode(&self, _episode: &str) -> Result<SeekableEpisode, OperationError> {
        self.events.lock().unwrap().push("episode".to_owned());
        Ok(self.episode.clone())
    }

    fn reserve_seek(&self, _robot: &str) -> u64 {
        self.events.lock().unwrap().push("reserve".to_owned());
        42
    }

    async fn sample_at(
        &self,
        _episode: &str,
        _seconds: f64,
    ) -> Result<RecordedSample, OperationError> {
        self.events.lock().unwrap().push("sample".to_owned());
        Ok(self.sample.clone())
    }

    async fn commit_if_current(&self, plan: PhysicalSeekPlan) -> Result<bool, OperationError> {
        self.events.lock().unwrap().push("commit".to_owned());
        if self.current {
            self.committed.lock().unwrap().push(plan);
        }
        Ok(self.current)
    }
}

fn command() -> SeekEpisodeCommand {
    SeekEpisodeCommand {
        episode: "episode_000001".to_owned(),
        robot: "follower".to_owned(),
        source_arm: None,
        seconds: 0.26,
    }
}

async fn invalid(port: &FakePort) -> OperationError {
    PhysicalSeekActions::new(port)
        .seek(command())
        .await
        .expect_err("seek must be refused")
}

#[tokio::test]
async fn invalid_time_is_refused_before_reading_activity() {
    for seconds in [-1.0, f64::NAN, f64::INFINITY] {
        let port = FakePort::default();
        let error = PhysicalSeekActions::new(&port)
            .seek(SeekEpisodeCommand {
                seconds,
                ..command()
            })
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::InvalidInput(_)));
        assert!(port.events.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn active_or_different_paused_recording_blocks_seek() {
    let mut active = FakePort::default();
    active.activity.recording = Some(ActiveRecording {
        episode: "episode_000001".to_owned(),
        paused: false,
    });
    assert!(matches!(
        invalid(&active).await,
        OperationError::Conflict(_)
    ));
    assert_eq!(active.events.lock().unwrap().as_slice(), ["activity"]);

    let mut different = FakePort::default();
    different.activity.recording = Some(ActiveRecording {
        episode: "episode_000002".to_owned(),
        paused: true,
    });
    assert!(matches!(
        invalid(&different).await,
        OperationError::Conflict(_)
    ));
    assert_eq!(different.events.lock().unwrap().as_slice(), ["activity"]);
}

#[tokio::test]
async fn another_control_mode_blocks_seek_before_target_lookup() {
    let port = FakePort {
        activity: SeekActivity {
            recording: None,
            mode: SeekControlMode::Other,
        },
        ..FakePort::default()
    };
    assert!(matches!(invalid(&port).await, OperationError::Conflict(_)));
    assert_eq!(port.events.lock().unwrap().as_slice(), ["activity"]);
}

#[tokio::test]
async fn target_must_exist_and_be_an_explicit_follower() {
    let missing = FakePort {
        target: None,
        ..FakePort::default()
    };
    assert!(matches!(
        invalid(&missing).await,
        OperationError::InvalidInput(_)
    ));

    for kind in [None, Some("so101_leader".to_owned())] {
        let mut port = FakePort::default();
        port.target.as_mut().unwrap().robot_type = kind;
        assert!(matches!(
            invalid(&port).await,
            OperationError::InvalidInput(_)
        ));
        assert_eq!(
            port.events.lock().unwrap().as_slice(),
            ["activity", "target"]
        );
    }

    let mut uppercase = FakePort::default();
    uppercase.target.as_mut().unwrap().robot_type = Some("SO101_FOLLOWER".to_owned());
    assert_eq!(
        PhysicalSeekActions::new(&uppercase)
            .seek(command())
            .await
            .unwrap()
            .state,
        RecordedSeekState::Seeking
    );
}

#[tokio::test]
async fn follower_must_be_connected_calibrated_and_problem_free() {
    let mut disconnected = FakePort::default();
    disconnected.target.as_mut().unwrap().connected = false;
    assert!(matches!(
        invalid(&disconnected).await,
        OperationError::Conflict(_)
    ));

    let mut uncalibrated = FakePort::default();
    uncalibrated.target.as_mut().unwrap().calibrated = false;
    assert!(matches!(
        invalid(&uncalibrated).await,
        OperationError::Conflict(_)
    ));

    let mut broken = FakePort::default();
    broken.target.as_mut().unwrap().problem = Some("motor missing".to_owned());
    assert!(matches!(
        invalid(&broken).await,
        OperationError::Conflict(_)
    ));
}

#[tokio::test]
async fn broken_finished_episode_is_refused_but_matching_paused_partial_is_allowed() {
    let mut finished = FakePort::default();
    finished.episode.problem = Some("recording did not finish".to_owned());
    assert!(matches!(
        invalid(&finished).await,
        OperationError::InvalidInput(_)
    ));
    assert!(
        !finished
            .events
            .lock()
            .unwrap()
            .contains(&"reserve".to_owned())
    );

    let mut partial = FakePort::default();
    partial.activity.recording = Some(ActiveRecording {
        episode: "episode_000001".to_owned(),
        paused: true,
    });
    partial.episode.problem = Some("recording did not finish".to_owned());
    partial.episode.unfinished = true;
    assert_eq!(
        PhysicalSeekActions::new(&partial)
            .seek(command())
            .await
            .unwrap()
            .state,
        RecordedSeekState::Seeking
    );
}

#[tokio::test]
async fn episode_sample_rate_must_be_finite_and_positive_before_ticketing() {
    for sample_hz in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut port = FakePort::default();
        port.episode.sample_hz = sample_hz;
        assert!(matches!(
            invalid(&port).await,
            OperationError::InvalidInput(_)
        ));
        assert!(!port.events.lock().unwrap().contains(&"reserve".to_owned()));
    }
}

#[tokio::test]
async fn recorded_arm_selection_supports_explicit_target_and_single_fallback() {
    let explicit = PhysicalSeekActions::new(&FakePort::default())
        .seek(SeekEpisodeCommand {
            source_arm: Some("follower".to_owned()),
            ..command()
        })
        .await
        .unwrap();
    assert_eq!(explicit.source_arm, "follower");

    let target = PhysicalSeekActions::new(&FakePort::default())
        .seek(command())
        .await
        .unwrap();
    assert_eq!(target.source_arm, "follower");

    let mut single = FakePort::default();
    single.episode.arms = vec!["leader".to_owned()];
    single.sample.arms[0].id = "leader".to_owned();
    assert_eq!(
        PhysicalSeekActions::new(&single)
            .seek(command())
            .await
            .unwrap()
            .source_arm,
        "leader"
    );
}

#[tokio::test]
async fn unknown_and_ambiguous_source_arms_are_refused_before_ticketing() {
    let unknown = PhysicalSeekActions::new(&FakePort::default())
        .seek(SeekEpisodeCommand {
            source_arm: Some("missing".to_owned()),
            ..command()
        })
        .await
        .unwrap_err();
    assert!(matches!(unknown, OperationError::InvalidInput(_)));

    let mut ambiguous = FakePort::default();
    ambiguous.target.as_mut().unwrap().id = "other".to_owned();
    let command = SeekEpisodeCommand {
        robot: "other".to_owned(),
        ..command()
    };
    let error = PhysicalSeekActions::new(&ambiguous)
        .seek(command)
        .await
        .unwrap_err();
    assert!(matches!(error, OperationError::InvalidInput(_)));
    assert!(
        !ambiguous
            .events
            .lock()
            .unwrap()
            .contains(&"reserve".to_owned())
    );
}

#[tokio::test]
async fn sampled_frame_must_contain_a_finite_nonempty_pose_for_the_source() {
    let mut missing = FakePort::default();
    missing.sample.arms.clear();
    assert!(matches!(
        invalid(&missing).await,
        OperationError::InvalidInput(_)
    ));

    let mut empty = FakePort::default();
    empty.sample.arms[0].normalized = BTreeMap::from([("joint".to_owned(), None)]);
    assert!(matches!(
        invalid(&empty).await,
        OperationError::InvalidInput(_)
    ));

    let mut non_finite = FakePort::default();
    non_finite.sample.arms[0].normalized = BTreeMap::from([("joint".to_owned(), Some(f64::NAN))]);
    assert!(matches!(
        invalid(&non_finite).await,
        OperationError::InvalidInput(_)
    ));

    let mut bad_time = FakePort::default();
    bad_time.sample.seconds = f64::NAN;
    assert!(matches!(
        invalid(&bad_time).await,
        OperationError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn admission_reservation_sampling_and_commit_stay_in_safe_order() {
    let port = FakePort::default();
    let result = PhysicalSeekActions::new(&port)
        .seek(command())
        .await
        .unwrap();

    assert_eq!(result.state, RecordedSeekState::Seeking);
    assert_eq!(result.resolved_seconds, 0.25);
    assert_eq!(result.frame, 15);
    assert_eq!(
        port.events.lock().unwrap().as_slice(),
        [
            "activity", "target", "episode", "reserve", "sample", "commit"
        ]
    );
    let committed = port.committed.lock().unwrap();
    assert_eq!(committed[0].ticket, 42);
    assert_eq!(
        committed[0].pose,
        BTreeMap::from([("shoulder".to_owned(), 0.5)])
    );
}

#[tokio::test]
async fn superseded_ticket_reports_coalesced_without_claiming_motion() {
    let port = FakePort {
        current: false,
        ..FakePort::default()
    };
    assert_eq!(
        PhysicalSeekActions::new(&port)
            .seek(command())
            .await
            .unwrap()
            .state,
        RecordedSeekState::Coalesced
    );
    assert!(port.committed.lock().unwrap().is_empty());
}
