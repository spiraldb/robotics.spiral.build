/// Annex B access units require an IDR plus SPS/PPS at each random-access point.
pub struct H264AccessUnit {
    keyframe: bool,
    codec: Option<String>,
}
impl H264AccessUnit {
    /// The bitstream filters that make FFmpeg's H.264 such access units: the
    /// parameter sets repeated at every keyframe, and every access unit
    /// opened by a delimiter, so a stream decodes from any keyframe and splits
    /// into frames at each delimiter.
    pub const FFMPEG_PACKET_FILTER: &str = "dump_extra=freq=keyframe,h264_metadata=aud=insert";
    /// FFmpeg's output options for the H.264 Studio records, after the
    /// encoder is chosen: 4:2:0, no B-frames so packets decode in the order
    /// they arrive, a keyframe every 30 frames, and raw Annex B access units.
    pub const FFMPEG_OUTPUT_OPTIONS: [&str; 10] = [
        "-pix_fmt",
        "yuv420p",
        "-bf",
        "0",
        "-g",
        "30",
        "-bsf:v",
        Self::FFMPEG_PACKET_FILTER,
        "-f",
        "h264",
    ];

    pub fn keyframe(&self) -> bool {
        self.keyframe
    }
    pub fn codec(&self) -> Option<&str> {
        self.codec.as_deref()
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut keyframe = false;
        let mut codec = None;
        let mut picture = false;
        let mut parameters = false;
        for nal in Self::nal_units(bytes) {
            match nal[0] & 0x1f {
                1 => picture = true,
                5 => {
                    keyframe = true;
                    picture = true;
                }
                7 if nal.len() >= 4 => {
                    codec = Some(format!("avc1.{:02x}{:02x}{:02x}", nal[1], nal[2], nal[3]))
                }
                8 => parameters = true,
                _ => {}
            }
        }
        if !picture {
            return Err("Camera packet is not an Annex B H.264 picture".into());
        }
        if keyframe && (codec.is_none() || !parameters) {
            return Err("H.264 keyframe is missing its SPS/PPS decoding parameters".into());
        }
        Ok(Self { keyframe, codec })
    }

    /// Whether the Annex B access unit `bytes` holds an IDR picture, a
    /// point a decoder can start from. Unlike [`Self::parse`], it asks
    /// nothing else of the bytes.
    pub fn holds_idr(bytes: &[u8]) -> bool {
        Self::nal_units(bytes).any(|nal| nal[0] & 0x1f == 5)
    }

    /// Every NAL unit of `bytes` from its header on, found by its start code.
    fn nal_units(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
        (0..bytes.len().saturating_sub(3))
            .filter(move |&index| bytes[index..].starts_with(&[0, 0, 1]))
            .map(move |index| &bytes[index + 3..])
    }
}

#[cfg(test)]
mod tests {
    use super::H264AccessUnit;

    #[test]
    fn an_idr_is_a_keyframe_whatever_else_the_unit_holds() {
        let delimiter = [0, 0, 0, 1, 9, 0xf0];
        let idr = [0, 0, 1, 0x65, 0x88];
        let slice = [0, 0, 1, 0x41, 0x9a];
        let parameters = [0, 0, 0, 1, 0x67, 0x64, 0, 0x1f, 0, 0, 1, 0x68, 0xee];
        let keyframe = [delimiter.as_slice(), &parameters, &idr].concat();
        assert!(H264AccessUnit::holds_idr(&keyframe));
        assert!(H264AccessUnit::parse(&keyframe).unwrap().keyframe());
        // An IDR without its parameter sets is still one, though not a
        // packet a recording keeps.
        let bare = [delimiter.as_slice(), &idr].concat();
        assert!(H264AccessUnit::holds_idr(&bare));
        assert!(H264AccessUnit::parse(&bare).is_err());
        let predicted = [delimiter.as_slice(), &slice].concat();
        assert!(!H264AccessUnit::holds_idr(&predicted));
        assert!(!H264AccessUnit::parse(&predicted).unwrap().keyframe());
        assert!(!H264AccessUnit::holds_idr(&[0, 0, 1]));
    }
}
