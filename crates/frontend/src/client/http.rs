//! URLs for media and download surfaces.

pub(crate) trait CameraApiPath {
    /// URL of this camera's live MJPEG stream.
    fn camera_stream_url(&self) -> String;
}

impl CameraApiPath for str {
    fn camera_stream_url(&self) -> String {
        format!("/api/cameras/stream.mjpeg?id={}", self.url_component())
    }
}

pub(crate) trait EpisodeApiPath {
    /// Browser-native MP4 endpoint for one segment of this episode's camera stream.
    fn video_url(&self, stream: &str, segment: u32) -> String;
}

impl EpisodeApiPath for str {
    fn video_url(&self, stream: &str, segment: u32) -> String {
        format!(
            "/api/episodes/{}/videos/{}?segment={segment}",
            self.url_component(),
            stream.url_component()
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
        assert_eq!(
            "/dev/video0".camera_stream_url(),
            "/api/cameras/stream.mjpeg?id=%2Fdev%2Fvideo0"
        );
        // macOS reports opaque identifiers that may contain anything at all.
        assert_eq!("cam 1&x=2".url_component(), "cam%201%26x%3D2");
        assert_eq!("plain-id_1.0~".url_component(), "plain-id_1.0~");
        assert_eq!(
            "episode 1".video_url("arm/camera", 2),
            "/api/episodes/episode%201/videos/arm%2Fcamera?segment=2"
        );
    }
}
