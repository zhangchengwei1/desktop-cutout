//! Explicit macOS-only diagnostic. Build with --features coreml-probe, then run
//! with S04_NATIVE_RUNTIME and --ignored;
//! availability alone is not evidence that model nodes execute on CoreML.
#![cfg(all(target_os = "macos", feature = "coreml-probe"))]
#[cfg(target_os = "macos")]
fn provider_counts(profile: &str) -> std::collections::BTreeMap<String, usize> {
    let events: serde_json::Value =
        serde_json::from_slice(&std::fs::read(profile).unwrap()).unwrap();
    let mut providers = std::collections::BTreeMap::<String, usize>::new();
    for event in events.as_array().unwrap() {
        if event["cat"] != "Node" {
            continue;
        }
        if let Some(provider) = event["args"]["provider"].as_str() {
            *providers.entry(provider.to_owned()).or_default() += 1;
        }
    }
    providers
}
#[cfg(target_os = "macos")]
#[test]
#[ignore]
fn coreml_available_in_pinned_runtime() {
    use ort::ep::{self, ExecutionProvider};
    use std::path::PathBuf;

    let runtime = std::env::var("S04_NATIVE_RUNTIME").expect("set S04_NATIVE_RUNTIME");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime = std::fs::canonicalize(root.join(runtime)).unwrap();
    let _ = ort::init_from(&runtime).unwrap().commit();
    let available = ep::CoreML::default().is_available().unwrap();
    println!("CoreML compiled into native ORT: {available}");
    assert!(available, "pinned ORT does not contain CoreML");
}

#[cfg(target_os = "macos")]
#[test]
#[ignore]
fn vitmatte_coreml_profile() {
    use cutout_core::{matte, model};
    use flate2::read::GzDecoder;
    use ort::{ep, session::Session, value::TensorRef};
    use std::{io::Read, path::PathBuf, time::Instant};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime = std::env::var("S04_NATIVE_RUNTIME").expect("set S04_NATIVE_RUNTIME");
    let runtime = std::fs::canonicalize(root.join(runtime)).unwrap();
    let _ = ort::init_from(&runtime).unwrap().commit();
    let model_path = root.join("test-output/models/vitmatte-high.onnx");
    model::verify_file(&model_path, model::find("vitmatte-high").unwrap()).unwrap();
    let mut model = std::fs::read(model_path).unwrap();
    matte::fix_output_metadata(&mut model).unwrap();
    let fixture = root.join("tests/fixtures/reference/model-s04");
    let mut bytes = Vec::new();
    GzDecoder::new(std::fs::File::open(fixture.join("matte-input.f32le.gz")).unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    let values: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    let output_dir = root.join("test-output/s04-coreml");
    std::fs::create_dir_all(&output_dir).unwrap();
    let profile_prefix = output_dir.join("vitmatte-profile");
    let start = Instant::now();
    let mut session = Session::builder()
        .unwrap()
        .with_execution_providers([
            ep::CoreML::default()
                .with_model_format(ep::coreml::ModelFormat::MLProgram)
                .build()
                .error_on_failure(),
            ep::CPU::default().build(),
        ])
        .unwrap()
        .with_profiling(&profile_prefix)
        .unwrap()
        .commit_from_memory(&model)
        .unwrap();
    let load_ms = start.elapsed().as_millis();
    let input = TensorRef::from_array_view(([1, 4, 64, 96], values.as_slice())).unwrap();
    let start = Instant::now();
    let output = session.run(ort::inputs!["pixel_values" => input]).unwrap();
    let run_ms = start.elapsed().as_millis();
    let (shape, alpha) = output
        .get("alphas")
        .unwrap()
        .try_extract_tensor::<f32>()
        .unwrap();
    assert_eq!(&**shape, &[1, 1, 64, 96]);
    let trimap = {
        let mut result = Vec::new();
        GzDecoder::new(std::fs::File::open(fixture.join("trimap.u8.gz")).unwrap())
            .read_to_end(&mut result)
            .unwrap();
        result
    };
    let mask = matte::alpha(alpha, &trimap, (96, 64), (96, 64)).unwrap();
    let expected = {
        let mut result = Vec::new();
        GzDecoder::new(std::fs::File::open(fixture.join("matte-mask.u8.gz")).unwrap())
            .read_to_end(&mut result)
            .unwrap();
        result
    };
    let mae = mask
        .iter()
        .zip(&expected)
        .map(|(a, b)| f64::from(a.abs_diff(*b)))
        .sum::<f64>()
        / mask.len() as f64;
    let max = mask
        .iter()
        .zip(&expected)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    drop(output);
    let profile = session.end_profiling().unwrap();
    let providers = provider_counts(&profile);
    let mut cpu = Session::builder()
        .unwrap()
        .with_execution_providers([ep::CPU::default().build()])
        .unwrap()
        .commit_from_memory(&model)
        .unwrap();
    let input = TensorRef::from_array_view(([1, 4, 64, 96], values.as_slice())).unwrap();
    let start = Instant::now();
    let cpu_output = cpu.run(ort::inputs!["pixel_values" => input]).unwrap();
    let cpu_ms = start.elapsed().as_millis();
    let (_, cpu_alpha) = cpu_output
        .get("alphas")
        .unwrap()
        .try_extract_tensor::<f32>()
        .unwrap();
    let cpu_mask = matte::alpha(cpu_alpha, &trimap, (96, 64), (96, 64)).unwrap();
    assert_eq!(
        cpu_mask, expected,
        "CPU reference changed during CoreML comparison"
    );
    println!("ViTMatte CoreML candidate: load {load_ms} ms, CoreML run {run_ms} ms, CPU run {cpu_ms} ms, alpha MAE {mae:.4}, max {max}, node counts {providers:?}, profile {profile}");
    assert!(
        providers
            .get("CoreMLExecutionProvider")
            .copied()
            .unwrap_or(0)
            > 0,
        "CoreML registered but ran no nodes"
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore]
fn mobilesam_encoder_coreml_profile() {
    use cutout_core::model;
    use flate2::read::GzDecoder;
    use ort::{ep, session::Session, value::TensorRef};
    use std::{io::Read, path::PathBuf, time::Instant};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime = std::env::var("S04_NATIVE_RUNTIME").expect("set S04_NATIVE_RUNTIME");
    let runtime = std::fs::canonicalize(root.join(runtime)).unwrap();
    let _ = ort::init_from(&runtime).unwrap().commit();
    let model_path = root.join("test-output/models/mobilesam-image-encoder.onnx");
    model::verify_file(&model_path, model::find("mobilesam-image-encoder").unwrap()).unwrap();
    let mut bytes = Vec::new();
    GzDecoder::new(
        std::fs::File::open(root.join("tests/fixtures/reference/model-s04/sam-input.f32le.gz"))
            .unwrap(),
    )
    .read_to_end(&mut bytes)
    .unwrap();
    let values: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    let output_dir = root.join("test-output/s04-coreml");
    std::fs::create_dir_all(&output_dir).unwrap();
    let profile_prefix = output_dir.join("sam-encoder-profile");
    let start = Instant::now();
    let mut coreml = Session::builder()
        .unwrap()
        .with_execution_providers([
            ep::CoreML::default().build().error_on_failure(),
            ep::CPU::default().build(),
        ])
        .unwrap()
        .with_profiling(&profile_prefix)
        .unwrap()
        .commit_from_file(&model_path)
        .unwrap();
    let load_ms = start.elapsed().as_millis();
    let input = TensorRef::from_array_view(([683, 1024, 3], values.as_slice())).unwrap();
    let start = Instant::now();
    let output = coreml.run(ort::inputs!["input_image" => input]).unwrap();
    let run_ms = start.elapsed().as_millis();
    let (shape, core_embedding) = output
        .get("image_embeddings")
        .unwrap()
        .try_extract_tensor::<f32>()
        .unwrap();
    assert_eq!(&**shape, &[1, 256, 64, 64]);
    let core_embedding = core_embedding.to_vec();
    drop(output);
    let profile = coreml.end_profiling().unwrap();
    let providers = provider_counts(&profile);

    let mut cpu = Session::builder()
        .unwrap()
        .with_execution_providers([ep::CPU::default().build()])
        .unwrap()
        .commit_from_file(&model_path)
        .unwrap();
    let input = TensorRef::from_array_view(([683, 1024, 3], values.as_slice())).unwrap();
    let start = Instant::now();
    let output = cpu.run(ort::inputs!["input_image" => input]).unwrap();
    let cpu_ms = start.elapsed().as_millis();
    let (_, cpu_embedding) = output
        .get("image_embeddings")
        .unwrap()
        .try_extract_tensor::<f32>()
        .unwrap();
    let errors: Vec<f64> = core_embedding
        .iter()
        .zip(cpu_embedding)
        .map(|(a, b)| f64::from((a - b).abs()))
        .collect();
    let mae = errors.iter().sum::<f64>() / errors.len() as f64;
    let max = errors.iter().copied().fold(0.0, f64::max);
    println!("MobileSAM encoder CoreML candidate: load {load_ms} ms, CoreML run {run_ms} ms, CPU run {cpu_ms} ms, embedding MAE {mae:.6}, max {max:.6}, node counts {providers:?}, profile {profile}");
    assert!(
        providers
            .get("CoreMLExecutionProvider")
            .copied()
            .unwrap_or(0)
            > 0,
        "CoreML registered but ran no nodes"
    );
}

#[test]
#[ignore]
fn isnet_coreml_profile() {
    use cutout_core::{mask, model};
    use flate2::read::GzDecoder;
    use ort::{ep, session::Session, value::TensorRef};
    use std::{io::Read, path::PathBuf, time::Instant};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime = std::env::var("S04_NATIVE_RUNTIME").expect("set S04_NATIVE_RUNTIME");
    let runtime = std::fs::canonicalize(root.join(runtime)).unwrap();
    let _ = ort::init_from(&runtime).unwrap().commit();
    let model_path = root.join("test-output/models/isnet-auto-high.onnx");
    model::verify_file(&model_path, model::find("isnet-auto-high").unwrap()).unwrap();
    let mut bytes = Vec::new();
    GzDecoder::new(
        std::fs::File::open(
            root.join("tests/fixtures/reference/hard-edge-opaque.png.tensor.f32le.gz"),
        )
        .unwrap(),
    )
    .read_to_end(&mut bytes)
    .unwrap();
    let values: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    assert_eq!(values.len(), 3 * 1024 * 1024);
    let output_dir = root.join("test-output/s04-coreml");
    std::fs::create_dir_all(&output_dir).unwrap();
    let profile_prefix = output_dir.join("isnet-profile");
    let start = Instant::now();
    let mut coreml = Session::builder()
        .unwrap()
        .with_execution_providers([
            ep::CoreML::default().build().error_on_failure(),
            ep::CPU::default().build(),
        ])
        .unwrap()
        .with_profiling(&profile_prefix)
        .unwrap()
        .commit_from_file(&model_path)
        .unwrap();
    let load_ms = start.elapsed().as_millis();
    let input = TensorRef::from_array_view(([1, 3, 1024, 1024], values.as_slice())).unwrap();
    let start = Instant::now();
    let output = coreml.run(ort::inputs!["input_image" => input]).unwrap();
    let run_ms = start.elapsed().as_millis();
    let (shape, core_output) = output
        .get("output_image")
        .unwrap()
        .try_extract_tensor::<f32>()
        .unwrap();
    assert_eq!(&**shape, &[1, 1, 1024, 1024]);
    let core_output = core_output.to_vec();
    assert!(core_output.iter().all(|value| value.is_finite()));
    drop(output);
    let profile = coreml.end_profiling().unwrap();
    let providers = provider_counts(&profile);
    drop(coreml);

    let mut cpu = Session::builder()
        .unwrap()
        .with_execution_providers([ep::CPU::default().build()])
        .unwrap()
        .commit_from_file(&model_path)
        .unwrap();
    let input = TensorRef::from_array_view(([1, 3, 1024, 1024], values.as_slice())).unwrap();
    let start = Instant::now();
    let output = cpu.run(ort::inputs!["input_image" => input]).unwrap();
    let cpu_ms = start.elapsed().as_millis();
    let (_, cpu_output) = output
        .get("output_image")
        .unwrap()
        .try_extract_tensor::<f32>()
        .unwrap();
    let errors: Vec<f64> = core_output
        .iter()
        .zip(cpu_output)
        .map(|(a, b)| f64::from((a - b).abs()))
        .collect();
    let mae = errors.iter().sum::<f64>() / errors.len() as f64;
    let max = errors.iter().copied().fold(0.0, f64::max);
    let core_mask = mask::normalize(&core_output).unwrap();
    let cpu_mask = mask::normalize(cpu_output).unwrap();
    let mask_mae = core_mask
        .iter()
        .zip(&cpu_mask)
        .map(|(a, b)| f64::from(a.abs_diff(*b)))
        .sum::<f64>()
        / core_mask.len() as f64;
    let mask_max = core_mask
        .iter()
        .zip(&cpu_mask)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    let mask_changed = core_mask
        .iter()
        .zip(&cpu_mask)
        .filter(|(a, b)| a != b)
        .count();
    println!("IS-Net CoreML candidate: load {load_ms} ms, CoreML run {run_ms} ms, CPU run {cpu_ms} ms, raw output MAE {mae:.6}, max {max:.6}; normalized mask MAE {mask_mae:.4}/255, max {mask_max}/255, changed {mask_changed}/{}; node counts {providers:?}, profile {profile}", core_mask.len());
    assert!(
        providers
            .get("CoreMLExecutionProvider")
            .copied()
            .unwrap_or(0)
            > 0,
        "CoreML registered but ran no nodes"
    );
}
