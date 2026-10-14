//! A LeRobot feature as a stream's values: one with a name per value is a
//! struct of one field per name, one without is `{data: fixed_size_list}`,
//! and a camera is `{data, format}` holding one H.264 access unit.
use arrow_array::cast::AsArray as _;
use arrow_array::{Array, ArrayRef, FixedSizeListArray, OffsetSizeTrait, StructArray, UInt32Array};
use arrow_schema::{DataType, Field, Fields};
use serde::Deserialize;
use std::sync::Arc;

/// What `meta/info.json` says of one feature.
#[derive(Deserialize)]
pub(super) struct FeatureInfo {
    dtype: String,
    shape: Vec<usize>,
    names: Option<serde_json::Value>,
}

/// One feature a repository records of every frame.
pub(super) struct LeRobotFeature {
    name: String,
    shape: FeatureShape,
}

enum FeatureShape {
    /// One value per name.
    Named(Vec<String>),
    /// This many values.
    Values(usize),
    Camera,
}

impl LeRobotFeature {
    /// The features LeRobot positions each frame by, which every message's
    /// position and its episode's row already hold.
    const FRAME_POSITIONS: [&str; 5] = [
        "timestamp",
        "frame_index",
        "episode_index",
        "index",
        "task_index",
    ];

    /// The feature `info` describes as `name`; none for one of LeRobot's
    /// own frame positions.
    pub(super) fn of(name: &str, info: &FeatureInfo) -> Result<Option<Self>, String> {
        if Self::FRAME_POSITIONS.contains(&name) {
            return Ok(None);
        }
        let shape = match info.dtype.as_str() {
            "video" => FeatureShape::Camera,
            "image" => {
                return Err(format!(
                    "{name} is a LeRobot image feature; Studio imports cameras stored as video"
                ));
            }
            _ => {
                let size = info.shape.iter().product::<usize>();
                let names = info
                    .names
                    .as_ref()
                    .and_then(|names| names.as_array())
                    .map(|names| {
                        names
                            .iter()
                            .map(|name| name.as_str().map(str::to_owned))
                            .collect::<Option<Vec<_>>>()
                    });
                match names.flatten() {
                    Some(names)
                        if names.len() == size
                            && names
                                .iter()
                                .enumerate()
                                .all(|(place, name)| !names[..place].contains(name)) =>
                    {
                        FeatureShape::Named(names)
                    }
                    _ => FeatureShape::Values(size),
                }
            }
        };
        Ok(Some(Self {
            name: name.to_owned(),
            shape,
        }))
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn is_camera(&self) -> bool {
        matches!(self.shape, FeatureShape::Camera)
    }

    /// What a camera's every frame is.
    pub(super) fn camera_type() -> DataType {
        DataType::Struct(Fields::from(vec![
            Field::new("data", DataType::BinaryView, false),
            Field::new("format", DataType::Utf8View, false),
        ]))
    }

    /// `column`, the feature's column of a frame table, as one value per
    /// frame.
    pub(super) fn values(&self, column: &ArrayRef) -> Result<ArrayRef, String> {
        let size = match &self.shape {
            FeatureShape::Named(names) => names.len(),
            FeatureShape::Values(size) => *size,
            FeatureShape::Camera => return Err(format!("{} is a camera", self.name)),
        };
        let items = self.items(column, size)?;
        let item_type = items.data_type().clone();
        if !(item_type.is_numeric() || item_type == DataType::Boolean) || items.null_count() > 0 {
            return Err(format!(
                "{} holds {item_type} values with gaps or of a kind Studio cannot store",
                self.name
            ));
        }
        let (fields, members): (Vec<_>, Vec<_>) = match &self.shape {
            FeatureShape::Named(names) => names
                .iter()
                .enumerate()
                .map(|(place, name)| {
                    let indices = UInt32Array::from_iter_values(
                        (0..column.len()).map(|frame| (frame * size + place) as u32),
                    );
                    let member = arrow_select::take::take(&items, &indices, None)
                        .map_err(|error| error.to_string())?;
                    Ok((Field::new(name, item_type.clone(), false), member))
                })
                .collect::<Result<Vec<_>, String>>()?
                .into_iter()
                .unzip(),
            _ => {
                let item = Arc::new(Field::new_list_field(item_type, false));
                let data = FixedSizeListArray::try_new(item, size as i32, items, None)
                    .map_err(|error| format!("{}: {error}", self.name))?;
                (
                    vec![Field::new("data", data.data_type().clone(), false)],
                    vec![Arc::new(data) as ArrayRef],
                )
            }
        };
        StructArray::try_new(Fields::from(fields), members, None)
            .map(|array| Arc::new(array) as ArrayRef)
            .map_err(|error| format!("{}: {error}", self.name))
    }

    /// Every frame's `size` values, one after another; a frame of any other
    /// count is refused.
    fn items(&self, column: &ArrayRef, size: usize) -> Result<ArrayRef, String> {
        if column.null_count() > 0 {
            return Err(format!("{} is missing from some frames", self.name));
        }
        match column.data_type() {
            DataType::List(_) => self.listed(column.as_list::<i32>(), size),
            DataType::LargeList(_) => self.listed(column.as_list::<i64>(), size),
            DataType::FixedSizeList(_, listed) if *listed as usize == size => {
                Ok(column.as_fixed_size_list().values().clone())
            }
            DataType::FixedSizeList(..) | DataType::Struct(_) => Err(format!(
                "{} holds other than {size} values a frame",
                self.name
            )),
            _ if size == 1 => Ok(column.clone()),
            other => Err(format!(
                "{} holds {other}, not {size} values a frame",
                self.name
            )),
        }
    }

    fn listed<O: OffsetSizeTrait>(
        &self,
        list: &arrow_array::GenericListArray<O>,
        size: usize,
    ) -> Result<ArrayRef, String> {
        let offsets = list.value_offsets();
        if offsets
            .windows(2)
            .any(|pair| (pair[1] - pair[0]).as_usize() != size)
        {
            return Err(format!(
                "{} holds other than {size} values a frame",
                self.name
            ));
        }
        let first = offsets[0].as_usize();
        Ok(list
            .values()
            .slice(first, offsets[list.len()].as_usize() - first))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, ListBuilder};
    use serde_json::json;

    fn feature(name: &str, info: serde_json::Value) -> LeRobotFeature {
        let info: FeatureInfo = serde_json::from_value(info).unwrap();
        LeRobotFeature::of(name, &info).unwrap().unwrap()
    }

    fn frames(values: &[[f32; 2]]) -> ArrayRef {
        let mut column = ListBuilder::new(Float32Builder::new());
        for frame in values {
            column.values().append_slice(frame);
            column.append(true);
        }
        Arc::new(column.finish())
    }

    /// svla_so101_pickplace's `observation.state` names every joint.
    #[test]
    fn a_named_feature_is_a_struct_of_one_float_per_name() {
        let state = feature(
            "observation.state",
            json!({"dtype": "float32", "shape": [2], "names": ["shoulder_pan.pos", "gripper.pos"]}),
        );
        let values = state.values(&frames(&[[1.0, 2.0], [3.0, 4.0]])).unwrap();
        assert_eq!(
            crate::datasets::ArrowTypeText::try_from(values.data_type())
                .unwrap()
                .to_string(),
            "struct<shoulder_pan.pos: float not null, gripper.pos: float not null>"
        );
        let gripper = values
            .as_struct()
            .column(1)
            .as_primitive::<arrow_array::types::Float32Type>()
            .clone();
        assert_eq!(gripper.values().to_vec(), [2.0, 4.0]);
        // A slice of a frame table reads only its own frames.
        let sliced = state
            .values(&frames(&[[1.0, 2.0], [3.0, 4.0]]).slice(1, 1))
            .unwrap();
        let pan = sliced
            .as_struct()
            .column(0)
            .as_primitive::<arrow_array::types::Float32Type>()
            .clone();
        assert_eq!(pan.values().to_vec(), [3.0]);
    }

    /// sim-bottles-mjwarp-v1's `actions` has one name for fourteen values,
    /// so it names none of them.
    #[test]
    fn an_unnamed_feature_is_a_fixed_list_of_its_values() {
        let actions = feature(
            "actions",
            json!({"dtype": "float32", "shape": [2], "names": ["actions"]}),
        );
        let mut fixed = FixedSizeListBuilder::new(Float32Builder::new(), 2);
        fixed.values().append_slice(&[1.0, 2.0]);
        fixed.append(true);
        for column in [frames(&[[1.0, 2.0]]), Arc::new(fixed.finish()) as ArrayRef] {
            let values = actions.values(&column).unwrap();
            assert_eq!(
                crate::datasets::ArrowTypeText::try_from(values.data_type())
                    .unwrap()
                    .to_string(),
                "struct<data: fixed_size_list<item: float not null>[2] not null>"
            );
        }
        assert!(actions.values(&frames(&[[1.0, 2.0]]).slice(0, 0)).is_ok());
        let mut ragged = ListBuilder::new(Float32Builder::new());
        ragged.values().append_slice(&[1.0]);
        ragged.append(true);
        assert!(
            actions
                .values(&(Arc::new(ragged.finish()) as ArrayRef))
                .is_err()
        );
    }

    #[test]
    fn frame_positions_are_no_features_and_images_are_refused() {
        let info = |dtype: &str| -> FeatureInfo {
            serde_json::from_value(json!({"dtype": dtype, "shape": [1], "names": null})).unwrap()
        };
        assert!(
            LeRobotFeature::of("timestamp", &info("float32"))
                .unwrap()
                .is_none()
        );
        assert!(LeRobotFeature::of("observation.images.up", &info("image")).is_err());
        assert!(
            LeRobotFeature::of("observation.images.up", &info("video"))
                .unwrap()
                .unwrap()
                .is_camera()
        );
        let scalar = LeRobotFeature::of("next.reward", &info("float32"))
            .unwrap()
            .unwrap();
        let values = scalar
            .values(&(Arc::new(arrow_array::Float32Array::from(vec![0.5])) as ArrayRef))
            .unwrap();
        assert_eq!(values.len(), 1);
    }
}
