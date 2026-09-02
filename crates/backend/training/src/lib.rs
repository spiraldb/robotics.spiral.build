//! Backend-neutral model-training boundary.
//!
//! Studio owns orchestration and the visible run history. A job backend owns
//! SpiralDB reads, model execution, telemetry, and artifact publication.
//! Rollout inference has a separate HTTP + Arrow boundary; no framework tensor
//! type crosses this training seam.

mod backend;
mod job;

pub use backend::{
    TrainingBackend, TrainingBackendError, TrainingBackendIdentity, TrainingBackendInfo,
};
pub use job::{
    ActLayerCounts, ActModelDimensions, ActTrainingArchitecture, ActTrainingInput,
    ActTrainingJobConfig, SpiralJobConnection, SpiralServiceEndpoints, TrainingCheckpointSchedule,
    TrainingDataColumns, TrainingDataQuery, TrainingDataSelection, TrainingImageSize,
    TrainingInitialization, TrainingJobCheckpoint, TrainingJobCursor, TrainingJobIdentity,
    TrainingJobLog, TrainingJobMetric, TrainingJobPhase, TrainingJobPlan, TrainingJobRequest,
    TrainingJobResolved, TrainingJobSnapshot, TrainingOptimizer, TrainingProgress,
    TrainingRuntimeSelection, TrainingSchedule,
};

include!(concat!(env!("OUT_DIR"), "/training_protocol_version.rs"));

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde::{Serialize, de::DeserializeOwned};

    use super::*;

    struct RemoteBackend;

    impl TrainingBackend for RemoteBackend {
        fn health(&self) -> Result<TrainingBackendInfo, TrainingBackendError> {
            Ok(TrainingBackendInfo::new(
                TrainingBackendIdentity::new("remote.test", "Remote test backend", "HTTP"),
                vec!["remote".to_owned()],
            ))
        }
    }

    #[test]
    fn boundary_is_object_safe_and_remote_implementable() {
        let backend: Arc<dyn TrainingBackend> = Arc::new(RemoteBackend);
        assert_eq!(backend.health().unwrap().id(), "remote.test");
    }

    #[test]
    fn job_connection_debug_output_redacts_the_client_secret() {
        let connection = SpiralJobConnection::new(
            SpiralServiceEndpoints::new("https://api.spiraldb.dev", "https://spfs.spiraldb.dev"),
            "project",
            "spiral_robotics",
        )
        .with_credentials(
            Some("client-id".to_owned()),
            Some("do-not-log-this".to_owned()),
        );
        let debug_output = format!("{connection:?}");
        assert!(debug_output.contains("[redacted]"));
        assert!(!debug_output.contains("do-not-log-this"));
    }

    #[test]
    fn training_image_size_preserves_the_native_resolution_sentinel() {
        TrainingImageSize::new(0, 0);
        TrainingImageSize::new(224, 224);
        assert!(std::panic::catch_unwind(|| TrainingImageSize::new(0, 224)).is_err());
    }

    #[test]
    fn compatibility_uses_major_minor_and_schema_range_rules() {
        let current_schema = TRAINING_BACKEND_MIN_SPIRAL_SCHEMA_VERSION;
        assert_eq!(
            TrainingBackendInfo::test_fixture(
                TRAINING_BACKEND_API_REVISION,
                current_schema,
                current_schema,
            )
            .compatibility_error(current_schema),
            None
        );
        assert_eq!(
            TrainingBackendInfo::test_fixture(
                TRAINING_BACKEND_API_REVISION + 1,
                current_schema,
                current_schema,
            )
            .compatibility_error(current_schema),
            None,
            "a newer additive runtime revision remains compatible"
        );

        let wrong_major = TrainingBackendInfo::test_fixture_with_api_version(
            TRAINING_BACKEND_API_VERSION + 1,
            current_schema,
        );
        assert!(
            wrong_major
                .compatibility_error(current_schema)
                .is_some_and(|error| error.contains("major"))
        );

        assert!(
            TrainingBackendInfo::test_fixture(
                TRAINING_BACKEND_API_REVISION,
                current_schema + 1,
                current_schema + 1,
            )
            .compatibility_error(current_schema)
            .is_some_and(|error| error.contains("Spiral schema"))
        );
    }

    #[test]
    fn canonical_wire_fixtures_round_trip_through_rust_contracts() {
        let health: TrainingBackendInfo =
            serde_json::from_str(include_str!("../fixtures/health.json")).unwrap();
        assert_eq!(health.api_id(), TRAINING_BACKEND_API_ID);
        assert_eq!(health.api_version(), TRAINING_BACKEND_API_VERSION);
        assert_eq!(health.api_revision(), TRAINING_BACKEND_API_REVISION);
        assert_eq!(
            health.spiral_schema_minimum(),
            TRAINING_BACKEND_MIN_SPIRAL_SCHEMA_VERSION
        );
        assert_eq!(
            health.spiral_schema_maximum(),
            TRAINING_BACKEND_MAX_SPIRAL_SCHEMA_VERSION
        );
        let request_fixture: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/job-request.json")).unwrap();
        let storage_schema: serde_json::Value =
            serde_json::from_str(include_str!("../../storage/schema/version.json")).unwrap();
        assert_eq!(
            request_fixture["spiral_schema_version"],
            storage_schema["current"]
        );
        CanonicalWireFixture(include_str!("../fixtures/health.json"))
            .assert_round_trip::<TrainingBackendInfo>();
        CanonicalWireFixture(include_str!("../fixtures/job-request.json"))
            .assert_round_trip::<TrainingJobRequest>();
        CanonicalWireFixture(include_str!("../fixtures/job-snapshot.json"))
            .assert_round_trip::<TrainingJobSnapshot>();
    }

    struct CanonicalWireFixture<'a>(&'a str);

    impl CanonicalWireFixture<'_> {
        fn assert_round_trip<T>(&self)
        where
            T: DeserializeOwned + Serialize,
        {
            let expected: serde_json::Value = serde_json::from_str(self.0).unwrap();
            let contract: T = serde_json::from_value(expected.clone()).unwrap();
            assert_eq!(serde_json::to_value(contract).unwrap(), expected);
        }
    }
}
