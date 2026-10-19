//! Native MobileSAM CPU path. An embedding belongs to one decoded image and
//! is reused while point groups are decoded; callers own image lifecycle.
use crate::{image_io, model, sam};
use image::GrayImage;
use ort::{
    ep,
    session::Session,
    value::{TensorElementType, TensorRef},
};
use serde::Serialize;
use std::{error::Error, io, path::Path, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}
fn elapsed(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

pub struct SelectionSession {
    decoder: Session,
    embedding: Vec<f32>,
    original: (u32, u32),
    pub encoder_ms: u64,
    pub decoder_load_ms: u64,
}

impl SelectionSession {
    pub fn load(
        runtime: &Path,
        encoder_path: &Path,
        decoder_path: &Path,
        image: &image::RgbaImage,
    ) -> Result<(Self, u64)> {
        let (data, width, height) = sam::image_tensor(image)?;
        Self::load_prepared(
            runtime,
            encoder_path,
            decoder_path,
            image.dimensions(),
            (width, height),
            &data,
        )
    }

    /// Diagnostic entry point for a frozen HWC Canvas tensor. The same fixed
    /// model and decoder path are used, isolating preprocessing from ORT.
    pub fn load_prepared(
        runtime: &Path,
        encoder_path: &Path,
        decoder_path: &Path,
        original: (u32, u32),
        resized: (u32, u32),
        data: &[f32],
    ) -> Result<(Self, u64)> {
        if sam::resized_size(original.0, original.1)? != resized
            || data.len() != resized.0 as usize * resized.1 as usize * 3
            || data
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=255.0).contains(value))
        {
            return Err(invalid("SAM 编码输入张量无效").into());
        }
        let started = Instant::now();
        model::verify_file(
            encoder_path,
            model::find("mobilesam-image-encoder").unwrap(),
        )?;
        model::verify_file(decoder_path, model::find("mobilesam-mask-decoder").unwrap())?;
        let verify_ms = elapsed(started);
        crate::runtime::initialize(runtime)?;
        let started = Instant::now();
        let mut encoder = Session::builder()?
            .with_execution_providers([ep::CPU::default().build()])
            .map_err(|error| io::Error::other(error.to_string()))?
            .commit_from_file(encoder_path)?;
        if encoder.inputs().len() != 1
            || encoder.inputs()[0].name() != "input_image"
            || encoder.inputs()[0].dtype().tensor_type() != Some(TensorElementType::Float32)
            || !encoder
                .outputs()
                .iter()
                .any(|o| o.name() == "image_embeddings")
        {
            return Err(invalid("SAM encoder 张量契约不匹配").into());
        }
        let input = TensorRef::from_array_view(([resized.1 as i64, resized.0 as i64, 3], data))?;
        let output = encoder.run(ort::inputs!["input_image" => input])?;
        let value = output
            .get("image_embeddings")
            .ok_or_else(|| invalid("SAM embedding 缺失"))?;
        let (shape, values) = value.try_extract_tensor::<f32>()?;
        if &**shape != [1, 256, 64, 64] || values.iter().any(|v| !v.is_finite()) {
            return Err(invalid("SAM embedding 无效").into());
        }
        let embedding = values.to_vec();
        let encoder_ms = elapsed(started);
        drop(output);
        drop(encoder);
        let started = Instant::now();
        let decoder = Session::builder()?
            .with_execution_providers([ep::CPU::default().build()])
            .map_err(|error| io::Error::other(error.to_string()))?
            .commit_from_file(decoder_path)?;
        let expected = [
            "image_embeddings",
            "point_coords",
            "point_labels",
            "mask_input",
            "has_mask_input",
            "orig_im_size",
        ];
        if decoder.inputs().len() != expected.len()
            || expected.iter().any(|name| {
                !decoder.inputs().iter().any(|input| {
                    input.name() == *name
                        && input.dtype().tensor_type() == Some(TensorElementType::Float32)
                })
            })
            || !decoder
                .outputs()
                .iter()
                .any(|o| o.name() == "low_res_masks")
            || !decoder
                .outputs()
                .iter()
                .any(|o| o.name() == "iou_predictions")
        {
            return Err(invalid("SAM decoder 张量契约不匹配").into());
        }
        let decoder_load_ms = elapsed(started);
        Ok((
            Self {
                decoder,
                embedding,
                original,
                encoder_ms,
                decoder_load_ms,
            },
            verify_ms,
        ))
    }

    pub fn select(
        &mut self,
        points: &[sam::Point],
        output_size: (u32, u32),
    ) -> Result<(Vec<u8>, u64)> {
        let (coords, labels) = sam::prompt(points, self.original.0, self.original.1)?;
        let embedding = TensorRef::from_array_view(([1, 256, 64, 64], self.embedding.as_slice()))?;
        let point_coords =
            TensorRef::from_array_view(([1, labels.len() as i64, 2], coords.as_slice()))?;
        let point_labels =
            TensorRef::from_array_view(([1, labels.len() as i64], labels.as_slice()))?;
        let zeros = vec![0f32; 256 * 256];
        let mask_input = TensorRef::from_array_view(([1, 1, 256, 256], zeros.as_slice()))?;
        let no_mask = [0f32];
        let has_mask = TensorRef::from_array_view(([1], no_mask.as_slice()))?;
        let size = [self.original.1 as f32, self.original.0 as f32];
        let orig_size = TensorRef::from_array_view(([2], size.as_slice()))?;
        let started = Instant::now();
        let output = self.decoder.run(ort::inputs![
            "image_embeddings" => embedding, "point_coords" => point_coords, "point_labels" => point_labels,
            "mask_input" => mask_input, "has_mask_input" => has_mask, "orig_im_size" => orig_size
        ])?;
        let masks = output
            .get("low_res_masks")
            .ok_or_else(|| invalid("SAM masks 缺失"))?;
        let scores = output
            .get("iou_predictions")
            .ok_or_else(|| invalid("SAM scores 缺失"))?;
        let (mask_shape, mask_values) = masks.try_extract_tensor::<f32>()?;
        let (score_shape, score_values) = scores.try_extract_tensor::<f32>()?;
        if mask_shape.len() != 4
            || mask_shape[0] != 1
            || mask_shape[2] != 256
            || mask_shape[3] != 256
            || score_shape.len() != 2
            || score_shape[0] != 1
            || score_shape[1] != mask_shape[1]
        {
            return Err(invalid("SAM 输出形状无效").into());
        }
        let mask = sam::select_mask(
            mask_values,
            score_values,
            points,
            self.original,
            output_size,
        )?;
        Ok((mask, elapsed(started)))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub backend: &'static str,
    pub model_verify_ms: u64,
    pub encoder_ms: u64,
    pub decoder_load_ms: u64,
    pub decoder_ms: Vec<u64>,
    pub subject_count: usize,
    pub source_width: u32,
    pub source_height: u32,
}

/// `groups` are independent subjects; each is decoded using the same embedding.
pub fn select_file(
    runtime: &Path,
    encoder: &Path,
    decoder: &Path,
    image_path: &Path,
    output_path: &Path,
    groups: &[Vec<sam::Point>],
) -> Result<Report> {
    if groups.is_empty() || [runtime, encoder, decoder, image_path].contains(&output_path) {
        return Err(invalid("SAM 输入或输出路径无效").into());
    }
    let image = image_io::decode_image(image_path)?;
    let size = image.dimensions();
    let (mut session, model_verify_ms) = SelectionSession::load(runtime, encoder, decoder, &image)?;
    let mut union = vec![0; size.0 as usize * size.1 as usize];
    let mut decoder_ms = Vec::with_capacity(groups.len());
    for group in groups {
        let (mask, duration) = session.select(group, size)?;
        union = sam::union(&union, &mask)?;
        decoder_ms.push(duration);
    }
    let output =
        GrayImage::from_raw(size.0, size.1, union).ok_or_else(|| invalid("SAM 输出尺寸无效"))?;
    image_io::save_gray_png_noclobber(&output, output_path)?;
    Ok(Report {
        backend: "cpu",
        model_verify_ms,
        encoder_ms: session.encoder_ms,
        decoder_load_ms: session.decoder_load_ms,
        decoder_ms,
        subject_count: groups.len(),
        source_width: size.0,
        source_height: size.1,
    })
}
