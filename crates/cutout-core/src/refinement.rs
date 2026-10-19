//! Native FP32 ViTMatte CPU path. Verify the source file before correcting
//! the pinned ONNX export's output shape metadata in memory.
use crate::{image_io, matte, model};
use image::{imageops::FilterType, GrayImage, RgbaImage};
use ort::{
    ep,
    session::Session,
    value::{TensorElementType, TensorRef},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{borrow::Cow, error::Error, io, path::Path, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}
fn elapsed(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub backend: &'static str,
    pub model_verify_ms: u64,
    pub session_load_ms: u64,
    pub inference_ms: u64,
    pub derived_sha256: String,
    pub source_width: u32,
    pub source_height: u32,
    pub mask_width: u32,
    pub mask_height: u32,
}

pub struct RefinementSession {
    session: Session,
    pub derived_sha256: String,
}

impl RefinementSession {
    pub fn load(runtime: &Path, model_path: &Path) -> Result<(Self, u64, u64)> {
        let started = Instant::now();
        model::verify_file(model_path, model::find("vitmatte-high").unwrap())?;
        let model_verify_ms = elapsed(started);
        let mut bytes = std::fs::read(model_path)?;
        matte::fix_output_metadata(&mut bytes)?;
        let derived_sha256 = format!("{:x}", Sha256::digest(&bytes));
        crate::runtime::initialize(runtime)?;
        let started = Instant::now();
        let session = Session::builder()?
            .with_execution_providers([ep::CPU::default().build()])
            .map_err(|error| io::Error::other(error.to_string()))?
            .commit_from_memory(&bytes)?;
        if session.inputs().len() != 1
            || session.inputs()[0].name() != "pixel_values"
            || session.inputs()[0].dtype().tensor_type() != Some(TensorElementType::Float32)
            || !session.outputs().iter().any(|o| {
                o.name() == "alphas" && o.dtype().tensor_type() == Some(TensorElementType::Float32)
            })
        {
            return Err(invalid("ViTMatte 张量契约不匹配").into());
        }
        Ok((
            Self {
                session,
                derived_sha256,
            },
            model_verify_ms,
            elapsed(started),
        ))
    }

    /// Return the model-resolution alpha. Callers may resize it to the source
    /// dimensions for workspace editing while the CLI keeps its file contract.
    pub fn refine(
        &mut self,
        image: &RgbaImage,
        coarse: &GrayImage,
        edge: u32,
    ) -> Result<(GrayImage, u64)> {
        if edge > 1024 {
            return Err(invalid("精修边缘宽度无效").into());
        }
        let original = image.dimensions();
        let target = matte::size(original.0, original.1)?;
        let scaled = if target == original {
            Cow::Borrowed(image)
        } else {
            Cow::Owned(image::imageops::resize(
                image,
                target.0,
                target.1,
                FilterType::Triangle,
            ))
        };
        let trimap = matte::trimap(coarse.as_raw(), None, coarse.dimensions(), target, edge)?;
        let (tensor, padded_width, padded_height) = matte::input(&scaled, &trimap)?;
        let input = TensorRef::from_array_view((
            [1, 4, padded_height as i64, padded_width as i64],
            tensor.as_slice(),
        ))?;
        let started = Instant::now();
        let outputs = self.session.run(ort::inputs!["pixel_values" => input])?;
        let output = outputs
            .get("alphas")
            .ok_or_else(|| invalid("ViTMatte alpha 缺失"))?;
        let (shape, values) = output.try_extract_tensor::<f32>()?;
        if &**shape != [1, 1, padded_height as i64, padded_width as i64] {
            return Err(invalid("ViTMatte 输出形状无效").into());
        }
        let alpha = matte::alpha(values, &trimap, target, (padded_width, padded_height))?;
        let gray = GrayImage::from_raw(target.0, target.1, alpha)
            .ok_or_else(|| invalid("ViTMatte 输出尺寸无效"))?;
        Ok((gray, elapsed(started)))
    }
}

pub fn refine_file(
    runtime: &Path,
    model_path: &Path,
    image_path: &Path,
    coarse_path: &Path,
    output_path: &Path,
    edge: u32,
) -> Result<Report> {
    if [runtime, model_path, image_path, coarse_path].contains(&output_path) || edge > 1024 {
        return Err(invalid("精修输入或输出路径无效").into());
    }
    let image = image_io::decode_image(image_path)?;
    let coarse = image_io::decode_gray_png(coarse_path)?;
    let original = image.dimensions();
    let (mut session, model_verify_ms, session_load_ms) =
        RefinementSession::load(runtime, model_path)?;
    let (gray, inference_ms) = session.refine(&image, &coarse, edge)?;
    image_io::save_gray_png_noclobber(&gray, output_path)?;
    Ok(Report {
        backend: "cpu",
        model_verify_ms,
        session_load_ms,
        inference_ms,
        derived_sha256: session.derived_sha256,
        source_width: original.0,
        source_height: original.1,
        mask_width: gray.width(),
        mask_height: gray.height(),
    })
}
