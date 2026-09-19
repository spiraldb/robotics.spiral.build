use base64::Engine as _;
use spiral_robotics_common::datasets::{EpisodeVideo, H264AccessUnit, VideoPacket};

const MAX_PACKETS: usize = 18_000;
const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct CameraPackets {
    packets: Vec<VideoPacket>,
    codec: Option<String>,
    previous_time: Option<u64>,
    encoded_bytes: usize,
}
impl CameraPackets {
    pub(crate) fn push(&mut self, timestamp: u64, bytes: &[u8]) -> Result<(), String> {
        if self.packets.len() >= MAX_PACKETS {
            return Err("Camera exceeds the 18,000-packet playback limit".into());
        }
        if self
            .previous_time
            .is_some_and(|previous| timestamp <= previous)
        {
            return Err("Camera timestamps must increase strictly for playback".into());
        }
        self.encoded_bytes = self.encoded_bytes.saturating_add(bytes.len());
        if self.encoded_bytes > MAX_ENCODED_BYTES {
            return Err("Camera exceeds the 64 MiB encoded playback limit".into());
        }
        let access = H264AccessUnit::parse(bytes)?;
        if self.packets.is_empty() && !access.keyframe() {
            return Err("Camera does not begin with a decodable H.264 keyframe".into());
        }
        if let Some(codec) = access.codec().map(str::to_owned) {
            if self
                .codec
                .as_ref()
                .is_some_and(|previous| previous != &codec)
            {
                return Err("Camera changes H.264 codec configuration during this episode".into());
            }
            self.codec = Some(codec);
        }
        self.packets.push(VideoPacket::new(
            timestamp,
            base64::engine::general_purpose::STANDARD.encode(bytes),
            access.keyframe(),
        ));
        self.previous_time = Some(timestamp);
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<EpisodeVideo, String> {
        if self.packets.is_empty() {
            return Err("This camera has no video in the selected episode".into());
        }
        Ok(EpisodeVideo::new(
            self.codec
                .ok_or("Camera has no H.264 sequence parameter set")?,
            self.packets,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn annex_b_random_access_requires_idr_and_parameter_sets() {
        let packet = [
            0, 0, 0, 1, 0x67, 0x64, 0, 0x1e, 0, 0, 1, 0x68, 0x88, 0, 0, 1, 0x65, 0x88,
        ];
        let access = H264AccessUnit::parse(&packet).unwrap();
        assert!(access.keyframe());
        assert_eq!(access.codec(), Some("avc1.64001e"));
        assert!(
            !H264AccessUnit::parse(&[0, 0, 1, 0x41, 0x88])
                .unwrap()
                .keyframe()
        );
        assert!(H264AccessUnit::parse(&[0, 0, 1, 0x65, 0x88]).is_err());
        assert!(H264AccessUnit::parse(&[0, 0, 0, 5, 0x65]).is_err());
        assert!(H264AccessUnit::parse(&[]).is_err());
    }
    #[test]
    fn camera_reads_reject_empty_and_unordered_streams() {
        assert!(CameraPackets::default().finish().is_err());
        let key: &[u8] = &[
            0, 0, 1, 0x67, 0x64, 0, 0x1e, 0, 0, 1, 0x68, 0x88, 0, 0, 1, 0x65, 0x88,
        ];
        let mut packets = CameraPackets::default();
        packets.push(10, key).unwrap();
        assert!(
            packets
                .push(10, key)
                .unwrap_err()
                .contains("increase strictly")
        );
    }
}
