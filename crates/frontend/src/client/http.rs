//! URLs for media and download surfaces.

use spiral_robotics_common::project::ProjectKey;
use spiral_robotics_common::robot_connector::RELAY_PREFIX;

pub(crate) trait CameraApiPath {
    /// Path of this camera's live MJPEG stream, relayed by Studio from the
    /// robot connector of `project`'s workspace. An `<img>` sends no project
    /// headers, so the query names the project, as Studio's scoped URLs do.
    fn camera_stream_path(&self, project: &ProjectKey) -> String;
}

impl CameraApiPath for str {
    fn camera_stream_path(&self, project: &ProjectKey) -> String {
        format!(
            "{RELAY_PREFIX}/cameras/stream.mjpeg?id={}&endpoint={}&project_id={}",
            self.url_component(),
            project.endpoint().url_component(),
            project.project_id().url_component(),
        )
    }
}

/// Percent-encode a value for use in a query string.
///
/// Camera identifiers are device paths on Linux, so at minimum the separators
/// and reserved characters have to survive the round trip.
pub(crate) trait UrlComponent {
    fn url_component(&self) -> String;
}

impl UrlComponent for str {
    fn url_component(&self) -> String {
        let mut encoded = String::with_capacity(self.len());
        for byte in self.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    encoded.push(byte as char);
                }
                _ => encoded.push_str(&format!("%{byte:02X}")),
            }
        }
        encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn device_paths_survive_the_query_string() {
        assert_eq!("/dev/video0".url_component(), "%2Fdev%2Fvideo0");
        let project = ProjectKey::new("https://spiral.example/api", "bench 1");
        assert_eq!(
            "/dev/video0".camera_stream_path(&project),
            "/api/robot-connector/cameras/stream.mjpeg?id=%2Fdev%2Fvideo0\
             &endpoint=https%3A%2F%2Fspiral.example%2Fapi&project_id=bench%201"
        );
        // macOS reports opaque identifiers that may contain anything at all.
        assert_eq!("cam 1&x=2".url_component(), "cam%201%26x%3D2");
        assert_eq!("plain-id_1.0~".url_component(), "plain-id_1.0~");
    }
}
