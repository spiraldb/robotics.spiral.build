//! Recorded-arm selection shared by replay and historical seek actions.

use crate::OperationError;

pub(crate) struct RecordedArms<'a> {
    episode: &'a str,
    arms: &'a [String],
    target: &'a str,
}

impl<'a> RecordedArms<'a> {
    pub(crate) fn new(episode: &'a str, arms: &'a [String], target: &'a str) -> Self {
        Self {
            episode,
            arms,
            target,
        }
    }

    pub(crate) fn select(&self, requested: Option<&str>) -> Result<String, OperationError> {
        if let Some(arm) = requested {
            if !self.arms.iter().any(|recorded| recorded == arm) {
                return Err(OperationError::InvalidInput(format!(
                    "take {:?} did not record an arm named {arm:?}",
                    self.episode
                )));
            }
            return Ok(arm.to_owned());
        }
        if self.arms.iter().any(|recorded| recorded == self.target) {
            return Ok(self.target.to_owned());
        }
        match self.arms {
            [only] => Ok(only.clone()),
            arms => Err(OperationError::InvalidInput(format!(
                "take {:?} recorded {} arms ({}); say which one to use",
                self.episode,
                arms.len(),
                arms.join(", ")
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_then_target_then_single_arm_are_the_selection_precedence() {
        let two = vec!["leader".to_owned(), "follower".to_owned()];
        assert_eq!(
            RecordedArms::new("take", &two, "follower")
                .select(Some("leader"))
                .unwrap(),
            "leader"
        );
        assert_eq!(
            RecordedArms::new("take", &two, "follower")
                .select(None)
                .unwrap(),
            "follower"
        );
        assert_eq!(
            RecordedArms::new("take", &["leader".to_owned()], "follower")
                .select(None)
                .unwrap(),
            "leader"
        );
    }

    #[test]
    fn unknown_or_ambiguous_arms_are_refused() {
        let arms = vec!["left".to_owned(), "right".to_owned()];
        assert!(matches!(
            RecordedArms::new("take", &arms, "follower").select(Some("missing")),
            Err(OperationError::InvalidInput(_))
        ));
        assert!(matches!(
            RecordedArms::new("take", &arms, "follower").select(None),
            Err(OperationError::InvalidInput(_))
        ));
    }
}
