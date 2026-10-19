//! Which of a dataset's streams a statement reads, found by what Studio names
//! a stream by: a topic and the schema it was recorded with, or a camera
//! topic. Curations, joint roles and robots name topics; only a stream names
//! a `messages` table.
use super::DatasetTopic;

/// The streams of one dataset, or of one episode of it, as its `topics`
/// table lists them.
#[derive(Clone, Copy)]
pub struct DatasetStreams<'a> {
    topics: &'a [DatasetTopic],
}

impl<'a> DatasetStreams<'a> {
    pub fn new(topics: &'a [DatasetTopic]) -> Self {
        Self { topics }
    }

    /// The stream of `topic` recorded as `schema`, or, with no schema, the
    /// topic's stream without one.
    pub fn recorded(self, topic: &str, schema: Option<&str>) -> Result<&'a DatasetTopic, String> {
        self.only(topic, |stream| stream.is_recorded_as(topic, schema))
    }

    /// The camera stream of `topic`.
    pub fn camera(self, topic: &str) -> Result<&'a DatasetTopic, String> {
        self.only(topic, |stream| stream.name() == topic && stream.is_video())
    }

    /// The one stream of `topic`, whatever it was recorded as.
    pub fn topic(self, topic: &str) -> Result<&'a DatasetTopic, String> {
        self.only(topic, |stream| stream.name() == topic)
    }

    /// The one stream `wanted` picks; a topic recorded as two streams the
    /// way it names them is not one.
    fn only(
        self,
        topic: &str,
        wanted: impl Fn(&DatasetTopic) -> bool,
    ) -> Result<&'a DatasetTopic, String> {
        let mut matching = self.topics.iter().filter(|stream| wanted(stream));
        let found = matching
            .next()
            .ok_or_else(|| format!("The dataset records no {topic} stream"))?;
        if matching.next().is_some() {
            return Err(format!(
                "The dataset records {topic} as more than one stream"
            ));
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datasets::TopicRevision;

    fn stream(topic: &str, stream: &str, schema: Option<&str>) -> DatasetTopic {
        DatasetTopic::new(
            topic.into(),
            TopicRevision::new(stream).unwrap(),
            "protobuf".into(),
        )
        .with_schema(schema.map(str::to_owned))
    }

    #[test]
    fn a_stream_is_found_by_its_topic_and_schema_and_only_once() {
        let topics = [
            stream("/top", "top_00000001", Some("foxglove.CompressedVideo")),
            stream("/joints", "joints_00000002", Some("JointState")),
            stream("/joints", "joints_00000003", Some("JointStateV2")),
            stream("/instruction", "instruction_00000004", Some("Instruction")),
        ];
        let streams = DatasetStreams::new(&topics);
        let found = |result: Result<&DatasetTopic, String>| {
            result.map(|topic| topic.revision().stream().to_owned())
        };
        assert_eq!(found(streams.camera("/top")), Ok("top_00000001".into()));
        assert!(streams.camera("/joints").is_err());
        assert_eq!(
            found(streams.recorded("/joints", Some("JointStateV2"))),
            Ok("joints_00000003".into())
        );
        assert!(streams.recorded("/joints", None).is_err());
        assert!(streams.topic("/joints").is_err());
        assert_eq!(
            found(streams.topic("/instruction")),
            Ok("instruction_00000004".into())
        );
        assert!(streams.topic("/missing").is_err());
    }
}
