//! Explicit native batch benchmark. The timer includes admission, decode,
//! inference, PNG encoding and commit; optional concurrent ZIP overlaps this
//! timer, while output validation runs afterward.

use cutout_core::{
    image_io,
    jobs::{BatchItemTimings, TaskState, Workspace},
    model,
    model_store::{ModelState, ModelStore},
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    error::Error,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type BenchResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Plan {
    schema_version: u32,
    description: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    path: PathBuf,
    reference: Option<PathBuf>,
    repeat: usize,
}

struct ResolvedCase {
    path: PathBuf,
    reference: Option<PathBuf>,
    repeat: usize,
    dimensions: (u32, u32),
    bytes: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ZipMode {
    None,
    After,
    During,
}

impl ZipMode {
    fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::After => "after",
            Self::During => "during",
        }
    }
}

pub fn run(
    runtime: &Path,
    model_dir: &Path,
    plan_path: &Path,
    output_dir: &Path,
    zip_mode: ZipMode,
) -> BenchResult<()> {
    let runtime = fs::canonicalize(runtime)?;
    if !runtime.is_file() {
        return Err("ORT 动态库不是普通文件".into());
    }
    let model_dir = fs::canonicalize(model_dir)?;
    let plan_path = fs::canonicalize(plan_path)?;
    let plan_bytes = fs::read(&plan_path)?;
    let plan: Plan = serde_json::from_slice(&plan_bytes)?;
    if plan.schema_version != 1 || plan.cases.is_empty() || plan.description.trim().is_empty() {
        return Err("批量基准计划无效".into());
    }
    let parent = plan_path.parent().ok_or("基准计划路径无效")?;
    let mut cases = Vec::with_capacity(plan.cases.len());
    let mut inputs = Vec::new();
    let mut case_indices = Vec::new();
    for (index, case) in plan.cases.iter().enumerate() {
        if case.repeat == 0 || case.repeat > 1000 || inputs.len() + case.repeat > 1000 {
            return Err("单次基准应包含 1–1000 项".into());
        }
        let path = fs::canonicalize(parent.join(&case.path))?;
        let reference = case
            .reference
            .as_ref()
            .map(|path| fs::canonicalize(parent.join(path)))
            .transpose()?;
        let dimensions = image::image_dimensions(&path)?;
        let bytes = fs::metadata(&path)?.len();
        for _ in 0..case.repeat {
            inputs.push(path.clone());
            case_indices.push(index);
        }
        cases.push(ResolvedCase {
            path,
            reference,
            repeat: case.repeat,
            dimensions,
            bytes,
        });
    }
    let model_spec = model::find("isnet-auto-high").ok_or("IS-Net 模型清单缺失")?;
    let store = Arc::new(ModelStore::new(model_dir.clone(), |_| {})?);
    let verification_start = Instant::now();
    loop {
        let status = store.snapshot();
        if status.features.automatic.files_verified {
            break;
        }
        let model = status
            .models
            .iter()
            .find(|model| model.spec.id == model_spec.id)
            .ok_or("IS-Net 模型状态缺失")?;
        if matches!(model.state, ModelState::Missing | ModelState::Failed) {
            return Err(format!("IS-Net 模型不可用: {:?}: {:?}", model.state, model.error).into());
        }
        if verification_start.elapsed() > Duration::from_secs(120) {
            return Err("模型验证超时".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
    let model_verification_ms = verification_start.elapsed().as_millis();
    let (notify, changed) = mpsc::sync_channel(1);
    let workspace = Arc::new(Workspace::new_with_inference(
        move |_| {
            let _ = notify.try_send(());
        },
        runtime.clone(),
        Arc::clone(&store),
    )?);
    fs::create_dir(output_dir)?;
    let output_dir = fs::canonicalize(output_dir)?;
    let started_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let start = Instant::now();
    let batch_id = workspace.create_batch(&output_dir, &inputs)?;
    let running_zip = if zip_mode == ZipMode::During {
        let workspace = Arc::clone(&workspace);
        let destination = output_dir.join("batch-results.zip");
        let expected = inputs.len();
        Some(thread::spawn(move || {
            export_zip_report(&workspace, batch_id, expected, &destination)
        }))
    } else {
        None
    };
    let summary = loop {
        let snapshot = workspace.snapshot();
        let summary = snapshot
            .batches
            .iter()
            .find(|batch| batch.batch_id == batch_id)
            .ok_or("批次状态丢失")?;
        if summary.succeeded + summary.failed + summary.cancelled == inputs.len() {
            break summary.clone();
        }
        if start.elapsed() > Duration::from_secs(3600) {
            workspace.cancel_batch(batch_id)?;
            return Err("批量基准超过一小时，已请求取消".into());
        }
        let _ = changed.recv_timeout(Duration::from_millis(250));
    };
    let wall_ms = start.elapsed().as_millis();
    let during_zip_result = running_zip.map(|worker| {
        worker
            .join()
            .unwrap_or_else(|_| Err("ZIP 工作线程异常退出".into()))
    });
    let timings = workspace.batch_timings(batch_id)?;
    let validation_start = Instant::now();
    // Decode once after timing so EXIF orientation and color transforms match
    // the same image path used by the batch worker without warming the input
    // cache before the throughput measurement.
    for case in &mut cases {
        case.dimensions = image_io::decode_image(&case.path)?.dimensions();
    }
    let mut output_bytes = 0_u64;
    let mut errors = Vec::new();
    let mut case_hashes = (0..cases.len()).map(|_| HashSet::new()).collect::<Vec<_>>();
    let mut quality = vec![None; cases.len()];
    for offset in (0..inputs.len()).step_by(200) {
        let page = workspace.batch_items(batch_id, offset, 200)?;
        for (page_index, item) in page.items.iter().enumerate() {
            let index = offset + page_index;
            let case_index = case_indices[index];
            if item.state != TaskState::Succeeded {
                errors.push(format!(
                    "{}: {:?}: {:?}",
                    item.display_name, item.state, item.error
                ));
                continue;
            }
            let output = output_dir.join(&item.output_name);
            output_bytes += fs::metadata(&output)?.len();
            let pixels = image_io::decode_png(&output)?;
            if pixels.dimensions() != cases[case_index].dimensions {
                errors.push(format!("{}: 输出尺寸与输入不一致", item.output_name));
                continue;
            }
            case_hashes[case_index].insert(format!("{:x}", Sha256::digest(pixels.as_raw())));
            if quality[case_index].is_none() {
                if let Some(reference) = &cases[case_index].reference {
                    let expected = image_io::decode_png(reference)?;
                    quality[case_index] = Some(compare_rgba(&pixels, &expected)?);
                }
            }
        }
    }
    for (index, hashes) in case_hashes.iter().enumerate() {
        if hashes.len() != 1 {
            errors.push(format!(
                "{}: 重复输入产生 {} 种输出像素",
                cases[index].path.display(),
                hashes.len()
            ));
        }
    }
    if summary.succeeded == inputs.len()
        && timings.iter().any(|timing| {
            timing.prepare_us.is_none()
                || timing.model_load_us.is_none()
                || timing.inference_us.is_none()
                || timing.compose_queue_us.is_none()
                || timing.write_us.is_none()
        })
    {
        errors.push("成功项缺少分阶段计时".to_owned());
    }
    let validation_ms = validation_start.elapsed().as_millis();
    let zip_result = match zip_mode {
        ZipMode::None => None,
        ZipMode::After if summary.succeeded == inputs.len() && errors.is_empty() => {
            Some(export_zip_report(
                &workspace,
                batch_id,
                inputs.len(),
                &output_dir.join("batch-results.zip"),
            ))
        }
        ZipMode::After => None,
        ZipMode::During => during_zip_result,
    };
    let zip_report = match zip_result {
        Some(Ok(report)) => Some(report),
        Some(Err(error)) => {
            errors.push(format!("ZIP 导出失败: {error}"));
            None
        }
        None => None,
    };
    let report_cases = cases
        .iter()
        .enumerate()
        .map(|(index, case)| {
            json!({
                "path": case.path,
                "reference": case.reference,
                "repeat": case.repeat,
                "width": case.dimensions.0,
                "height": case.dimensions.1,
                "sourceBytes": case.bytes,
                "uniqueOutputPixelHashes": case_hashes[index].len(),
                "referenceDifference": quality[index],
            })
        })
        .collect::<Vec<_>>();
    let report = json!({
        "schemaVersion": 1,
        "kind": "native-batch-baseline",
        "description": plan.description,
        "platform": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "availableParallelism": thread::available_parallelism().map(|value| value.get()).ok(),
        "startedUnixMs": started_at,
        "planSha256": format!("{:x}", Sha256::digest(&plan_bytes)),
        "runtimeSha256": hash_file(&runtime)?,
        "modelId": model_spec.id,
        "modelRevision": model_spec.revision,
        "modelSha256": model_spec.sha256,
        "modelVerificationMs": model_verification_ms,
        "inputCount": inputs.len(),
        "distinctInputCount": cases.len(),
        "inputBytesIncludingRepeats": cases.iter().map(|case| case.bytes * case.repeat as u64).sum::<u64>(),
        "inputPixelsIncludingRepeats": cases.iter().map(|case| u64::from(case.dimensions.0) * u64::from(case.dimensions.1) * case.repeat as u64).sum::<u64>(),
        "succeeded": summary.succeeded,
        "failed": summary.failed,
        "cancelled": summary.cancelled,
        "wallMs": wall_ms,
        "succeededImagesPerMinute": summary.succeeded as f64 * 60_000.0 / wall_ms.max(1) as f64,
        "validationMs": validation_ms,
        "stageTimingUs": {
            "prepare": stage_stats(&timings, |timing| timing.prepare_us),
            "modelLoad": stage_stats(&timings, |timing| timing.model_load_us),
            "inference": stage_stats(&timings, |timing| timing.inference_us),
            "composeQueue": stage_stats(&timings, |timing| timing.compose_queue_us),
            "write": stage_stats(&timings, |timing| timing.write_us),
        },
        "outputBytes": output_bytes,
        "zipMode": zip_mode.name(),
        "zip": zip_report,
        "cases": report_cases,
        "errors": errors,
    });
    let report_path = output_dir.join("report.json");
    let mut file = File::create_new(&report_path)?;
    serde_json::to_writer_pretty(&mut file, &report)?;
    writeln!(file)?;
    file.sync_all()?;
    println!("{}", report_path.display());
    if summary.succeeded != inputs.len() || !errors.is_empty() {
        return Err("批量基准未全部通过，请查看 report.json".into());
    }
    Ok(())
}

fn export_zip_report(
    workspace: &Workspace,
    batch_id: u64,
    expected: usize,
    destination: &Path,
) -> BenchResult<serde_json::Value> {
    let succeeded_at_start = workspace
        .snapshot()
        .batches
        .iter()
        .find(|batch| batch.batch_id == batch_id)
        .ok_or("ZIP 启动时批次状态丢失")?
        .succeeded;
    let started = Instant::now();
    let items = workspace.export_batch_zip(batch_id, destination)?;
    let wall_ms = started.elapsed().as_millis();
    if items != expected {
        return Err(format!("ZIP 仅包含 {items}/{expected} 项").into());
    }
    Ok(json!({
        "path": destination,
        "items": items,
        "succeededAtStart": succeeded_at_start,
        "wallMs": wall_ms,
        "bytes": fs::metadata(destination)?.len(),
        "sha256": hash_file(destination)?,
    }))
}

fn stage_stats(
    timings: &[BatchItemTimings],
    select: impl Fn(&BatchItemTimings) -> Option<u64>,
) -> serde_json::Value {
    let mut values = timings.iter().filter_map(select).collect::<Vec<_>>();
    values.sort_unstable();
    let percentile = |percent: usize| {
        values
            .get((values.len() * percent).div_ceil(100).saturating_sub(1))
            .copied()
    };
    json!({
        "sampleCount": values.len(),
        "missingCount": timings.len() - values.len(),
        "p50Us": percentile(50),
        "p95Us": percentile(95),
        "maxUs": values.last().copied(),
        "sumUs": values.iter().copied().sum::<u64>(),
    })
}

fn compare_rgba(
    actual: &image::RgbaImage,
    expected: &image::RgbaImage,
) -> BenchResult<serde_json::Value> {
    if actual.dimensions() != expected.dimensions() {
        return Err("参考 PNG 尺寸不一致".into());
    }
    let mut alpha_sum = 0_u64;
    let mut alpha_max = 0_u8;
    let mut premultiplied_sum = 0_u64;
    for (actual, expected) in actual.pixels().zip(expected.pixels()) {
        let alpha_diff = actual[3].abs_diff(expected[3]);
        alpha_sum += u64::from(alpha_diff);
        alpha_max = alpha_max.max(alpha_diff);
        for channel in 0..3 {
            let a = u16::from(actual[channel]) * u16::from(actual[3]) / 255;
            let b = u16::from(expected[channel]) * u16::from(expected[3]) / 255;
            premultiplied_sum += u64::from(a.abs_diff(b));
        }
    }
    let pixels = u64::from(actual.width()) * u64::from(actual.height());
    Ok(json!({
        "alphaMae255": alpha_sum as f64 / pixels as f64,
        "alphaMax255": alpha_max,
        "premultipliedRgbMae255": premultiplied_sum as f64 / (pixels * 3) as f64,
    }))
}

fn hash_file(path: &Path) -> BenchResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
