//! Opt-in native regression checks; no model or user image is checked in.
use cutout_core::{image_io, inference::IsnetSession, isnet, mask, model, runtime};
use ort::{ep, session::Session, value::TensorRef};
use std::{path::PathBuf, time::Instant};

#[test]
#[ignore = "requires S08_NATIVE_RUNTIME, S08_MODEL_DIR and S08_PERF_IMAGE"]
fn prepared_mask_matches_uncached_native_output() {
    let runtime_path = PathBuf::from(std::env::var_os("S08_NATIVE_RUNTIME").unwrap());
    let model_root = PathBuf::from(std::env::var_os("S08_MODEL_DIR").unwrap());
    let spec = model::find("isnet-auto-high").unwrap();
    let model_path = model_root.join(format!("{}-{}.onnx", spec.id, spec.revision));
    runtime::initialize(&runtime_path).unwrap();
    let (mut optimized, _, _) = IsnetSession::load_cpu(&runtime_path, &model_path).unwrap();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/images");
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
        .map(|name| fixtures.join(name)),
    );
    // Separate, original all-output session: compare actual masks, not timings.
    let mut reference = Session::builder()
        .unwrap()
        .with_execution_providers([ep::CPU::default().build()])
        .unwrap()
        .commit_from_file(&model_path)
        .unwrap();
    for path in paths {
        let source = image_io::decode_image(&path).unwrap();
        let prepared = isnet::prepare(&source).unwrap();
        let tensor =
            TensorRef::from_array_view(([1_i64, 3, 1024, 1024], prepared.tensor.as_slice()))
                .unwrap();
        let output = reference.run(ort::inputs![tensor]).unwrap();
        let (_, values) = output["output_image"].try_extract_tensor::<f32>().unwrap();
        let normalized = mask::normalize(values).unwrap();
        let restored = isnet::restore_contain_mask(
            &normalized,
            1024,
            1024,
            prepared.fitted_width,
            prepared.fitted_height,
        )
        .unwrap();
        let expected =
            mask::resize_model_mask(&restored, 1024, 1024, source.width(), source.height())
                .unwrap();
        let first = optimized.infer_mask(&source).unwrap();
        let started = Instant::now();
        let cached = optimized.infer_mask(&source).unwrap();
        assert_eq!(first, expected, "uncached {}", path.display());
        assert_eq!(cached, expected, "cached {}", path.display());
        if path.as_os_str() == std::env::var_os("S08_PERF_IMAGE").unwrap() {
            if let Some(export_path) = std::env::var_os("S08_NATIVE_EXPORT") {
                let exported = image_io::decode_image(&PathBuf::from(export_path)).unwrap();
                let expected_rgba = mask::apply(&source, &expected).unwrap();
                assert_eq!(exported.dimensions(), expected_rgba.dimensions());
                let errors: Vec<u8> = exported
                    .as_raw()
                    .iter()
                    .zip(expected_rgba.as_raw())
                    .map(|(a, b)| a.abs_diff(*b))
                    .collect();
                let max = errors.iter().copied().max().unwrap();
                let changed = errors.iter().filter(|&&error| error != 0).count();
                assert!(max <= 1, "native export exceeds GPU rounding gate");
                assert!(exported
                    .pixels()
                    .zip(expected_rgba.pixels())
                    .all(|(actual, reference)| actual.0[..3] == reference.0[..3]));
                println!(
                    "native export: {}x{}, max {max}/255, {changed} changed alpha bytes",
                    exported.width(),
                    exported.height()
                );
            }
        }
        println!(
            "{}: pixel-identical, cache hit {} ms",
            path.file_name().unwrap().to_string_lossy(),
            started.elapsed().as_millis()
        );
    }
    optimized.cancel_handle().cancel().unwrap();
    let source = image_io::decode_image(&fixtures.join("exif-orient6.jpg")).unwrap();
    assert!(
        optimized.infer_mask(&source).is_err(),
        "cache must not bypass cancellation"
    );
}
