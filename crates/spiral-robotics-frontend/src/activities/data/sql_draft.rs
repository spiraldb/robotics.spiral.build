//! Lifecycle rules for SQL generated from asynchronously loaded settings.

pub(crate) trait GeneratedSqlDraft {
    /// Rebase an untouched generated query when its dataset changes. Hand-edited
    /// SQL remains exactly as the operator wrote it.
    fn rebase_generated(&mut self, previous: &str, next: &str) -> bool;
}

impl GeneratedSqlDraft for String {
    fn rebase_generated(&mut self, previous: &str, next: &str) -> bool {
        if self != previous {
            return false;
        }
        next.clone_into(self);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untouched_fallback_sql_tracks_the_loaded_dataset() {
        let mut current = "SELECT * FROM spiral_robotics.joint_samples".to_owned();
        assert!(current.rebase_generated(
            "SELECT * FROM spiral_robotics.joint_samples",
            "SELECT * FROM robotics.joint_samples"
        ));
        assert_eq!(current, "SELECT * FROM robotics.joint_samples");
    }

    #[test]
    fn operator_sql_is_never_rewritten() {
        let mut current = "SELECT state, action FROM experiments.samples".to_owned();
        assert!(!current.rebase_generated(
            "SELECT * FROM spiral_robotics.joint_samples",
            "SELECT * FROM robotics.joint_samples"
        ));
        assert_eq!(current, "SELECT state, action FROM experiments.samples");
    }
}
