//! Opt-in direct inference timings. Result caching must not hide model cost.
#![cfg(any(target_os = "macos", target_os = "windows"))]
#[cfg(target_os = "macos")]
use cutout_core::runtime;
use cutout_core::{image_io, inference::IsnetSession, isnet, mask, model};
use ort::{ep, session::Session, value::TensorRef};
use std::{path::PathBuf, time::Instant};

const GPU_BACKEND: &str = if cfg!(target_os = "windows") {
    "directml-fp32"
} else {
    "coreml-gpu-fp32"
};

#[test]
#[ignore = "requires local runtime, verified models, an authorized image and a supported GPU"]
fn production_gpu_matches_cpu_quality_gate() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime_path = PathBuf::from(std::env::var_os("S08_NATIVE_RUNTIME").unwrap());
    let spec = model::find("isnet-auto-high").unwrap();
    let model_path = PathBuf::from(std::env::var_os("S08_MODEL_DIR").unwrap())
        .join(format!("{}-{}.onnx", spec.id, spec.revision));
    let (mut gpu, _, gpu_load) = IsnetSession::load(&runtime_path, &model_path).unwrap();
    assert_eq!(
        gpu.backend(),
        GPU_BACKEND,
        "CPU fallback is not GPU validation"
    );
    let (mut cpu, _, _) = IsnetSession::load_cpu(&runtime_path, &model_path).unwrap();
    let mut paths = vec![PathBuf::from(std::env::var_os("S08_PERF_IMAGE").unwrap())];
    paths.extend(
        [
            "hard-edge-opaque.png",
            "portrait-strands.png",
            "wide-alpha.png",
            "low-contrast.png",
            "gamma-tagged.png",
            "icc-tagged.png",
            "exif-orient6.jpg",
        ]
        .map(|name| root.join("tests/fixtures/images").join(name)),
    );
    let mut reports = Vec::new();
    for path in paths {
        let source = image_io::decode_image(&path).unwrap();
        let input = isnet::prepare(&source).unwrap();
        let started = Instant::now();
        let expected = cpu.run_prepared(&input).unwrap();
        let cpu_ms = started.elapsed().as_millis();
        let started = Instant::now();
        let actual = gpu.run_prepared(&input).unwrap();
        let gpu_ms = started.elapsed().as_millis();
        assert_eq!(gpu.backend(), GPU_BACKEND);
        let expected = mask::normalize(&expected).unwrap();
        let actual = mask::normalize(&actual).unwrap();
        let error = |left: &[u8], right: &[u8]| {
            let max = left
                .iter()
                .zip(right)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            let mae = left
                .iter()
                .zip(right)
                .map(|(a, b)| f64::from(a.abs_diff(*b)))
                .sum::<f64>()
                / left.len() as f64;
            let changed = left.iter().zip(right).filter(|(a, b)| a != b).count();
            (max, mae, changed)
        };
        let (max, mae, changed) = error(&expected, &actual);
        assert!(
            max <= 1 && mae <= 0.01,
            "{}: max={max}, mae={mae}",
            path.display()
        );
        let compose = |values: &[u8]| {
            let restored = isnet::restore_contain_mask(
                values,
                1024,
                1024,
                input.fitted_width,
                input.fitted_height,
            )
            .unwrap();
            mask::apply_model_mask(&source, &restored, 1024, 1024).unwrap()
        };
        let expected_rgba = compose(&expected);
        let actual_rgba = compose(&actual);
        let (rgba_max, rgba_mae, rgba_changed) =
            error(expected_rgba.as_raw(), actual_rgba.as_raw());
        assert!(rgba_max <= 1 && rgba_mae <= 0.01);
        assert_eq!(expected_rgba.dimensions(), source.dimensions());
        assert!(
            actual_rgba
                .pixels()
                .zip(source.pixels())
                .all(|(result, original)| result.0[..3] == original.0[..3]
                    && result[3] <= original[3])
        );
        let report = serde_json::json!({"image":path.file_name().unwrap().to_string_lossy(),"cpuMs":cpu_ms,"gpuMs":gpu_ms,"maskMax":max,"maskMae":mae,"maskChanged":changed,"rgbaMax":rgba_max,"rgbaMae":rgba_mae,"rgbaChanged":rgba_changed});
        println!("{report}");
        reports.push(report);
    }
    // Reopen the production session, then time direct runs: no prepared-result cache.
    drop(gpu);
    let (mut gpu, _, reload_ms) = IsnetSession::load(&runtime_path, &model_path).unwrap();
    assert_eq!(gpu.backend(), GPU_BACKEND);
    let source =
        image_io::decode_image(&PathBuf::from(std::env::var_os("S08_PERF_IMAGE").unwrap()))
            .unwrap();
    let input = isnet::prepare(&source).unwrap();
    let mut runs = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        gpu.run_prepared(&input).unwrap();
        runs.push(started.elapsed().as_millis());
    }
    gpu.cancel_handle().cancel().unwrap();
    assert!(
        gpu.run_prepared(&input).is_err(),
        "cancelled GPU jobs must not retry on CPU"
    );
    assert_eq!(gpu.backend(), GPU_BACKEND);
    let directory = root.join(format!("test-output/acceleration-{}", std::env::consts::OS));
    std::fs::create_dir_all(&directory).unwrap();
    let report = serde_json::json!({"firstLoadMs":gpu_load,"reloadMs":reload_ms,"uncachedInferenceMs":runs,"samples":reports});
    std::fs::write(
        directory.join("production-quality.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!(
        "first load {gpu_load} ms; session reload {reload_ms} ms; direct inference {runs:?} ms"
    );
}

#[test]
#[cfg(target_os = "macos")]
#[ignore = "requires local native runtime, verified models and a permitted image"]
fn compare_isnet_cpu_and_gpu() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime_path = PathBuf::from(std::env::var_os("S08_NATIVE_RUNTIME").unwrap());
    let spec = model::find("isnet-auto-high").unwrap();
    let model_path = PathBuf::from(std::env::var_os("S08_MODEL_DIR").unwrap())
        .join(format!("{}-{}.onnx", spec.id, spec.revision));
    model::verify_file(&model_path, spec).unwrap();
    runtime::initialize(&runtime_path).unwrap();
    let image_path = PathBuf::from(std::env::var_os("S08_PERF_IMAGE").unwrap());
    let source = image_io::decode_image(&image_path).unwrap();
    let prepared = isnet::prepare(&source).unwrap();
    let directory = root.join("test-output/acceleration-20261009");
    std::fs::create_dir_all(&directory).unwrap();
    let mut reference = None;
    let mut reports = Vec::new();
    for (name, threads, format) in [
        ("cpu-default", 0, None),
        ("cpu-2", 2, None),
        ("cpu-4", 4, None),
        (
            "gpu-neuralnetwork",
            2,
            Some(ep::coreml::ModelFormat::NeuralNetwork),
        ),
        ("gpu-mlprogram", 2, Some(ep::coreml::ModelFormat::MLProgram)),
        (
            "gpu-mlprogram-cached",
            2,
            Some(ep::coreml::ModelFormat::MLProgram),
        ),
        ("cpu-default-repeat", 0, None),
    ] {
        let mut builder = Session::builder()
            .unwrap()
            .with_intra_threads(threads)
            .unwrap();
        if let Some(format) = format {
            let cache = directory.join(format!("compiled-{format:?}"));
            std::fs::create_dir_all(&cache).unwrap();
            builder = builder
                .with_execution_providers([
                    ep::CoreML::default()
                        .with_model_format(format)
                        .with_compute_units(ep::coreml::ComputeUnits::CPUAndGPU)
                        .with_low_precision_accumulation_on_gpu(false)
                        .with_static_input_shapes(true)
                        .with_model_cache_dir(cache.display().to_string())
                        .build()
                        .error_on_failure(),
                    ep::CPU::default().build(),
                ])
                .unwrap()
                .with_profiling(directory.join(name))
                .unwrap();
        } else {
            builder = builder
                .with_execution_providers([ep::CPU::default().build()])
                .unwrap();
        }
        let started = Instant::now();
        let mut session = builder.commit_from_file(&model_path).unwrap();
        let load_ms = started.elapsed().as_millis();
        let mut runs = Vec::new();
        let mut output_mask = Vec::new();
        for _ in 0..3 {
            let input =
                TensorRef::from_array_view(([1_i64, 3, 1024, 1024], prepared.tensor.as_slice()))
                    .unwrap();
            let started = Instant::now();
            let output = session.run(ort::inputs![input]).unwrap();
            let (_, values) = output["output_image"].try_extract_tensor::<f32>().unwrap();
            runs.push(started.elapsed().as_millis());
            output_mask = mask::normalize(values).unwrap();
        }
        let baseline = reference.get_or_insert_with(|| output_mask.clone());
        let changed = baseline
            .iter()
            .zip(&output_mask)
            .filter(|(a, b)| a != b)
            .count();
        let maximum = baseline
            .iter()
            .zip(&output_mask)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        let mae = baseline
            .iter()
            .zip(&output_mask)
            .map(|(a, b)| f64::from(a.abs_diff(*b)))
            .sum::<f64>()
            / baseline.len() as f64;
        let mut providers = std::collections::BTreeMap::<String, usize>::new();
        if format.is_some() {
            let profile = session.end_profiling().unwrap();
            let events: serde_json::Value =
                serde_json::from_slice(&std::fs::read(profile).unwrap()).unwrap();
            for event in events.as_array().unwrap() {
                if event["cat"] == "Node" {
                    if let Some(provider) = event["args"]["provider"].as_str() {
                        *providers.entry(provider.to_owned()).or_default() += 1;
                    }
                }
            }
            assert!(
                providers
                    .get("CoreMLExecutionProvider")
                    .copied()
                    .unwrap_or(0)
                    > 0
            );
        }
        let report = serde_json::json!({"name":name,"loadMs":load_ms,"runsMs":runs,"maskChanged":changed,"maskMaxError":maximum,"maskMae":mae,"providers":providers});
        println!("{report}");
        reports.push(report);
        std::fs::write(
            directory.join("isnet-comparison.json"),
            serde_json::to_vec_pretty(&reports).unwrap(),
        )
        .unwrap();
    }
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires Windows DX12 GPU and local verified model/runtime"]
fn windows_profile_contains_directml_nodes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime_path = PathBuf::from(std::env::var_os("S08_NATIVE_RUNTIME").unwrap());
    let spec = model::find("isnet-auto-high").unwrap();
    let model_path = PathBuf::from(std::env::var_os("S08_MODEL_DIR").unwrap())
        .join(format!("{}-{}.onnx", spec.id, spec.revision));
    // Production load also preloads the app-local DirectML dependency safely.
    let (production, _, _) = IsnetSession::load(&runtime_path, &model_path).unwrap();
    assert_eq!(
        production.backend(),
        GPU_BACKEND,
        "CPU fallback is not GPU validation"
    );
    drop(production);
    let directory = root.join("test-output/acceleration-windows");
    std::fs::create_dir_all(&directory).unwrap();
    let mut session = Session::builder()
        .unwrap()
        .with_memory_pattern(false)
        .unwrap()
        .with_parallel_execution(false)
        .unwrap()
        .with_execution_providers([
            ep::DirectML::default()
                .with_performance_preference(ep::directml::PerformancePreference::HighPerformance)
                .with_device_filter(ep::directml::DeviceFilter::Gpu)
                .build()
                .error_on_failure(),
            ep::CPU::default().build(),
        ])
        .unwrap()
        .with_profiling(directory.join("directml"))
        .unwrap()
        .commit_from_file(&model_path)
        .unwrap();
    let source =
        image_io::decode_image(&PathBuf::from(std::env::var_os("S08_PERF_IMAGE").unwrap()))
            .unwrap();
    let input = isnet::prepare(&source).unwrap();
    let tensor =
        TensorRef::from_array_view(([1_i64, 3, 1024, 1024], input.tensor.as_slice())).unwrap();
    session.run(ort::inputs![tensor]).unwrap();
    let profile = session.end_profiling().unwrap();
    let events: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&profile).unwrap()).unwrap();
    let mut providers = std::collections::BTreeMap::<String, usize>::new();
    for event in events.as_array().unwrap() {
        if event["cat"] == "Node" {
            if let Some(provider) = event["args"]["provider"].as_str() {
                *providers.entry(provider.to_owned()).or_default() += 1;
            }
        }
    }
    println!("profile: {profile}; providers: {providers:?}");
    assert!(
        providers.get("DmlExecutionProvider").copied().unwrap_or(0) > 0,
        "No DirectML graph nodes executed: {providers:?}"
    );
    std::fs::write(
        directory.join("providers.json"),
        serde_json::to_vec_pretty(&providers).unwrap(),
    )
    .unwrap();
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "run in a fresh process with a runtime directory lacking DirectML.dll"]
fn windows_missing_directml_falls_back_to_cpu() {
    let runtime_path = PathBuf::from(std::env::var_os("S08_NATIVE_RUNTIME").unwrap());
    assert!(!runtime_path.parent().unwrap().join("DirectML.dll").exists());
    let spec = model::find("isnet-auto-high").unwrap();
    let model_path = PathBuf::from(std::env::var_os("S08_MODEL_DIR").unwrap())
        .join(format!("{}-{}.onnx", spec.id, spec.revision));
    let (mut automatic, _, _) = IsnetSession::load(&runtime_path, &model_path).unwrap();
    assert_eq!(automatic.backend(), "cpu");
    let (mut reference, _, _) = IsnetSession::load_cpu(&runtime_path, &model_path).unwrap();
    let source =
        image_io::decode_image(&PathBuf::from(std::env::var_os("S08_PERF_IMAGE").unwrap()))
            .unwrap();
    let input = isnet::prepare(&source).unwrap();
    assert_eq!(
        automatic.run_prepared(&input).unwrap(),
        reference.run_prepared(&input).unwrap()
    );
    automatic.cancel_handle().cancel().unwrap();
    assert!(automatic.run_prepared(&input).is_err());
}
