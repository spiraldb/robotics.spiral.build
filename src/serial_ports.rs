//! Serial-port discovery helpers.

use std::collections::{BTreeMap, BTreeSet};

/// Return the ports that were present in `before` but not in `after`.
///
/// Results are sorted and duplicate input entries are ignored. This keeps the
/// unplug-and-diff workflow deterministic and makes the hardware-facing CLI
/// straightforward to test.
pub fn disconnected_ports(before: &[String], after: &[String]) -> Vec<String> {
    let before = before.iter().collect::<BTreeSet<_>>();
    let after = after.iter().collect::<BTreeSet<_>>();

    before
        .difference(&after)
        .map(|port| (*port).clone())
        .collect()
}

/// Collapse macOS's paired `/dev/tty.*` and `/dev/cu.*` names.
///
/// `serialport` reports both names for a physical serial device. LeRobot uses
/// the `/dev/tty.*` namespace, so prefer it whenever both aliases are present.
pub fn collapse_macos_aliases(ports: Vec<String>) -> Vec<String> {
    let mut by_device = BTreeMap::<String, String>::new();

    for port in ports {
        let key = port
            .strip_prefix("/dev/cu.")
            .or_else(|| port.strip_prefix("/dev/tty."))
            .map_or_else(|| port.clone(), ToOwned::to_owned);

        by_device
            .entry(key)
            .and_modify(|current| {
                if port.starts_with("/dev/tty.") && !current.starts_with("/dev/tty.") {
                    current.clone_from(&port);
                }
            })
            .or_insert(port);
    }

    let mut collapsed = by_device.into_values().collect::<Vec<_>>();
    collapsed.sort();
    collapsed
}

#[cfg(test)]
mod tests {
    use super::{collapse_macos_aliases, disconnected_ports};

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn finds_one_disconnected_port() {
        let before = strings(&["/dev/ttyACM1", "/dev/ttyACM0"]);
        let after = strings(&["/dev/ttyACM0"]);

        assert_eq!(disconnected_ports(&before, &after), ["/dev/ttyACM1"]);
    }

    #[test]
    fn ignores_new_ports_and_duplicates() {
        let before = strings(&["COM2", "COM2", "COM1"]);
        let after = strings(&["COM1", "COM3"]);

        assert_eq!(disconnected_ports(&before, &after), ["COM2"]);
    }

    #[test]
    fn returns_multiple_ports_in_stable_order() {
        let before = strings(&["COM9", "COM2", "COM4"]);
        let after = strings(&["COM4"]);

        assert_eq!(disconnected_ports(&before, &after), ["COM2", "COM9"]);
    }

    #[test]
    fn reports_no_difference() {
        let ports = strings(&["/dev/ttyUSB0"]);

        assert!(disconnected_ports(&ports, &ports).is_empty());
    }

    #[test]
    fn prefers_tty_alias_on_macos() {
        let ports = strings(&[
            "/dev/cu.usbmodem1",
            "/dev/tty.usbmodem1",
            "/dev/cu.Bluetooth-Incoming-Port",
            "/dev/tty.Bluetooth-Incoming-Port",
        ]);

        assert_eq!(
            collapse_macos_aliases(ports),
            ["/dev/tty.Bluetooth-Incoming-Port", "/dev/tty.usbmodem1"]
        );
    }

    #[test]
    fn retains_an_unpaired_cu_alias() {
        let ports = strings(&["/dev/cu.usbserial-only"]);

        assert_eq!(collapse_macos_aliases(ports), ["/dev/cu.usbserial-only"]);
    }

    #[test]
    fn sorts_mixed_paired_and_unpaired_aliases() {
        let ports = strings(&["/dev/cu.alpha", "/dev/tty.alpha", "/dev/cu.zulu"]);

        assert_eq!(
            collapse_macos_aliases(ports),
            ["/dev/cu.zulu", "/dev/tty.alpha"]
        );
    }
}
