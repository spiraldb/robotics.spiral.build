//! SigLIP as an Arrow operator. No queries, cache lookups, or persistence.
use arrow_array::{
    Array as _, ArrayRef, Float32Array, LargeListArray, ListArray, RecordBatch, StructArray,
    UInt8Array, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef, extension::EXTENSION_TYPE_NAME_KEY};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::{clip, siglip};
use futures::{StreamExt as _, TryStreamExt as _};
use hf_hub::{Repo, RepoType, api::sync::ApiBuilder};
use image::imageops::FilterType;
use serde::Deserialize;
use spiral_robotics_common::workflow::WorkflowModelFeature;
use spiral_robotics_storage::{
    DatabaseError, DatabaseErrorKind, RecordBatchStream,
    workflow_features::{SIGLIP_BASE_MODEL_ID, SIGLIP_BASE_REPOSITORY, SIGLIP_BASE_REVISION},
};
use std::{path::PathBuf, sync::Arc};
use tokenizers::{
    Tokenizer,
    utils::{
        padding::{PaddingDirection, PaddingParams, PaddingStrategy},
        truncation::TruncationParams,
    },
};
use tokio_util::sync::CancellationToken;

const IMAGE_EXTENSION_NAME: &str = "spql.image.Image";
const DEFAULT_MODEL_INPUT_BUDGET_BYTES: usize = 8 * 1024 * 1024;
const MIN_MODEL_CHUNK_ROWS: usize = 1;
const MAX_MODEL_CHUNK_ROWS: usize = 16;

pub(crate) struct ModelOperator {
    image_column: String,
    features: Vec<WorkflowModelFeature>,
    engine: Option<SiglipEngine>,
    cancel: CancellationToken,
}

impl ModelOperator {
    pub(crate) fn new(image_column: String, features: Vec<WorkflowModelFeature>) -> Self {
        Self {
            image_column,
            features,
            engine: None,
            cancel: CancellationToken::new(),
        }
    }

    pub(crate) fn plan(&self, input: &SchemaRef) -> Result<SchemaRef, String> {
        let field = input
            .field_with_name(&self.image_column)
            .map_err(|e| e.to_string())?;
        if field
            .metadata()
            .get(EXTENSION_TYPE_NAME_KEY)
            .map(String::as_str)
            != Some(IMAGE_EXTENSION_NAME)
        {
            return Err(format!(
                "Model input {:?} must be a decoded Spiral image.",
                self.image_column
            ));
        }
        let DataType::Struct(children) = field.data_type() else {
            return Err("Spiral image must have Arrow struct storage.".into());
        };
        for (name, expected) in [
            ("width", DataType::UInt32),
            ("height", DataType::UInt32),
            ("pixel_format", DataType::UInt8),
        ] {
            if children
                .iter()
                .find(|f| f.name() == name)
                .is_none_or(|f| f.data_type() != &expected)
            {
                return Err(format!("Spiral image needs {name}: {expected}."));
            }
        }
        let pixels = children
            .iter()
            .find(|f| f.name() == "data")
            .map(|f| f.data_type());
        if !matches!(pixels, Some(DataType::List(f) | DataType::LargeList(f)) if f.data_type() == &DataType::UInt8)
        {
            return Err("Spiral image data must be a UInt8 list.".into());
        }
        let mut fields = input.fields().to_vec();
        for feature in &self.features {
            if fields.iter().any(|f| f.name() == feature.name()) {
                return Err(format!(
                    "Model output {:?} already exists in its input.",
                    feature.name()
                ));
            }
            fields.push(Arc::new(Field::new(
                feature.name(),
                DataType::Float32,
                true,
            )));
        }
        Ok(Arc::new(Schema::new_with_metadata(
            fields,
            input.metadata().clone(),
        )))
    }

    pub(crate) fn execute(
        mut self,
        input: RecordBatchStream,
        cancel: CancellationToken,
    ) -> Result<RecordBatchStream, String> {
        let schema = self.plan(input.schema())?;
        self.cancel = cancel;
        let output_schema = schema.clone();
        let stream = futures::stream::try_unfold((self, input), move |(operator, mut input)| {
            let schema = output_schema.clone();
            async move {
                operator.check_cancelled()?;
                let batch = tokio::select! {
                    _ = operator.cancel.cancelled() => return Err("Workflow run cancelled.".to_owned()),
                    batch = input.next() => match batch { Some(batch) => batch.map_err(|e| e.to_string())?, None => return Ok(None) },
                };
                // The blocking task is awaited, so cancellation cannot detach live inference.
                let (operator, batch) = tokio::task::spawn_blocking(move || {
                    let mut operator = operator;
                    let batch = operator.evaluate(batch, schema)?;
                    Ok::<_, String>((operator, batch))
                }).await.map_err(|e| format!("Model worker failed: {e}"))??;
                Ok(Some((batch, (operator, input))))
            }
        }).map_err(|error| DatabaseError::new(DatabaseErrorKind::Internal, error)).boxed();
        Ok(RecordBatchStream::new(schema, stream))
    }

    fn evaluate(&mut self, batch: RecordBatch, schema: SchemaRef) -> Result<RecordBatch, String> {
        let images = batch
            .column_by_name(&self.image_column)
            .and_then(|c| c.as_any().downcast_ref::<StructArray>())
            .ok_or_else(|| "Model image schema changed during execution.".to_owned())?;
        let mut scores = vec![vec![None; batch.num_rows()]; self.features.len()];
        if images.null_count() != images.len() {
            self.check_cancelled()?;
            if self.engine.is_none() {
                let artifacts = SiglipArtifacts::resolve()?;
                let concepts = self
                    .features
                    .iter()
                    .map(|f| f.concept())
                    .collect::<Vec<_>>();
                let (engine, warning) = SiglipEngine::load_preferred(&artifacts, &concepts)?;
                if let Some(warning) = warning {
                    tracing::warn!(%warning);
                }
                tracing::info!(device = %engine.device_name, "Loaded workflow SigLIP");
                self.engine = Some(engine);
            }
            let engine = self.engine.as_ref().unwrap();
            let indices = (0..self.features.len()).collect::<Vec<_>>();
            for offset in (0..images.len()).step_by(engine.processor.chunk_rows()) {
                self.check_cancelled()?;
                let end = images.len().min(offset + engine.processor.chunk_rows());
                let rows = (offset..end)
                    .filter(|row| !images.is_null(*row))
                    .collect::<Vec<_>>();
                if rows.is_empty() {
                    continue;
                }
                let mut pixels = Vec::new();
                for row in &rows {
                    pixels.extend(images.decode_image_row(*row, &engine.processor)?);
                }
                let values = engine.score_selected(SelectedImageBatch::new(
                    &pixels,
                    rows.len(),
                    &indices,
                ))?;
                for (row, values) in rows.into_iter().zip(values) {
                    for (column, value) in scores.iter_mut().zip(values) {
                        column[row] = Some(value);
                    }
                }
            }
        }
        self.check_cancelled()?;
        let mut columns = batch.columns().to_vec();
        columns.extend(
            scores
                .into_iter()
                .map(|values| Arc::new(Float32Array::from(values)) as ArrayRef),
        );
        RecordBatch::try_new(schema, columns).map_err(|e| e.to_string())
    }

    fn check_cancelled(&self) -> Result<(), String> {
        if self.cancel.is_cancelled() {
            Err("Workflow run cancelled.".into())
        } else {
            Ok(())
        }
    }
}

trait ArrowImagePixels {
    fn list_u8_value(&self, row: usize) -> Option<Vec<u8>>;
}
impl ArrowImagePixels for dyn arrow_array::Array + '_ {
    fn list_u8_value(&self, row: usize) -> Option<Vec<u8>> {
        if self.is_null(row) {
            return None;
        }
        let values = if let Some(values) = self.as_any().downcast_ref::<ListArray>() {
            values.value(row)
        } else {
            self.as_any().downcast_ref::<LargeListArray>()?.value(row)
        };
        let values = values.as_any().downcast_ref::<UInt8Array>()?;
        (values.null_count() == 0).then(|| values.values().to_vec())
    }
}

trait ImageRowDecoding {
    fn decode_image_row(
        &self,
        row: usize,
        processor: &SiglipImageProcessor,
    ) -> Result<Vec<f32>, String>;
}

impl ImageRowDecoding for StructArray {
    fn decode_image_row(
        &self,
        row: usize,
        processor: &SiglipImageProcessor,
    ) -> Result<Vec<f32>, String> {
        if self.is_null(row) {
            return Err("image is null".to_owned());
        }
        let widths = self
            .column_by_name("width")
            .and_then(|array| array.as_any().downcast_ref::<UInt32Array>())
            .ok_or_else(|| "image has no UInt32 width".to_owned())?;
        let heights = self
            .column_by_name("height")
            .and_then(|array| array.as_any().downcast_ref::<UInt32Array>())
            .ok_or_else(|| "image has no UInt32 height".to_owned())?;
        let formats = self
            .column_by_name("pixel_format")
            .and_then(|array| array.as_any().downcast_ref::<UInt8Array>())
            .ok_or_else(|| "image has no UInt8 pixel_format".to_owned())?;
        let data = self
            .column_by_name("data")
            .ok_or_else(|| "image has no pixel data".to_owned())?;
        if widths.is_null(row) || heights.is_null(row) || formats.is_null(row) {
            return Err("image dimensions or pixel format are null".to_owned());
        }
        let width = widths.value(row);
        let height = heights.value(row);
        if width == 0 || height == 0 {
            return Err("image dimensions are zero".to_owned());
        }
        let values = data
            .as_ref()
            .list_u8_value(row)
            .ok_or_else(|| "image pixels are not a non-null UInt8 list".to_owned())?;
        let dynamic = match formats.value(row) {
            1 => {
                image::RgbImage::from_raw(width, height, values).map(image::DynamicImage::ImageRgb8)
            }
            2 => image::RgbaImage::from_raw(width, height, values)
                .map(image::DynamicImage::ImageRgba8),
            format => return Err(format!("unsupported image pixel format {format}")),
        }
        .ok_or_else(|| "image pixels do not match their dimensions".to_owned())?;
        let rgb = dynamic.to_rgb8();
        let resized = image::imageops::resize(
            &rgb,
            processor.image_size as u32,
            processor.image_size as u32,
            FilterType::CatmullRom,
        );
        Ok(resized
            .as_raw()
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let channel = index % 3;
                ((*value as f32 * processor.rescale_factor) - processor.image_mean[channel])
                    / processor.image_std[channel]
            })
            .collect())
    }
}

struct SiglipArtifacts {
    config: PathBuf,
    preprocessor: PathBuf,
    tokenizer: PathBuf,
    weights: PathBuf,
}

impl SiglipArtifacts {
    fn resolve() -> Result<Self, String> {
        let api = ApiBuilder::new()
            .with_progress(false)
            .build()
            .map_err(|error| format!("opening the pinned Hugging Face model cache: {error}"))?;
        let repository = api.repo(Repo::with_revision(
            SIGLIP_BASE_REPOSITORY.to_owned(),
            RepoType::Model,
            SIGLIP_BASE_REVISION.to_owned(),
        ));
        let get = |name: &str| {
            repository.get(name).map_err(|error| {
                format!(
                    "loading pinned {SIGLIP_BASE_MODEL_ID} artifact {name:?}: {error}. Check network access, then run again."
                )
            })
        };
        Ok(Self {
            config: get("config.json")?,
            preprocessor: get("preprocessor_config.json")?,
            tokenizer: get("tokenizer.json")?,
            weights: get("model.safetensors")?,
        })
    }
}

#[derive(Deserialize)]
struct SiglipPreprocessorFile {
    #[serde(flatten)]
    operations: SiglipPreprocessorOperations,
    #[serde(flatten)]
    normalization: SiglipPreprocessorNormalization,
    #[serde(flatten)]
    resizing: SiglipPreprocessorResizing,
}

#[derive(Deserialize)]
struct SiglipPreprocessorOperations {
    do_normalize: bool,
    do_rescale: bool,
    do_resize: bool,
}

#[derive(Deserialize)]
struct SiglipPreprocessorNormalization {
    image_mean: Vec<f32>,
    image_std: Vec<f32>,
    rescale_factor: f32,
}

#[derive(Deserialize)]
struct SiglipPreprocessorResizing {
    resample: u8,
    size: SiglipPreprocessorSize,
}

#[derive(Deserialize)]
struct SiglipPreprocessorSize {
    height: usize,
    width: usize,
}

struct SiglipImageProcessor {
    image_size: usize,
    image_mean: [f32; 3],
    image_std: [f32; 3],
    rescale_factor: f32,
}

impl SiglipImageProcessor {
    fn load(path: &std::path::Path, model_image_size: usize) -> Result<Self, String> {
        let source: SiglipPreprocessorFile = serde_json::from_slice(
            &std::fs::read(path)
                .map_err(|error| format!("reading SigLIP preprocessor config: {error}"))?,
        )
        .map_err(|error| format!("decoding SigLIP preprocessor config: {error}"))?;
        Self::from_source(source, model_image_size)
    }

    fn from_source(
        source: SiglipPreprocessorFile,
        model_image_size: usize,
    ) -> Result<Self, String> {
        if !source.operations.do_resize
            || !source.operations.do_rescale
            || !source.operations.do_normalize
        {
            return Err(
                "Pinned SigLIP preprocessor must resize, rescale, and normalize images.".to_owned(),
            );
        }
        if source.resizing.resample != 3 {
            return Err(format!(
                "Pinned SigLIP preprocessor uses unsupported resample mode {}; expected PIL bicubic mode 3.",
                source.resizing.resample
            ));
        }
        if source.resizing.size.width != model_image_size
            || source.resizing.size.height != model_image_size
        {
            return Err(format!(
                "Pinned SigLIP processor dimensions {}x{} do not match model input {model_image_size}x{model_image_size}.",
                source.resizing.size.width, source.resizing.size.height
            ));
        }
        let image_mean: [f32; 3] =
            source
                .normalization
                .image_mean
                .try_into()
                .map_err(|values: Vec<f32>| {
                    format!(
                        "Pinned SigLIP preprocessor has {} mean values; expected 3.",
                        values.len()
                    )
                })?;
        let image_std: [f32; 3] =
            source
                .normalization
                .image_std
                .try_into()
                .map_err(|values: Vec<f32>| {
                    format!(
                        "Pinned SigLIP preprocessor has {} standard-deviation values; expected 3.",
                        values.len()
                    )
                })?;
        if !source.normalization.rescale_factor.is_finite()
            || source.normalization.rescale_factor <= 0.0
            || image_mean.iter().any(|value| !value.is_finite())
            || image_std
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err("Pinned SigLIP preprocessor normalization is invalid.".to_owned());
        }
        Ok(Self {
            image_size: model_image_size,
            image_mean,
            image_std,
            rescale_factor: source.normalization.rescale_factor,
        })
    }

    fn chunk_rows(&self) -> usize {
        let budget = std::env::var("SPIRAL_WORKFLOW_MODEL_INPUT_MIB")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .and_then(|mib| mib.checked_mul(1024 * 1024))
            .unwrap_or(DEFAULT_MODEL_INPUT_BUDGET_BYTES);
        Self::chunk_rows_for_budget(self.image_size, budget)
    }

    fn chunk_rows_for_budget(image_size: usize, budget_bytes: usize) -> usize {
        let bytes_per_image = image_size
            .saturating_mul(image_size)
            .saturating_mul(3)
            .saturating_mul(std::mem::size_of::<f32>());
        if bytes_per_image == 0 {
            return MIN_MODEL_CHUNK_ROWS;
        }
        (budget_bytes / bytes_per_image).clamp(MIN_MODEL_CHUNK_ROWS, MAX_MODEL_CHUNK_ROWS)
    }
}

struct SiglipEngine {
    model: siglip::Model,
    text_features: Tensor,
    logit_scale: Tensor,
    logit_bias: Tensor,
    device: Device,
    device_name: String,
    processor: SiglipImageProcessor,
}

struct SiglipDevice {
    device: Device,
    name: &'static str,
}

impl SiglipDevice {
    fn new(device: Device, name: &'static str) -> Self {
        Self { device, name }
    }
}

struct SelectedImageBatch<'a> {
    pixels: &'a [f32],
    images: usize,
    concept_indices: &'a [usize],
}

impl<'a> SelectedImageBatch<'a> {
    fn new(pixels: &'a [f32], images: usize, concept_indices: &'a [usize]) -> Self {
        Self {
            pixels,
            images,
            concept_indices,
        }
    }
}

impl SiglipEngine {
    fn load_preferred(
        artifacts: &SiglipArtifacts,
        concepts: &[&str],
    ) -> Result<(Self, Option<String>), String> {
        #[cfg(target_os = "macos")]
        {
            match Device::new_metal(0) {
                Ok(device) => {
                    match Self::load(artifacts, concepts, SiglipDevice::new(device, "Metal")) {
                        Ok(engine) => Ok((engine, None)),
                        Err(error) => {
                            let warning = format!(
                                "Metal SigLIP initialization failed; this run used CPU instead: {error}"
                            );
                            Self::load(artifacts, concepts, SiglipDevice::new(Device::Cpu, "CPU"))
                                .map(|engine| (engine, Some(warning)))
                        }
                    }
                }
                Err(error) => {
                    let warning =
                        format!("Metal is unavailable; this run used CPU instead: {error}");
                    Self::load(artifacts, concepts, SiglipDevice::new(Device::Cpu, "CPU"))
                        .map(|engine| (engine, Some(warning)))
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::load(artifacts, concepts, SiglipDevice::new(Device::Cpu, "CPU")).map(|engine| {
                (
                    engine,
                    Some("Metal is unavailable on this platform; this run used CPU.".to_owned()),
                )
            })
        }
    }

    fn load(
        artifacts: &SiglipArtifacts,
        concepts: &[&str],
        target: SiglipDevice,
    ) -> Result<Self, String> {
        let SiglipDevice {
            device,
            name: device_name,
        } = target;
        let config: siglip::Config = serde_json::from_slice(
            &std::fs::read(&artifacts.config)
                .map_err(|error| format!("reading SigLIP config: {error}"))?,
        )
        .map_err(|error| format!("decoding SigLIP config: {error}"))?;
        let processor =
            SiglipImageProcessor::load(&artifacts.preprocessor, config.vision_config.image_size)?;
        let mut tokenizer = Tokenizer::from_file(&artifacts.tokenizer)
            .map_err(|error| format!("loading SigLIP tokenizer: {error}"))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::Fixed(config.text_config.max_position_embeddings),
            direction: PaddingDirection::Right,
            pad_to_multiple_of: None,
            pad_id: config.text_config.pad_token_id,
            pad_type_id: 0,
            pad_token: "</s>".to_owned(),
        }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: config.text_config.max_position_embeddings,
                ..Default::default()
            }))
            .map_err(|error| format!("configuring SigLIP tokenizer: {error}"))?;
        let prompts = concepts
            .iter()
            .map(|concept| format!("This is a photo of {concept}."))
            .collect::<Vec<_>>();
        let encodings = tokenizer
            .encode_batch(prompts, true)
            .map_err(|error| format!("tokenizing SigLIP concepts: {error}"))?;
        let sequence = config.text_config.max_position_embeddings;
        let token_ids = encodings
            .iter()
            .flat_map(|encoding| encoding.get_ids().iter().copied())
            .collect::<Vec<_>>();
        let input_ids = Tensor::from_vec(token_ids, (concepts.len(), sequence), &device)
            .with_candle_context("uploading SigLIP concepts")?;
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[artifacts.weights.as_path()], DType::F32, &device)
                .with_candle_context("mapping SigLIP weights")?
        };
        let logit_scale = vb
            .get(&[1], "logit_scale")
            .with_candle_context("loading SigLIP logit scale")?
            .exp()
            .with_candle_context("calibrating SigLIP logit scale")?;
        let logit_bias = vb
            .get(&[1], "logit_bias")
            .with_candle_context("loading SigLIP logit bias")?;
        let model =
            siglip::Model::new(&config, vb).with_candle_context("loading the SigLIP model")?;
        let text_features = model
            .get_text_features(&input_ids)
            .and_then(|features| clip::div_l2_norm(&features))
            .with_candle_context("encoding SigLIP concepts")?;
        device
            .synchronize()
            .with_candle_context("synchronizing SigLIP concept encoding")?;
        Ok(Self {
            model,
            text_features,
            logit_scale,
            logit_bias,
            device,
            device_name: device_name.to_owned(),
            processor,
        })
    }

    fn score_selected(&self, batch: SelectedImageBatch<'_>) -> Result<Vec<Vec<f32>>, String> {
        let SelectedImageBatch {
            pixels,
            images,
            concept_indices,
        } = batch;
        let pixel_values = Tensor::from_vec(
            pixels.to_vec(),
            (
                images,
                self.processor.image_size,
                self.processor.image_size,
                3,
            ),
            &self.device,
        )
        .and_then(|tensor| tensor.permute((0, 3, 1, 2)))
        .with_candle_context("uploading SigLIP images")?;
        let image_features = self
            .model
            .get_image_features(&pixel_values)
            .and_then(|features| clip::div_l2_norm(&features))
            .with_candle_context("encoding SigLIP images")?;
        let indices = concept_indices
            .iter()
            .map(|index| {
                u32::try_from(*index).map_err(|_| "SigLIP concept index overflowed u32.".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let indices = Tensor::from_vec(indices, concept_indices.len(), &self.device)
            .with_candle_context("uploading selected SigLIP concept indices")?;
        let selected_text = self
            .text_features
            .index_select(&indices, 0)
            .with_candle_context("selecting cached SigLIP concept embeddings")?;
        let logits = selected_text
            .matmul(
                &image_features
                    .t()
                    .with_candle_context("transposing SigLIP image features")?,
            )
            .and_then(|logits| logits.broadcast_mul(&self.logit_scale))
            .and_then(|logits| logits.broadcast_add(&self.logit_bias))
            .and_then(|logits| logits.t())
            .and_then(|logits| candle_nn::ops::sigmoid(&logits))
            .with_candle_context("computing calibrated SigLIP scores")?;
        self.device
            .synchronize()
            .with_candle_context("synchronizing SigLIP inference")?;
        logits
            .to_vec2::<f32>()
            .with_candle_context("reading SigLIP scores")
    }
}

trait CandleResultExt<T> {
    fn with_candle_context(self, context: &'static str) -> Result<T, String>;
}

impl<T> CandleResultExt<T> for Result<T, candle_core::Error> {
    fn with_candle_context(self, context: &'static str) -> Result<T, String> {
        self.map_err(|error| format!("{context}: {error}"))
    }
}

#[cfg(test)]
mod tests;
