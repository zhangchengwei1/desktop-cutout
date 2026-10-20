use cutout_core::{image_io, matte, sam};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::{io::Read, path::PathBuf};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn gunzip(path: PathBuf) -> Vec<u8> {
    let mut bytes = Vec::new();
    GzDecoder::new(std::fs::File::open(path).unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

fn metrics(actual: &[f32], frozen: &[u8]) -> (f64, f64, f64) {
    assert_eq!(frozen.len(), actual.len() * 4);
    let mut errors: Vec<f64> = actual
        .iter()
        .zip(frozen.chunks_exact(4))
        .map(|(a, bytes)| {
            let b = f32::from_le_bytes(bytes.try_into().unwrap());
            f64::from((a - b).abs())
        })
        .collect();
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    errors.sort_by(f64::total_cmp);
    (
        mean,
        errors[(errors.len() - 1) * 99 / 100],
        *errors.last().unwrap(),
    )
}

#[test]
fn transparent_sam_inputs_measure_canvas_compatibility() {
    let root = fixtures();
    let side = 1024_usize;
    let plane = side * side;
    let means = [0.485_f64, 0.456, 0.406];
    for name in ["wide-alpha.png", "portrait-strands.png"] {
        let image = image_io::decode_image(&root.join("images").join(name)).unwrap();
        let (native, width, height) = sam::image_tensor(&image).unwrap();
        let tensor = gunzip(
            root.join("reference")
                .join(format!("{name}.tensor.f32le.gz")),
        );
        assert_eq!(tensor.len(), plane * 3 * 4);
        let source: Vec<f32> = tensor
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect();
        let mut canvas_rgb = Vec::with_capacity(native.len());
        for y in 0..height as usize {
            for x in 0..width as usize {
                for channel in 0..3 {
                    let normalized = source[channel * plane + y * side + x];
                    canvas_rgb
                        .push(((f64::from(normalized) + means[channel]) * 255.0).round() as f32);
                }
            }
        }
        let mut errors: Vec<f64> = native
            .iter()
            .zip(&canvas_rgb)
            .map(|(a, b)| f64::from((a - b).abs()))
            .collect();
        errors.sort_by(f64::total_cmp);
        let mae = errors.iter().sum::<f64>() / errors.len() as f64;
        println!(
            "{name} SAM Canvas input MAE {mae:.4}, P99 {:.4}, max {:.4} on raw [0,255] RGB",
            errors[(errors.len() - 1) * 99 / 100],
            errors.last().unwrap()
        );
        assert!(mae < 2.0, "transparent SAM input drifted from Canvas");
    }
}

#[test]
fn frozen_chromium_inputs_measure_s04_preprocessing() {
    let root = fixtures();
    let reference = root.join("reference/model-s04");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(reference.join("canvas.json")).unwrap()).unwrap();
    let image = image_io::decode_image(&root.join("images/hard-edge-opaque.png")).unwrap();
    let (sam_input, width, height) = sam::image_tensor(&image).unwrap();
    assert_eq!((width, height), (1024, 683));
    let sam_browser = gunzip(reference.join("sam-input.f32le.gz"));
    let sam_error = metrics(&sam_input, &sam_browser);
    println!(
        "SAM Canvas input MAE {:.4}, P99 {:.4}, max {:.4} on raw [0,255] RGB",
        sam_error.0, sam_error.1, sam_error.2
    );
    assert_eq!(
        manifest["sample"]["samShape"],
        serde_json::json!([683, 1024, 3])
    );

    let coarse = image_io::decode_gray_png(&root.join("images/mask-edges.png")).unwrap();
    let trimap = matte::trimap(
        coarse.as_raw(),
        None,
        coarse.dimensions(),
        image.dimensions(),
        12,
    )
    .unwrap();
    assert_eq!(
        trimap,
        gunzip(reference.join("trimap.u8.gz")),
        "trimap must match Chromium exactly"
    );
    let (matte_input, padded_width, padded_height) = matte::input(&image, &trimap).unwrap();
    assert_eq!((padded_width, padded_height), (96, 64));
    let matte_browser = gunzip(reference.join("matte-input.f32le.gz"));
    let matte_error = metrics(&matte_input, &matte_browser);
    println!(
        "ViTMatte Canvas input MAE {:.7}, P99 {:.7}, max {:.7}",
        matte_error.0, matte_error.1, matte_error.2
    );
    assert!(
        matte_error.2 <= 1e-6,
        "unscaled ViTMatte input drifted from Canvas"
    );
}

#[test]
fn native_cpu_s04_outputs_compare_with_frozen_wasm_when_models_are_installed() {
    let Ok(runtime) = std::env::var("S04_NATIVE_RUNTIME") else {
        return;
    };
    let root = fixtures();
    let runtime = std::fs::canonicalize(root.join("../..").join(runtime)).unwrap();
    let reference = root.join("reference/model-s04");
    let models = root.join("../../test-output/models");
    let temporary = tempfile::tempdir().unwrap();
    let input = root.join("images/hard-edge-opaque.png");
    let points: Vec<Vec<sam::Point>> =
        serde_json::from_slice(&std::fs::read(root.join("sam-points.json")).unwrap()).unwrap();
    let sam_path = temporary.path().join("sam.png");
    cutout_core::selection::select_file(
        &runtime,
        &models.join("mobilesam-image-encoder.onnx"),
        &models.join("mobilesam-mask-decoder.onnx"),
        &input,
        &sam_path,
        &points,
    )
    .unwrap();
    let native_sam = image::open(&sam_path).unwrap().to_luma8();
    let browser_sam = gunzip(reference.join("sam-mask.u8.gz"));
    let mismatches = native_sam
        .as_raw()
        .iter()
        .zip(&browser_sam)
        .filter(|(a, b)| a != b)
        .count();
    println!(
        "SAM final mask: {mismatches}/{} pixels differ from ORT Web on Canvas input",
        browser_sam.len()
    );

    let frozen = gunzip(reference.join("sam-input.f32le.gz"));
    let canvas_input: Vec<f32> = frozen
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    let (mut session, _) = cutout_core::selection::SelectionSession::load_prepared(
        &runtime,
        &models.join("mobilesam-image-encoder.onnx"),
        &models.join("mobilesam-mask-decoder.onnx"),
        (96, 64),
        (1024, 683),
        &canvas_input,
    )
    .unwrap();
    let mut canvas_union = vec![0; browser_sam.len()];
    for group in &points {
        let (mask, _) = session.select(group, (96, 64)).unwrap();
        canvas_union = sam::union(&canvas_union, &mask).unwrap();
    }
    let canvas_mismatches = canvas_union
        .iter()
        .zip(&browser_sam)
        .filter(|(a, b)| a != b)
        .count();
    println!(
        "SAM native ORT on frozen Canvas tensor: {canvas_mismatches}/{} pixels differ",
        browser_sam.len()
    );
    assert_eq!(
        canvas_mismatches, 0,
        "native SAM changed with identical Canvas input"
    );

    let matte_path = temporary.path().join("matte.png");
    cutout_core::refinement::refine_file(
        &runtime,
        &models.join("vitmatte-high.onnx"),
        &input,
        &root.join("images/mask-edges.png"),
        &matte_path,
        12,
    )
    .unwrap();
    let native_matte = image::open(&matte_path).unwrap().to_luma8();
    let browser_matte = gunzip(reference.join("matte-mask.u8.gz"));
    let errors: Vec<u8> = native_matte
        .as_raw()
        .iter()
        .zip(&browser_matte)
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let mae = errors.iter().map(|v| *v as f64).sum::<f64>() / errors.len() as f64;
    let max = errors.iter().copied().max().unwrap();
    println!("ViTMatte final alpha: MAE {mae:.4}/255, max {max}/255");
    assert_eq!(max, 0, "native ViTMatte changed on the frozen sample");
    assert_eq!(browser_sam.len(), native_sam.len());
    assert_eq!(browser_matte.len(), native_matte.len());
}
