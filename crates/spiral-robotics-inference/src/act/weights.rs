use std::sync::Arc;

use burn_core::tensor::backend::Backend;
use burn_core::tensor::{Tensor, TensorData};
use safetensors::{Dtype, SafeTensors};

use crate::{Error, Result};

/// Process-owned, structurally valid SafeTensors bytes.
pub(super) struct Weights {
    label: String,
    bytes: Arc<[u8]>,
}

impl Weights {
    pub fn from_bytes(label: impl Into<String>, bytes: impl Into<Arc<[u8]>>) -> Result<Self> {
        let label = label.into();
        let bytes = bytes.into();
        SafeTensors::deserialize(&bytes)
            .map_err(|error| Error::Checkpoint(format!("{label}: {error}")))?;
        Ok(Self { label, bytes })
    }

    pub fn tensor_exact<B: Backend, const D: usize>(
        &self,
        name: &str,
        expected: [usize; D],
        device: &B::Device,
    ) -> Result<Tensor<B, D>> {
        let view = self.f32_view_exact(name, &expected)?;
        let values = decode_f32(name, view.data())?;
        Ok(Tensor::from_data(TensorData::new(values, expected), device))
    }

    pub fn vector_exact(&self, name: &str, length: usize) -> Result<Vec<f32>> {
        self.values_exact(name, &[length])
    }

    pub fn values_exact(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let view = self.f32_view_exact(name, shape)?;
        decode_f32(name, view.data())
    }

    pub fn contains(&self, name: &str) -> bool {
        SafeTensors::deserialize(&self.bytes).is_ok_and(|tensors| tensors.tensor(name).is_ok())
    }

    fn f32_view_exact<'a>(
        &'a self,
        name: &str,
        expected: &[usize],
    ) -> Result<safetensors::tensor::TensorView<'a>> {
        let tensors = SafeTensors::deserialize(&self.bytes)
            .map_err(|error| Error::Checkpoint(format!("{}: {error}", self.label)))?;
        let view = tensors
            .tensor(name)
            .map_err(|_| Error::Checkpoint(format!("missing tensor {name:?}")))?;
        if view.dtype() != Dtype::F32 || view.shape() != expected {
            return Err(Error::Checkpoint(format!(
                "tensor {name:?} must be F32 with shape {expected:?}, found {:?} with shape {:?}",
                view.dtype(),
                view.shape()
            )));
        }
        Ok(view)
    }
}

fn decode_f32(name: &str, bytes: &[u8]) -> Result<Vec<f32>> {
    let chunks = bytes.chunks_exact(4);
    if !chunks.remainder().is_empty() {
        return Err(Error::Checkpoint(format!(
            "tensor {name:?} has an invalid F32 byte count"
        )));
    }
    chunks
        .map(|chunk| {
            let bytes: [u8; 4] = chunk.try_into().map_err(|_| {
                Error::Checkpoint(format!("tensor {name:?} has invalid F32 storage"))
            })?;
            Ok(f32::from_le_bytes(bytes))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Weights;

    fn one_tensor(shape: &[usize], data_bytes: usize) -> Vec<u8> {
        let mut header = format!(
            r#"{{"x":{{"dtype":"F32","shape":{},"data_offsets":[0,{data_bytes}]}}}}"#,
            serde_json::to_string(shape).unwrap()
        );
        while !header.len().is_multiple_of(8) {
            header.push(' ');
        }
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.resize(bytes.len() + data_bytes, 0);
        bytes
    }

    #[test]
    fn exact_shape_is_part_of_the_checkpoint_contract() {
        let weights =
            Weights::from_bytes("synthetic.safetensors", one_tensor(&[2, 2], 16)).unwrap();
        assert!(weights.f32_view_exact("x", &[2, 2]).is_ok());
        assert!(weights.f32_view_exact("x", &[2, 3]).is_err());
    }

    #[test]
    fn malformed_storage_is_rejected_before_tensor_creation() {
        assert!(Weights::from_bytes("synthetic.safetensors", one_tensor(&[2], 7)).is_err());
    }
}
