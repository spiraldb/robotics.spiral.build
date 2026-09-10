/// Annex B access units require an IDR plus SPS/PPS at each random-access point.
pub struct H264AccessUnit {
    keyframe: bool,
    codec: Option<String>,
}
impl H264AccessUnit {
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
        for index in 0..bytes.len().saturating_sub(3) {
            if bytes[index..].starts_with(&[0, 0, 1]) {
                let nal = &bytes[index + 3..];
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
        }
        if !picture {
            return Err("Camera packet is not an Annex B H.264 picture".into());
        }
        if keyframe && (codec.is_none() || !parameters) {
            return Err("H.264 keyframe is missing its SPS/PPS decoding parameters".into());
        }
        Ok(Self { keyframe, codec })
    }
}
