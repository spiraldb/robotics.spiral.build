//! Browser-owned unplug-and-compare setup guidance.
pub(crate) struct IdentifiedSerialPort {
    port: Option<String>,
    message: String,
}

impl IdentifiedSerialPort {
    pub(crate) fn port(&self) -> Option<&str> {
        self.port.as_deref()
    }
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn between(before: Vec<String>, after: Vec<String>) -> Self {
        let mut removed = before
            .into_iter()
            .filter(|port| !after.contains(port))
            .collect::<Vec<_>>();
        removed.sort();
        removed.dedup();
        let (port, message) = match removed.as_slice() {
            [port] => (
                Some(port.clone()),
                format!("Found the motor bus on {port}. Plug the cable back in."),
            ),
            [] => (
                None,
                "No port disappeared. Make sure you unplugged the arm, then try again.".to_owned(),
            ),
            ports => (
                None,
                format!(
                    "{} ports disappeared ({}). Unplug only the arm you are setting up.",
                    ports.len(),
                    ports.join(", ")
                ),
            ),
        };
        Self { port, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn identification_explains_zero_one_and_multiple_disconnections() {
        for (before, after, port, guidance) in [
            (vec!["/dev/a"], vec!["/dev/a"], None, "No port disappeared"),
            (
                vec!["/dev/a", "/dev/b"],
                vec!["/dev/b"],
                Some("/dev/a"),
                "Found the motor bus on /dev/a",
            ),
            (
                vec!["/dev/b", "/dev/a", "/dev/a"],
                vec![],
                None,
                "2 ports disappeared (/dev/a, /dev/b)",
            ),
        ] {
            let result = IdentifiedSerialPort::between(
                before.into_iter().map(str::to_owned).collect(),
                after.into_iter().map(str::to_owned).collect(),
            );
            assert_eq!(result.port.as_deref(), port);
            assert!(result.message.contains(guidance));
        }
    }
}
