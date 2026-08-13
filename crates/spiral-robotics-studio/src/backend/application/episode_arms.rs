//! Recorded-arm selection shared by replay and historical seek actions.

use super::error::ApplicationError;

pub(super) fn select_recorded_arm(
    episode: &str,
    arms: &[String],
    target: &str,
    requested: Option<&str>,
) -> Result<String, ApplicationError> {
    if let Some(arm) = requested {
        if !arms.iter().any(|recorded| recorded == arm) {
            return Err(ApplicationError::InvalidInput(format!(
                "take {episode:?} did not record an arm named {arm:?}"
            )));
        }
        return Ok(arm.to_owned());
    }
    if arms.iter().any(|recorded| recorded == target) {
        return Ok(target.to_owned());
    }
    match arms {
        [only] => Ok(only.clone()),
        arms => Err(ApplicationError::InvalidInput(format!(
            "take {episode:?} recorded {} arms ({}); say which one to use",
            arms.len(),
            arms.join(", ")
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_then_target_then_single_arm_are_the_selection_precedence() {
        let two = vec!["leader".to_owned(), "follower".to_owned()];
        assert_eq!(
            select_recorded_arm("take", &two, "follower", Some("leader")).unwrap(),
            "leader"
        );
        assert_eq!(
            select_recorded_arm("take", &two, "follower", None).unwrap(),
            "follower"
        );
        assert_eq!(
            select_recorded_arm("take", &["leader".to_owned()], "follower", None).unwrap(),
            "leader"
        );
    }

    #[test]
    fn unknown_or_ambiguous_arms_are_refused() {
        let arms = vec!["left".to_owned(), "right".to_owned()];
        assert!(matches!(
            select_recorded_arm("take", &arms, "follower", Some("missing")),
            Err(ApplicationError::InvalidInput(_))
        ));
        assert!(matches!(
            select_recorded_arm("take", &arms, "follower", None),
            Err(ApplicationError::InvalidInput(_))
        ));
    }
}
