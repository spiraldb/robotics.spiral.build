//! Camera discovery helpers.

/// Build a filesystem-safe suffix from a camera identifier.
pub fn sanitize_camera_identifier(identifier: &str) -> String {
    let sanitized = identifier
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();

    if sanitized.is_empty() {
        "unknown".to_owned()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_camera_identifier;

    #[test]
    fn makes_camera_identifiers_safe_for_paths() {
        assert_eq!(
            sanitize_camera_identifier("/dev/video0\\stream:main?"),
            "_dev_video0_stream_main_"
        );
    }

    #[test]
    fn gives_empty_identifiers_a_name() {
        assert_eq!(sanitize_camera_identifier(""), "unknown");
    }
}
