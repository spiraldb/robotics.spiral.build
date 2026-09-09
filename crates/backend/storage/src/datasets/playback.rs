//! Bounded, exact-member encoded camera reads. Decoding belongs to the browser.
use super::EpisodeTopicQuery;
use crate::{
    StorageManager,
    db::{ArrowColumns, DatabaseSession},
};
use arrow_array::{Array, BinaryArray, BinaryViewArray, LargeBinaryArray, RecordBatch};
use base64::Engine as _;
use futures::StreamExt as _;
use spiral_robotics_common::datasets::{EpisodeTopicRequest, EpisodeVideo, VideoPacket};

const MAX_PACKETS: usize = 18_000;
const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024;

impl StorageManager {
    pub async fn episode_video(
        &self,
        request: EpisodeTopicRequest,
    ) -> Result<EpisodeVideo, String> {
        let sql = format!(
            "SELECT log_time, value.format AS format, value.data AS data FROM spql.mcap.read_topic('{}', '{}') WHERE id = $1 ORDER BY log_time, sequence LIMIT {}",
            request.dataset().sql_name().replace('\'', "''"),
            request.topic().replace('\'', "''"),
            MAX_PACKETS + 1
        );
        let statement = EpisodeTopicQuery::for_sql(&request, sql)?.statement;
        let mut connection = self.project_store().connect().await?;
        let mut stream = connection
            .query(statement)
            .await
            .map_err(|error| error.to_string())?;
        let mut video = CameraPackets::default();
        while let Some(batch) = stream.next().await {
            video.append(&batch.map_err(|error| error.to_string())?)?;
        }
        video.finish()
    }
}

#[derive(Default)]
struct CameraPackets {
    packets: Vec<VideoPacket>,
    codec: Option<String>,
    previous_time: Option<u64>,
    encoded_bytes: usize,
}
impl CameraPackets {
    fn append(&mut self, batch: &RecordBatch) -> Result<(), String> {
        let columns = ArrowColumns::new(batch, "episode camera");
        let data = batch
            .column_by_name("data")
            .ok_or("Camera has no encoded data")?;
        for row in 0..batch.num_rows() {
            if self.packets.len() >= MAX_PACKETS {
                return Err("Camera exceeds the 18,000-packet playback limit".into());
            }
            if columns.text("format", row)? != "h264" {
                return Err("Camera playback currently requires H.264 video".into());
            }
            let timestamp = columns.unsigned("log_time", row)?;
            if self
                .previous_time
                .is_some_and(|previous| timestamp <= previous)
            {
                return Err("Camera timestamps must increase strictly for playback".into());
            }
            let bytes = Self::bytes(data.as_ref(), row)?;
            self.encoded_bytes = self.encoded_bytes.saturating_add(bytes.len());
            if self.encoded_bytes > MAX_ENCODED_BYTES {
                return Err("Camera exceeds the 64 MiB encoded playback limit".into());
            }
            let access = H264AccessUnit::parse(bytes)?;
            if self.packets.is_empty() && !access.keyframe {
                return Err("Camera does not begin with a decodable H.264 keyframe".into());
            }
            if let Some(codec) = access.codec {
                if self
                    .codec
                    .as_ref()
                    .is_some_and(|previous| previous != &codec)
                {
                    return Err(
                        "Camera changes H.264 codec configuration during this episode".into(),
                    );
                }
                self.codec = Some(codec);
            }
            self.packets.push(VideoPacket::new(
                timestamp,
                base64::engine::general_purpose::STANDARD.encode(bytes),
                access.keyframe,
            ));
            self.previous_time = Some(timestamp);
        }
        Ok(())
    }

    fn bytes(array: &dyn Array, row: usize) -> Result<&[u8], String> {
        if array.is_null(row) {
            return Err("Camera packet is null".into());
        }
        if let Some(values) = array.as_any().downcast_ref::<BinaryArray>() {
            return Ok(values.value(row));
        }
        if let Some(values) = array.as_any().downcast_ref::<LargeBinaryArray>() {
            return Ok(values.value(row));
        }
        if let Some(values) = array.as_any().downcast_ref::<BinaryViewArray>() {
            return Ok(values.value(row));
        }
        Err("Camera packet data is not binary".into())
    }

    fn finish(self) -> Result<EpisodeVideo, String> {
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

/// Annex B access units require an IDR plus SPS/PPS at each random-access point.
struct H264AccessUnit {
    keyframe: bool,
    codec: Option<String>,
}
impl H264AccessUnit {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn annex_b_random_access_requires_idr_and_parameter_sets() {
        let packet = [
            0, 0, 0, 1, 0x67, 0x64, 0, 0x1e, 0, 0, 1, 0x68, 0x88, 0, 0, 1, 0x65, 0x88,
        ];
        let access = H264AccessUnit::parse(&packet).unwrap();
        assert!(access.keyframe);
        assert_eq!(access.codec.as_deref(), Some("avc1.64001e"));
        assert!(
            !H264AccessUnit::parse(&[0, 0, 1, 0x41, 0x88])
                .unwrap()
                .keyframe
        );
        assert!(H264AccessUnit::parse(&[0, 0, 1, 0x65, 0x88]).is_err());
        assert!(H264AccessUnit::parse(&[0, 0, 0, 5, 0x65]).is_err());
        assert!(H264AccessUnit::parse(&[]).is_err());
    }
    #[test]
    fn camera_reads_reject_empty_and_unordered_streams() {
        assert!(CameraPackets::default().finish().is_err());
        let schema = std::sync::Arc::new(arrow_schema::Schema::new(vec![
            arrow_schema::Field::new("log_time", arrow_schema::DataType::UInt64, false),
            arrow_schema::Field::new("format", arrow_schema::DataType::Utf8, false),
            arrow_schema::Field::new("data", arrow_schema::DataType::Binary, false),
        ]));
        let key: &[u8] = &[
            0, 0, 1, 0x67, 0x64, 0, 0x1e, 0, 0, 1, 0x68, 0x88, 0, 0, 1, 0x65, 0x88,
        ];
        let batch = RecordBatch::try_new(
            schema,
            vec![
                std::sync::Arc::new(arrow_array::UInt64Array::from(vec![10, 10])),
                std::sync::Arc::new(arrow_array::StringArray::from(vec!["h264", "h264"])),
                std::sync::Arc::new(BinaryArray::from(vec![key, key])),
            ],
        )
        .unwrap();
        assert!(
            CameraPackets::default()
                .append(&batch)
                .unwrap_err()
                .contains("increase strictly")
        );
    }
}
