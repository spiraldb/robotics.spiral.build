//! Resolve native camera bindings using the dataset catalog and Arrow schemas.
use crate::StorageManager;
use arrow_schema::DataType;
use spiral_robotics_common::datasets::DatasetInspection;
use spiral_robotics_common::workflow::{WorkflowSpec, episode_filter::CameraBinding};

impl StorageManager {
    pub async fn workflow_cameras(
        &self,
        inspection: &DatasetInspection,
    ) -> Result<Vec<CameraBinding>, String> {
        let dataset = inspection.dataset().selection();
        if !dataset.can_preview() {
            return Err("This dataset path cannot be addressed by native topic reads.".into());
        }
        let mut cameras = Vec::new();
        let mut connection = self.project_store().connect().await?;
        for topic in inspection.topics() {
            // A schema name identifies encoded video; field types establish the input contract.
            if !topic
                .schemas()
                .iter()
                .any(|name| name == "foxglove.CompressedVideo")
            {
                continue;
            }
            let revision = topic
                .revision()
                .ok_or("Camera topic has no native schema revision.")?;
            let quote = |value: &str| value.replace('\'', "''");
            let sql = format!(
                "SELECT id, log_time, value FROM spql.mcap.read_topic('{}', '{}', '{}') LIMIT 0",
                quote(&dataset.sql_name()),
                quote(topic.name()),
                quote(revision.variant())
            );
            let stream = connection.execute_query(sql).await?;
            let schema = stream.schema();
            let DataType::Struct(fields) = schema
                .field_with_name("value")
                .map_err(|e| e.to_string())?
                .data_type()
            else {
                continue;
            };
            let bytes = fields.iter().any(|field| {
                field.name() == "data"
                    && matches!(
                        field.data_type(),
                        DataType::Binary | DataType::LargeBinary | DataType::BinaryView
                    )
            });
            let format = fields.iter().any(|field| {
                field.name() == "format"
                    && matches!(
                        field.data_type(),
                        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
                    )
            });
            if bytes && format {
                cameras.push(CameraBinding::new(
                    topic.name().to_owned(),
                    revision.clone(),
                ));
            }
        }
        Ok(cameras)
    }

    pub async fn validate_episode_workflow(&self, workflow: &WorkflowSpec) -> Result<(), String> {
        let Some(source) = workflow.dataset() else {
            return Ok(());
        };
        let inspection = self.inspect_dataset(source.selection().clone()).await?;
        if inspection.dataset() != source {
            return Err(
                "The selected dataset changed. Reselect it and inspect its cameras again.".into(),
            );
        }
        if let Some(scoring) = workflow.camera_scoring() {
            let cameras = self.workflow_cameras(&inspection).await?;
            if !cameras.contains(scoring.camera()) {
                return Err(format!(
                    "Camera {} is not a supported encoded-video input in this dataset/schema. Inspect the dataset's available cameras.",
                    scoring.camera().topic()
                ));
            }
        }
        Ok(())
    }
}
