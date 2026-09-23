//! The HTTP contract between Studio and a training worker.
//!
//! Studio records which runs it started and asks their worker how they are
//! doing. The worker owns SpiralDB reads, model execution, telemetry, and
//! checkpoint publication. Rollout inference has a separate HTTP + Arrow
//! boundary; no framework tensor type crosses this seam.

mod backend;
mod job;

pub use backend::{TrainingBackendError, TrainingBackendInfo};
pub use job::{
    SpiralJobConnection, SpiralServiceEndpoints, TrainingJobHistory, TrainingJobRequest,
    TrainingJobState,
};

include!(concat!(env!("OUT_DIR"), "/training_protocol_version.rs"));

#[cfg(test)]
mod tests {
    use serde::{Serialize, de::DeserializeOwned};

    use super::*;

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
        assert_eq!(
            request_fixture["spiral_schema_version"],
            CURRENT_SPIRAL_SCHEMA_VERSION
        );
        CanonicalWireFixture(include_str!("../fixtures/health.json"))
            .assert_round_trip::<TrainingBackendInfo>();
        CanonicalWireFixture(include_str!("../fixtures/job-request.json"))
            .assert_round_trip::<TrainingJobRequest>();
        CanonicalWireFixture(include_str!("../fixtures/job-history.json"))
            .assert_round_trip::<TrainingJobHistory>();
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
