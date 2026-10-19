use crate::{image_io, isnet, mask, model, model_store::ModelLease};
use image::{GrayImage, RgbaImage};
use ort::{
    ep,
    session::{RunOptions, Session},
    value::{TensorElementType, TensorRef},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

type CoreResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
const INPUT_SHAPE: [i64; 4] = [1, 3, 1024, 1024];
const OUTPUT_SHAPE: [i64; 4] = [1, 1, 1024, 1024];

#[derive(Debug)]
struct InferenceError(&'static str);

impl std::fmt::Display for InferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for InferenceError {}

fn invalid(message: &'static str) -> Box<dyn Error + Send + Sync> {
    Box::new(InferenceError(message))
}

#[derive(Clone)]
pub struct CancelHandle {
    requested: Arc<AtomicBool>,
    run_options: Arc<RunOptions>,
}

impl CancelHandle {
    /// Cancellation also signals ONNX Runtime. A caller must still wait for the
    /// worker to return before considering the session stopped.
    pub fn cancel(&self) -> CoreResult<()> {
        self.requested.store(true, Ordering::Release);
        self.run_options.terminate()?;
        Ok(())
    }

    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    fn check(&self) -> CoreResult<()> {
        if self.is_requested() {
            Err(invalid("推理已取消"))
        } else {
            Ok(())
        }
    }
}

pub struct IsnetSession {
    session: Session,
    backend: &'static str,
    model_path: PathBuf,
    cancel: CancelHandle,
    prepared_mask: Option<PreparedMask>,
}

// One model-sized mask (1 MiB), never a second session or full-resolution image.
struct PreparedMask {
    fingerprint: [u8; 32],
    normalized: Vec<u8>,
    fitted_width: u32,
    fitted_height: u32,
}

fn source_fingerprint(source: &RgbaImage) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(source.width().to_le_bytes());
    digest.update(source.height().to_le_bytes());
    digest.update(source.as_raw());
    digest.finalize().into()
}

impl IsnetSession {
    /// `runtime_path` must point to a locally installed native ONNX Runtime
    /// library. The caller decides its distribution and checks its provenance.
    pub fn load(runtime_path: &Path, model_path: &Path) -> CoreResult<(Self, u64, u64)> {
        Self::load_checked(runtime_path, model_path, true)
    }

    /// Explicit CPU reference for native quality comparisons and diagnostics.
    pub fn load_cpu(runtime_path: &Path, model_path: &Path) -> CoreResult<(Self, u64, u64)> {
        Self::load_checked(runtime_path, model_path, false)
    }

    fn load_checked(
        runtime_path: &Path,
        model_path: &Path,
        accelerate: bool,
    ) -> CoreResult<(Self, u64, u64)> {
        let spec = model::find("isnet-auto-high").expect("固定模型清单缺少 IS-Net");
        let verify_started = Instant::now();
        model::verify_file(model_path, spec)?;
        let verify_ms = elapsed_ms(verify_started);
        let (session, load_ms) = Self::load_session(runtime_path, model_path, accelerate)?;
        Ok((session, verify_ms, load_ms))
    }

    /// ModelStore only issues a lease after the file passed its pinned digest.
    /// Workspace sessions must not hash the same large model again on first use.
    pub(crate) fn load_verified(runtime_path: &Path, lease: &ModelLease) -> CoreResult<Self> {
        Ok(Self::load_session(runtime_path, lease.path(), true)?.0)
    }

    fn load_session(
        runtime_path: &Path,
        model_path: &Path,
        accelerate: bool,
    ) -> CoreResult<(Self, u64)> {
        let load_started = Instant::now();
        crate::runtime::initialize(runtime_path)?;
        let (session, backend) = load_backend(runtime_path, model_path, accelerate)?;
        validate_contract(&session)?;
        let run_options = Arc::new(RunOptions::new()?);
        let load_ms = elapsed_ms(load_started);
        Ok((
            Self {
                session,
                backend,
                model_path: model_path.to_owned(),
                prepared_mask: None,
                cancel: CancelHandle {
                    requested: Arc::new(AtomicBool::new(false)),
                    run_options,
                },
            },
            load_ms,
        ))
    }

    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    pub fn backend(&self) -> &'static str {
        self.backend
    }

    pub fn run_prepared(&mut self, input: &isnet::PreparedInput) -> CoreResult<Vec<f32>> {
        self.cancel.check()?;
        let expected = (3 * isnet::INPUT_SIDE * isnet::INPUT_SIDE) as usize;
        if input.tensor.len() != expected || input.tensor.iter().any(|value| !value.is_finite()) {
            return Err(invalid("IS-Net 输入张量无效"));
        }
        match self.run_once(input) {
            Err(error) if self.backend != "cpu" && !self.cancel.is_requested() => {
                eprintln!("IS-Net GPU execution failed; retrying with CPU: {error}");
                self.prepared_mask = None;
                self.cancel.check()?;
                // A runtime failure can occur long after initial loading. Never
                // reopen a replaced model using the old verification result.
                let spec = model::find("isnet-auto-high").expect("固定模型清单缺少 IS-Net");
                model::verify_file(&self.model_path, spec)?;
                self.cancel.check()?;
                let session = load_cpu_session(&self.model_path)?;
                validate_contract(&session)?;
                self.session = session;
                self.backend = "cpu";
                self.cancel.check()?;
                self.run_once(input)
            }
            result => result,
        }
    }

    fn run_once(&mut self, input: &isnet::PreparedInput) -> CoreResult<Vec<f32>> {
        self.cancel.check()?;
        let tensor = TensorRef::from_array_view((INPUT_SHAPE, input.tensor.as_slice()))?;
        let output = self
            .session
            .run_with_options(ort::inputs![tensor], &self.cancel.run_options)
            .map_err(|error| {
                if self.cancel.is_requested() {
                    invalid("推理已取消")
                } else {
                    Box::new(error) as Box<dyn Error + Send + Sync>
                }
            })?;
        self.cancel.check()?;
        let selected = output
            .get("output_image")
            .ok_or_else(|| invalid("IS-Net 缺少 output_image 输出"))?;
        let (shape, values) = selected.try_extract_tensor::<f32>()?;
        if **shape != OUTPUT_SHAPE || values.iter().any(|value| !value.is_finite()) {
            return Err(invalid("IS-Net 输出形状或数值无效"));
        }
        Ok(values.to_vec())
    }

    /// Return the full-size coverage mask for a Rust-owned workspace asset.
    /// The source image remains on disk and the mask never crosses JSON IPC.
    pub fn infer_mask(&mut self, source: &RgbaImage) -> CoreResult<GrayImage> {
        self.prepare_mask(source)?;
        let prepared = self.prepared_mask.as_ref().expect("已准备 IS-Net 蒙版");
        let restored = isnet::restore_contain_mask(
            &prepared.normalized,
            isnet::INPUT_SIDE,
            isnet::INPUT_SIDE,
            prepared.fitted_width,
            prepared.fitted_height,
        )?;
        Ok(mask::resize_model_mask(
            &restored,
            isnet::INPUT_SIDE,
            isnet::INPUT_SIDE,
            source.width(),
            source.height(),
        )?)
    }

    /// Speculatively prepare only the selected image while the scheduler is idle.
    /// Publishing a result still requires an explicit, current automatic task.
    pub(crate) fn prepare_mask(&mut self, source: &RgbaImage) -> CoreResult<()> {
        self.cancel.check()?;
        let fingerprint = source_fingerprint(source);
        if self
            .prepared_mask
            .as_ref()
            .is_some_and(|prepared| prepared.fingerprint == fingerprint)
        {
            return Ok(());
        }
        self.prepared_mask = None;
        let prepared = isnet::prepare(source)?;
        let values = self.run_prepared(&prepared)?;
        self.cancel.check()?;
        let normalized = mask::normalize(&values)?;
        self.prepared_mask = Some(PreparedMask {
            fingerprint,
            normalized,
            fitted_width: prepared.fitted_width,
            fitted_height: prepared.fitted_height,
        });
        Ok(())
    }
}

fn load_cpu_session(model_path: &Path) -> CoreResult<Session> {
    Ok(Session::builder()?
        .with_execution_providers([ep::CPU::default().build()])
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .commit_from_file(model_path)?)
}

fn load_backend(
    runtime_path: &Path,
    model_path: &Path,
    accelerate: bool,
) -> CoreResult<(Session, &'static str)> {
    #[cfg(target_os = "macos")]
    if accelerate {
        match load_gpu_session(model_path).and_then(|session| {
            validate_contract(&session)?;
            Ok(session)
        }) {
            Ok(session) => return Ok((session, "coreml-gpu-fp32")),
            Err(error) => eprintln!("IS-Net GPU unavailable; using CPU: {error}"),
        }
    }
    #[cfg(target_os = "windows")]
    if accelerate {
        match load_directml_session(runtime_path, model_path).and_then(|session| {
            validate_contract(&session)?;
            Ok(session)
        }) {
            Ok(session) => return Ok((session, "directml-fp32")),
            Err(error) => eprintln!("IS-Net DirectML unavailable; using CPU: {error}"),
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = runtime_path;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = accelerate;
    Ok((load_cpu_session(model_path)?, "cpu"))
}

#[cfg(target_os = "windows")]
fn load_directml_session(runtime_path: &Path, model_path: &Path) -> CoreResult<Session> {
    crate::runtime::load_directml(runtime_path)?;
    // DirectML requires sequential execution and disabled memory patterns.
    // The Rust scheduler owns this session; no concurrent Run calls are allowed.
    Ok(Session::builder()?
        .with_memory_pattern(false)
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .with_parallel_execution(false)
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .with_execution_providers([
            ep::DirectML::default()
                .with_performance_preference(ep::directml::PerformancePreference::HighPerformance)
                .with_device_filter(ep::directml::DeviceFilter::Gpu)
                .build()
                .error_on_failure(),
            ep::CPU::default().build(),
        ])
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .commit_from_file(model_path)?)
}

#[cfg(target_os = "macos")]
fn load_gpu_session(model_path: &Path) -> CoreResult<Session> {
    let spec = model::find("isnet-auto-high").expect("固定模型清单缺少 IS-Net");
    // The build identity and pinned model digest prevent reuse across runtime,
    // model or provider-option changes. This contains compiled models, no images.
    let mut identity = Sha256::new();
    identity.update(ort::info());
    identity.update(spec.sha256);
    identity.update(b"isnet-mlprogram-cpu-gpu-fp32-static-v1");
    let key = format!("{:x}", identity.finalize());
    let cache = dirs::cache_dir()
        .ok_or_else(|| invalid("无法获取 GPU 编译缓存目录"))?
        .join("dev.silta.clearframe.desktop/accelerators")
        .join(key);
    std::fs::create_dir_all(&cache)?;
    Ok(Session::builder()?
        .with_intra_threads(2)
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .with_execution_providers([
            ep::CoreML::default()
                .with_model_format(ep::coreml::ModelFormat::MLProgram)
                .with_compute_units(ep::coreml::ComputeUnits::CPUAndGPU)
                .with_low_precision_accumulation_on_gpu(false)
                .with_static_input_shapes(true)
                .with_model_cache_dir(cache.to_string_lossy())
                .build()
                .error_on_failure(),
            ep::CPU::default().build(),
        ])
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .commit_from_file(model_path)?)
}

fn validate_contract(session: &Session) -> CoreResult<()> {
    if session.inputs().len() != 1 {
        return Err(invalid("IS-Net 输入数量不匹配"));
    }
    let input = &session.inputs()[0];
    let output = session
        .outputs()
        .iter()
        .find(|candidate| candidate.name() == "output_image")
        .ok_or_else(|| invalid("IS-Net 缺少 output_image 输出"))?;
    if input.name() != "input_image"
        || input.dtype().tensor_type() != Some(TensorElementType::Float32)
        || output.dtype().tensor_type() != Some(TensorElementType::Float32)
        || input.dtype().tensor_shape().map(|shape| &**shape) != Some(&INPUT_SHAPE[..])
        || output.dtype().tensor_shape().map(|shape| &**shape) != Some(&OUTPUT_SHAPE[..])
    {
        return Err(invalid("IS-Net 张量契约不匹配"));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageReport {
    pub model_id: &'static str,
    pub backend: &'static str,
    pub model_verify_ms: u64,
    pub session_load_ms: u64,
    pub decode_ms: u64,
    pub preprocess_ms: u64,
    pub inference_ms: Vec<u64>,
    pub compose_ms: u64,
    pub encode_ms: u64,
    pub source_width: u32,
    pub source_height: u32,
    pub input_name: String,
    pub output_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TensorReport {
    pub backend: &'static str,
    pub model_verify_ms: u64,
    pub session_load_ms: u64,
    pub inference_ms: u64,
    pub output_elements: usize,
}

/// Diagnostic path: run the native backend on an already captured NCHW input
/// so preprocessing differences do not contaminate backend comparisons.
pub fn infer_tensor_file(
    runtime_path: &Path,
    model_path: &Path,
    input_path: &Path,
    output_path: &Path,
) -> CoreResult<TensorReport> {
    if input_path == output_path || model_path == output_path || runtime_path == output_path {
        return Err(invalid("输出张量不能覆盖输入或模型文件"));
    }
    let (mut session, model_verify_ms, session_load_ms) =
        IsnetSession::load(runtime_path, model_path)?;
    let tensor = read_tensor(input_path)?;
    let input = isnet::PreparedInput {
        tensor,
        fitted_width: isnet::INPUT_SIDE,
        fitted_height: isnet::INPUT_SIDE,
    };
    let started = Instant::now();
    let output = session.run_prepared(&input)?;
    let inference_ms = elapsed_ms(started);
    save_tensor_noclobber(&output, output_path)?;
    Ok(TensorReport {
        backend: session.backend(),
        model_verify_ms,
        session_load_ms,
        inference_ms,
        output_elements: output.len(),
    })
}

fn read_tensor(path: &Path) -> CoreResult<Vec<f32>> {
    let expected = (3 * isnet::INPUT_SIDE * isnet::INPUT_SIDE * 4) as u64;
    if std::fs::metadata(path)?.len() != expected {
        return Err(invalid("输入张量字节数不匹配"));
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() as u64 != expected {
        return Err(invalid("输入张量读取长度不匹配"));
    }
    let tensor: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("四字节分块")))
        .collect();
    if tensor.iter().any(|value| !value.is_finite()) {
        return Err(invalid("输入张量含非有限数值"));
    }
    Ok(tensor)
}

/// Execute the native path on a caller-owned worker thread. The same
/// session is reused for every requested run; the first duration is cold.
pub fn cutout_file(
    runtime_path: &Path,
    model_path: &Path,
    input_path: &Path,
    output_path: &Path,
    tensor_path: Option<&Path>,
    runs: usize,
) -> CoreResult<StageReport> {
    if runs == 0 || runs > 20 {
        return Err(invalid("推理次数必须在 1 到 20 之间"));
    }
    if output_path == input_path
        || output_path == model_path
        || tensor_path
            .is_some_and(|path| path == input_path || path == model_path || path == output_path)
    {
        return Err(invalid("输出路径不能覆盖输入图片、模型或其他输出"));
    }
    let (mut session, model_verify_ms, session_load_ms) =
        IsnetSession::load(runtime_path, model_path)?;
    let input_name = session.session.inputs()[0].name().to_owned();
    let output_name = "output_image".to_owned();
    let started = Instant::now();
    let source = image_io::decode_image(input_path)?;
    let decode_ms = elapsed_ms(started);
    let started = Instant::now();
    let prepared = isnet::prepare(&source)?;
    let preprocess_ms = elapsed_ms(started);
    if let Some(path) = tensor_path {
        save_tensor_noclobber(&prepared.tensor, path)?;
    }
    let mut inference_ms = Vec::with_capacity(runs);
    let mut values = Vec::new();
    for _ in 0..runs {
        let started = Instant::now();
        values = session.run_prepared(&prepared)?;
        inference_ms.push(elapsed_ms(started));
    }
    let started = Instant::now();
    let result = compose(&source, &prepared, &values)?;
    let compose_ms = elapsed_ms(started);
    let started = Instant::now();
    image_io::save_png_noclobber(&result, output_path)?;
    let encode_ms = elapsed_ms(started);
    Ok(StageReport {
        model_id: "isnet-auto-high",
        backend: session.backend(),
        model_verify_ms,
        session_load_ms,
        decode_ms,
        preprocess_ms,
        inference_ms,
        compose_ms,
        encode_ms,
        source_width: source.width(),
        source_height: source.height(),
        input_name,
        output_name,
    })
}

fn compose(
    source: &RgbaImage,
    prepared: &isnet::PreparedInput,
    values: &[f32],
) -> CoreResult<RgbaImage> {
    let normalized = mask::normalize(values)?;
    let restored = isnet::restore_contain_mask(
        &normalized,
        isnet::INPUT_SIDE,
        isnet::INPUT_SIDE,
        prepared.fitted_width,
        prepared.fitted_height,
    )?;
    Ok(mask::apply_model_mask(
        source,
        &restored,
        isnet::INPUT_SIDE,
        isnet::INPUT_SIDE,
    )?)
}

fn save_tensor_noclobber(values: &[f32], path: &Path) -> CoreResult<()> {
    use std::io::Write;
    if path.extension().and_then(|extension| extension.to_str()) != Some("f32le") {
        return Err(invalid("中间张量输出需使用 .f32le 扩展名"));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    for value in values {
        temporary.write_all(&value.to_le_bytes())?;
    }
    temporary.as_file_mut().sync_all()?;
    temporary.persist_noclobber(path)?;
    Ok(())
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires local native runtime and pinned IS-Net/MobileSAM models"]
    fn native_accelerator_error_retries_cpu_and_rechecks_model() {
        let runtime = PathBuf::from(std::env::var_os("S08_NATIVE_RUNTIME").unwrap());
        let models = PathBuf::from(std::env::var_os("S08_MODEL_DIR").unwrap());
        let path_for = |id| {
            let spec = model::find(id).unwrap();
            models.join(format!("{}-{}.onnx", spec.id, spec.revision))
        };
        let (mut session, _, _) =
            IsnetSession::load_cpu(&runtime, &path_for("isnet-auto-high")).unwrap();
        let source = image_io::decode_image(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/images/hard-edge-opaque.png"),
        )
        .unwrap();
        let input = isnet::prepare(&source).unwrap();
        let expected = session.run_prepared(&input).unwrap();
        let decoder = path_for("mobilesam-mask-decoder");
        model::verify_file(&decoder, model::find("mobilesam-mask-decoder").unwrap()).unwrap();
        let accelerator = if cfg!(target_os = "windows") {
            "directml-fp32"
        } else {
            "coreml-gpu-fp32"
        };
        // Inject an ORT execution failure using a valid session with incompatible
        // inputs. This tests recovery, not a claim to reproduce a GPU driver crash.
        session.session = load_cpu_session(&decoder).unwrap();
        session.backend = accelerator;
        assert_eq!(session.run_prepared(&input).unwrap(), expected);
        assert_eq!(session.backend(), "cpu");

        // A replaced file must fail its digest before CPU can reopen it.
        let directory = tempfile::tempdir().unwrap();
        let replaced = directory.path().join("replaced.onnx");
        std::fs::write(&replaced, b"invalid replacement").unwrap();
        session.model_path = replaced;
        session.session = load_cpu_session(&decoder).unwrap();
        session.backend = accelerator;
        assert!(session.run_prepared(&input).is_err());
        assert_ne!(
            session.backend(),
            "cpu",
            "failed reload must not report CPU success"
        );
        session.cancel_handle().cancel().unwrap();
        assert!(session.run_prepared(&input).is_err());
    }

    #[test]
    fn mask_cache_key_includes_dimensions_and_every_rgba_channel() {
        let source = RgbaImage::from_pixel(2, 1, image::Rgba([20, 30, 40, 128]));
        let fingerprint = source_fingerprint(&source);
        assert_eq!(fingerprint, source_fingerprint(&source.clone()));
        let reshaped = RgbaImage::from_raw(1, 2, source.as_raw().clone()).unwrap();
        assert_ne!(fingerprint, source_fingerprint(&reshaped));
        for channel in 0..4 {
            let mut changed = source.clone();
            changed.get_pixel_mut(1, 0)[channel] += 1;
            assert_ne!(fingerprint, source_fingerprint(&changed));
        }
    }
}
