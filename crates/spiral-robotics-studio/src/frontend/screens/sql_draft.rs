//! Lifecycle rules for SQL generated from asynchronously loaded settings.

/// Rebase an untouched generated query when its dataset changes. Hand-edited
/// SQL remains exactly as the operator wrote it.
pub(crate) fn rebase_generated_sql(current: &mut String, previous: &str, next: &str) -> bool {
    if current != previous {
        return false;
    }
    next.clone_into(current);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untouched_fallback_sql_tracks_the_loaded_dataset() {
        let mut current = "SELECT * FROM spiral_robotics.joint_samples".to_owned();
        assert!(rebase_generated_sql(
            &mut current,
            "SELECT * FROM spiral_robotics.joint_samples",
            "SELECT * FROM robotics.joint_samples"
        ));
        assert_eq!(current, "SELECT * FROM robotics.joint_samples");
    }

    #[test]
    fn operator_sql_is_never_rewritten() {
        let mut current = "SELECT state, action FROM experiments.samples".to_owned();
        assert!(!rebase_generated_sql(
            &mut current,
            "SELECT * FROM spiral_robotics.joint_samples",
            "SELECT * FROM robotics.joint_samples"
        ));
        assert_eq!(current, "SELECT state, action FROM experiments.samples");
    }
}
