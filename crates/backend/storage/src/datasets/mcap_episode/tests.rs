use super::*;
use crate::datasets::streams::ArrowTypeText;
use prost::Message as _;
use prost_reflect::Value;
use prost_reflect::prost_types::field_descriptor_proto::{Label, Type};
use prost_reflect::prost_types::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    OneofDescriptorProto,
};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_recording::{CameraRecording, EpisodeFixture, McapRecording, Sample};
use std::collections::BTreeMap;

/// What Spiral's MCAP reader made of each stream of Studio's recordings and
/// of the Hugging Face datasets Studio imported, as Arrow prints it.
const JOINTS: &str = "struct<episode_id: string_view, stream_id: string_view, sample_index: uint64, episode_time_ns: uint64, state_valid: bool, action_valid: bool, state_position: struct<values: list<item: struct<value: double> not null> not null>, action_position: struct<values: list<item: struct<value: double> not null> not null>, raw_position: struct<values: list<item: struct<value: int64> not null> not null>, velocity: struct<values: list<item: struct<value: double> not null> not null>, effort: struct<values: list<item: struct<value: double> not null> not null>, follow_error_degrees: double>";
const COMPRESSED_VIDEO: &str = "struct<timestamp: struct<seconds: int64 not null, nanos: int32 not null>, frame_id: string_view not null, data: binary_view not null, format: string_view not null>";
const EPISODE: &str = "struct<episode_id: string_view, dataset_id: string_view, schema_version: uint32, started_ms: int64, sample_hz: double, source: string_view, note: string_view, frames: uint64, duration_ns: uint64, max_follow_error_degrees: double, discarded: bool, recording_gap_after_ns: struct<values: list<item: struct<value: uint64> not null> not null>, recording_gap_duration_ns: struct<values: list<item: struct<value: uint64> not null> not null>>";
const STREAM: &str = "struct<episode_id: string_view, stream_id: string_view, ordinal: uint32, modality: string_view, robot_type: string_view, joint_names: struct<values: list<item: struct<value: string_view> not null> not null>, camera_id: string_view, width: uint32, height: uint32, fps: uint32, start_temperature_celsius: struct<values: list<item: struct<value: double> not null> not null>, start_voltage_volts: struct<values: list<item: struct<value: double> not null> not null>, limit_max_temperature_celsius: struct<values: list<item: struct<value: double> not null> not null>, limit_min_voltage_volts: struct<values: list<item: struct<value: double> not null> not null>, limit_max_voltage_volts: struct<values: list<item: struct<value: double> not null> not null>, limit_max_load: struct<values: list<item: struct<value: double> not null> not null>, peak_temperature_celsius: struct<values: list<item: struct<value: double> not null> not null>, min_voltage_volts: struct<values: list<item: struct<value: double> not null> not null>, peak_load: struct<values: list<item: struct<value: double> not null> not null>, peak_current_amps: struct<values: list<item: struct<value: double> not null> not null>>";
const ARM_STATE: &str = "struct<timestamp: struct<seconds: int64 not null, nanos: int32 not null>, position: list<item: double not null> not null, velocity: list<item: double not null> not null, acceleration: list<item: double not null> not null, torque: list<item: double not null> not null, pose: list<item: double not null> not null>";
const INSTRUCTION: &str = "struct<timestamp: struct<seconds: int64 not null, nanos: int32 not null>, data: string_view not null>";
const CAMERA_INFO: &str = "struct<timestamp: struct<seconds: int64 not null, nanos: int32 not null>, frame_id: string_view not null, width: uint32 not null, height: uint32 not null, distortion_model: string_view not null, D: list<item: double not null> not null, K: list<item: double not null> not null, R: list<item: double not null> not null, P: list<item: double not null> not null>";
const WARP_FRAME: &str = "struct<state: list<item: float not null> not null, actions: list<item: float not null> not null, timestamp: float, frame_index: int64, episode_index: int64, index: int64, task_index: int64>";

fn decode(bytes: &[u8]) -> DecodedEpisode {
    DecodedEpisode::of_mcap(McapFile::held("episode.mcap", bytes).unwrap(), bytes).unwrap()
}

/// Each stream's topic and value type.
fn types(episode: &DecodedEpisode) -> BTreeMap<String, String> {
    episode
        .streams()
        .iter()
        .map(|messages| {
            (
                messages.stream().topic().to_owned(),
                ArrowTypeText::try_from(messages.stream().value_type())
                    .unwrap()
                    .to_string(),
            )
        })
        .collect()
}

#[test]
fn a_rollout_recording_decodes_as_spiral_read_it() {
    let camera = CameraRecording::from_annex_b(
        "follower.camera.top".into(),
        (32, 32, 30),
        (
            include_bytes!("../../../../../frontend/src/activities/episode/fixtures/seek.h264")
                .to_vec(),
            (0..8).map(|index| index * 33_333_333).collect(),
        ),
    )
    .unwrap();
    let recording = EpisodeFixture::new("take", "default")
        .with_arms(vec![spiral_robotics_common::recording::EpisodeArm::new(
            "leader",
            None,
            vec!["joint".into()],
        )])
        .with_timing(1_700_000_000_000, 500.0)
        .with_samples(
            (0..4)
                .map(|index| {
                    Sample::new(
                        f64::from(index) * 0.002,
                        vec![vec![Some(0.5)]],
                        vec![vec![1]],
                    )
                })
                .collect(),
        )
        .with_duration_seconds(0.3)
        .into_episode();
    let episode = decode(&McapRecording::encode(&recording, &[camera]).unwrap());
    let types = types(&episode);
    assert_eq!(types["/studio/joints/leader"], JOINTS);
    assert_eq!(types["/camera/follower.camera.top/video"], COMPRESSED_VIDEO);
    assert_eq!(types["/studio/episode"], EPISODE);
    assert_eq!(types["/studio/streams/follower.camera.top"], STREAM);
    let camera = episode
        .streams()
        .iter()
        .find(|messages| messages.stream().is_video())
        .unwrap();
    assert_eq!(camera.len(), 8);
    assert_eq!(camera.stream().schema(), Some("foxglove.CompressedVideo"));
    let joints = episode
        .streams()
        .iter()
        .find(|messages| messages.stream().topic() == "/studio/joints/leader")
        .unwrap();
    assert_eq!(joints.len(), 4);
    assert_eq!(
        joints.time_span(),
        Some((1_700_000_000_000_000_000, 1_700_000_000_006_000_000))
    );
}

/// One `.proto` file built by hand.
struct ProtoFile {
    file: FileDescriptorProto,
}

impl ProtoFile {
    fn new(syntax: &str) -> Self {
        Self {
            file: FileDescriptorProto {
                name: Some("test.proto".into()),
                package: Some("test".into()),
                syntax: Some(syntax.into()),
                ..Default::default()
            },
        }
    }

    fn message(mut self, name: &str, fields: Vec<FieldDescriptorProto>) -> Self {
        self.file.message_type.push(DescriptorProto {
            name: Some(name.into()),
            field: fields,
            ..Default::default()
        });
        self
    }

    fn bytes(&self) -> Vec<u8> {
        FileDescriptorSet {
            file: vec![self.file.clone()],
        }
        .encode_to_vec()
    }

    fn field(name: &str, number: i32, kind: Type) -> FieldDescriptorProto {
        FieldDescriptorProto {
            name: Some(name.into()),
            number: Some(number),
            label: Some(Label::Optional as i32),
            r#type: Some(kind as i32),
            ..Default::default()
        }
    }

    fn repeated(name: &str, number: i32, kind: Type) -> FieldDescriptorProto {
        FieldDescriptorProto {
            label: Some(Label::Repeated as i32),
            ..Self::field(name, number, kind)
        }
    }

    fn message_field(name: &str, number: i32, message: &str) -> FieldDescriptorProto {
        FieldDescriptorProto {
            type_name: Some(format!(".test.{message}")),
            ..Self::field(name, number, Type::Message)
        }
    }
}

/// An MCAP file of `messages` on `topic`, each built by `build` from its
/// index, in `schema` of `proto`.
fn mcap(channels: &[(&str, &str, &ProtoFile)], messages: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut writer = mcap::Writer::new(std::io::Cursor::new(Vec::new())).unwrap();
    let ids = channels
        .iter()
        .map(|(topic, schema, proto)| {
            let schema = writer
                .add_schema(schema, "protobuf", &proto.bytes())
                .unwrap();
            writer
                .add_channel(schema, topic, "protobuf", &Default::default())
                .unwrap()
        })
        .collect::<Vec<_>>();
    for (sequence, (channel, payload)) in messages.iter().enumerate() {
        let time = 100 + sequence as u64;
        writer
            .write_to_known_channel(
                &mcap::records::MessageHeader {
                    channel_id: ids[*channel],
                    sequence: sequence as u32,
                    log_time: time,
                    publish_time: time + 1,
                },
                payload,
            )
            .unwrap();
    }
    writer.finish().unwrap();
    writer.into_inner().into_inner()
}

fn message(proto: &ProtoFile, name: &str, fields: &[(&str, Value)]) -> Vec<u8> {
    let pool = DescriptorPool::decode(proto.bytes().as_slice()).unwrap();
    let mut message = DynamicMessage::new(pool.get_message_by_name(name).unwrap());
    for (field, value) in fields {
        message.set_field_by_name(field, value.clone());
    }
    message.encode_to_vec()
}

#[test]
fn imported_protobuf_decodes_as_spiral_read_it() {
    let timestamp = ProtoFile::field;
    let abc = ProtoFile::new("proto3")
        .message(
            "Time",
            vec![
                timestamp("seconds", 1, Type::Int64),
                timestamp("nanos", 2, Type::Int32),
            ],
        )
        .message(
            "RobotState",
            vec![
                ProtoFile::message_field("timestamp", 1, "Time"),
                ProtoFile::repeated("position", 2, Type::Double),
                ProtoFile::repeated("velocity", 3, Type::Double),
                ProtoFile::repeated("acceleration", 4, Type::Double),
                ProtoFile::repeated("torque", 5, Type::Double),
                ProtoFile::repeated("pose", 6, Type::Double),
            ],
        )
        .message(
            "Instructions",
            vec![
                ProtoFile::message_field("timestamp", 1, "Time"),
                ProtoFile::field("data", 2, Type::String),
            ],
        )
        .message(
            "CameraCalibration",
            vec![
                ProtoFile::message_field("timestamp", 1, "Time"),
                ProtoFile::field("frame_id", 2, Type::String),
                ProtoFile::field("width", 3, Type::Fixed32),
                ProtoFile::field("height", 4, Type::Uint32),
                ProtoFile::field("distortion_model", 5, Type::String),
                ProtoFile::repeated("D", 6, Type::Double),
                ProtoFile::repeated("K", 7, Type::Double),
                ProtoFile::repeated("R", 8, Type::Double),
                ProtoFile::repeated("P", 9, Type::Double),
            ],
        );
    let warp = ProtoFile::new("proto2").message(
        "Frame",
        vec![
            ProtoFile::repeated("state", 1, Type::Float),
            ProtoFile::repeated("actions", 2, Type::Float),
            ProtoFile::field("timestamp", 3, Type::Float),
            ProtoFile::field("frame_index", 4, Type::Int64),
            ProtoFile::field("episode_index", 5, Type::Int64),
            ProtoFile::field("index", 6, Type::Int64),
            ProtoFile::field("task_index", 7, Type::Int64),
        ],
    );
    let bytes = mcap(
        &[
            ("/left-arm-state", "test.RobotState", &abc),
            ("/instruction", "test.Instructions", &abc),
            ("/top-camera-info", "test.CameraCalibration", &abc),
            ("/lerobot/frame", "test.Frame", &warp),
        ],
        &[
            (
                0,
                message(
                    &abc,
                    "test.RobotState",
                    &[("position", Value::List(vec![Value::F64(0.5)]))],
                ),
            ),
            (
                1,
                message(
                    &abc,
                    "test.Instructions",
                    &[("data", Value::String("pick".into()))],
                ),
            ),
            (2, message(&abc, "test.CameraCalibration", &[])),
            (
                3,
                message(&warp, "test.Frame", &[("frame_index", Value::I64(3))]),
            ),
            (0, message(&abc, "test.RobotState", &[])),
        ],
    );
    let episode = decode(&bytes);
    let types = types(&episode);
    assert_eq!(types["/left-arm-state"], ARM_STATE);
    assert_eq!(types["/instruction"], INSTRUCTION);
    assert_eq!(types["/top-camera-info"], CAMERA_INFO);
    assert_eq!(types["/lerobot/frame"], WARP_FRAME);
    let arm = episode
        .streams()
        .iter()
        .find(|messages| messages.stream().topic() == "/left-arm-state")
        .unwrap();
    // A stream's messages keep their order in the file, and their headers.
    assert_eq!(arm.len(), 2);
    assert_eq!(arm.time_span(), Some((100, 104)));
    let id = EpisodeId::try_from("01900000000070008000000000000001").unwrap();
    let rows = arm.batch(&id).unwrap();
    let publish = rows
        .column_by_name("publish_time")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::UInt64Array>()
        .unwrap()
        .values()
        .to_vec();
    let sequences = rows
        .column_by_name("sequence")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::UInt32Array>()
        .unwrap()
        .values()
        .to_vec();
    assert_eq!((publish, sequences), (vec![101, 105], vec![0, 4]));
    // An unset message field is NULL; an unset implicit scalar its default.
    let value = rows.column_by_name("value").unwrap();
    let value = value
        .as_any()
        .downcast_ref::<arrow_array::StructArray>()
        .unwrap();
    assert!(value.column_by_name("timestamp").unwrap().is_null(0));
    let positions = value
        .column_by_name("position")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::ListArray>()
        .unwrap()
        .iter()
        .map(|items| items.map(|items| items.len()))
        .collect::<Vec<_>>();
    assert_eq!(positions, [Some(1), Some(0)]);
}

#[test]
fn presence_oneofs_enums_and_maps_follow_protobuf() {
    let mut file = ProtoFile::new("proto3");
    file.file
        .enum_type
        .push(prost_reflect::prost_types::EnumDescriptorProto {
            name: Some("Mode".into()),
            value: vec![prost_reflect::prost_types::EnumValueDescriptorProto {
                name: Some("IDLE".into()),
                number: Some(0),
                ..Default::default()
            }],
            ..Default::default()
        });
    let mut choice = ProtoFile::field("label", 2, Type::String);
    choice.oneof_index = Some(0);
    let mut mode = ProtoFile::field("mode", 3, Type::Enum);
    mode.type_name = Some(".test.Mode".into());
    let mut entries = ProtoFile::repeated("gains", 4, Type::Message);
    entries.type_name = Some(".test.Status.GainsEntry".into());
    file.file.message_type.push(DescriptorProto {
        name: Some("Status".into()),
        field: vec![
            ProtoFile::field("count", 1, Type::Uint32),
            choice,
            mode,
            entries,
        ],
        oneof_decl: vec![OneofDescriptorProto {
            name: Some("kind".into()),
            ..Default::default()
        }],
        nested_type: vec![DescriptorProto {
            name: Some("GainsEntry".into()),
            field: vec![
                ProtoFile::field("key", 1, Type::String),
                ProtoFile::field("value", 2, Type::Double),
            ],
            options: Some(prost_reflect::prost_types::MessageOptions {
                map_entry: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    });
    let gains = Value::Map(
        [("b", 2.0), ("a", 1.0)]
            .into_iter()
            .map(|(key, gain)| (prost_reflect::MapKey::String(key.into()), Value::F64(gain)))
            .collect(),
    );
    let bytes = mcap(
        &[("/status", "test.Status", &file)],
        &[(0, message(&file, "test.Status", &[("gains", gains)]))],
    );
    let episode = decode(&bytes);
    assert_eq!(
        types(&episode)["/status"],
        "struct<count: uint32 not null, label: string_view, mode: int32 not null, gains: list<item: struct<key: string_view not null, value: double not null> not null> not null>"
    );
    let rows = episode.streams()[0]
        .batch(&EpisodeId::try_from("01900000000070008000000000000001").unwrap())
        .unwrap();
    let value = rows.column_by_name("value").unwrap();
    let value = value
        .as_any()
        .downcast_ref::<arrow_array::StructArray>()
        .unwrap();
    assert!(value.column_by_name("label").unwrap().is_null(0));
    let gains = value
        .column_by_name("gains")
        .unwrap()
        .as_any()
        .downcast_ref::<arrow_array::ListArray>()
        .unwrap()
        .value(0);
    let keys = gains
        .as_any()
        .downcast_ref::<arrow_array::StructArray>()
        .unwrap()
        .column(0)
        .as_any()
        .downcast_ref::<arrow_array::StringViewArray>()
        .unwrap()
        .iter()
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [Some("a"), Some("b")],
        "a map's entries are in key order"
    );
}

/// One file of `payloads` on one schemaless channel encoded as `encoding`.
fn schemaless(topic: &str, encoding: &str, payloads: &[&[u8]]) -> Vec<u8> {
    let mut writer = mcap::Writer::new(std::io::Cursor::new(Vec::new())).unwrap();
    let channel = writer
        .add_channel(0, topic, encoding, &Default::default())
        .unwrap();
    for (sequence, payload) in payloads.iter().enumerate() {
        writer
            .write_to_known_channel(
                &mcap::records::MessageHeader {
                    channel_id: channel,
                    sequence: sequence as u32,
                    log_time: 1,
                    publish_time: 1,
                },
                payload,
            )
            .unwrap();
    }
    writer.finish().unwrap();
    writer.into_inner().into_inner()
}

#[test]
fn json_is_kept_as_its_bytes_and_other_encodings_are_refused() {
    let episode = decode(&schemaless(
        "/right-arm-state.plot",
        "json",
        &[b"{\"x\":1}", b"{\"x\":2}"],
    ));
    assert_eq!(types(&episode)["/right-arm-state.plot"], "binary_view");
    assert_eq!(episode.streams()[0].stream().encoding(), "json");
    assert_eq!(episode.streams()[0].len(), 2);
    let bytes = schemaless("/scan", "cdr", &[b""]);
    let error = DecodedEpisode::of_mcap(McapFile::held("scan.mcap", &bytes).unwrap(), &bytes)
        .err()
        .unwrap();
    assert!(error.contains("\"cdr\""), "{error}");
}

#[test]
fn a_message_containing_itself_has_no_arrow_type() {
    let tree = ProtoFile::new("proto3")
        .message("Node", vec![ProtoFile::message_field("child", 1, "Node")]);
    let bytes = mcap(
        &[("/tree", "test.Node", &tree)],
        &[(0, message(&tree, "test.Node", &[]))],
    );
    let error = DecodedEpisode::of_mcap(McapFile::held("tree.mcap", &bytes).unwrap(), &bytes)
        .err()
        .unwrap();
    assert!(error.contains("contains itself"), "{error}");
}
