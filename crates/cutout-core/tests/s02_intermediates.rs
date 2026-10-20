use cutout_core::{image_io, isnet, mask};
use flate2::read::GzDecoder;
use image::{imageops, GrayImage};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

fn read_gzip(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    GzDecoder::new(std::fs::File::open(path).unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

fn summarize(label: &str, actual: &[u8], expected: &[u8], channels: usize) {
    assert_eq!(actual.len(), expected.len(), "{label}: length");
    let mut errors: Vec<u8> = actual
        .chunks_exact(channels)
        .zip(expected.chunks_exact(channels))
        .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)))
        .collect();
    let sum: usize = errors.iter().map(|&value| value as usize).sum();
    errors.sort_unstable();
    let p99 = errors[(errors.len() - 1) * 99 / 100];
    let max = errors[errors.len() - 1];
    let different = errors.partition_point(|&value| value == 0);
    println!(
        "{label}: MAE {:.3}, P99 {p99}, max {max}, changed {}/{}",
        sum as f64 / errors.len() as f64,
        errors.len() - different,
        errors.len()
    );
}

fn summarize_rgba(label: &str, actual: &[u8], expected: &[u8]) {
    assert_eq!(actual.len(), expected.len(), "{label}: length");
    let mut rgb_actual = Vec::with_capacity(actual.len() / 4 * 3);
    let mut rgb_expected = Vec::with_capacity(expected.len() / 4 * 3);
    let mut alpha_actual = Vec::with_capacity(actual.len() / 4);
    let mut alpha_expected = Vec::with_capacity(expected.len() / 4);
    for (a, b) in actual.chunks_exact(4).zip(expected.chunks_exact(4)) {
        if a[3] != 0 || b[3] != 0 {
            rgb_actual.extend_from_slice(&a[..3]);
            rgb_expected.extend_from_slice(&b[..3]);
        }
        alpha_actual.push(a[3]);
        alpha_expected.push(b[3]);
    }
    summarize(
        &format!("{label} RGB (visible)"),
        &rgb_actual,
        &rgb_expected,
        1,
    );
    summarize(&format!("{label} alpha"), &alpha_actual, &alpha_expected, 1);
}

fn compare_filter_candidates(
    label: &str,
    input: &GrayImage,
    expected: &[u8],
    width: u32,
    height: u32,
) {
    for (name, filter) in [
        ("nearest", imageops::FilterType::Nearest),
        ("triangle", imageops::FilterType::Triangle),
        ("catmull-rom", imageops::FilterType::CatmullRom),
        ("gaussian", imageops::FilterType::Gaussian),
        ("lanczos3", imageops::FilterType::Lanczos3),
    ] {
        let candidate = imageops::resize(input, width, height, filter);
        summarize(&format!("{label} {name}"), candidate.as_raw(), expected, 1);
    }
}

fn bilinear_alpha_mae(
    source: &GrayImage,
    expected: &[u8],
    width: u32,
    height: u32,
    quantization: Option<(f64, bool)>,
) -> f64 {
    let (sw, sh) = source.dimensions();
    let pixels = source.as_raw();
    let mut total = 0_u64;
    let mut count = 0_u64;
    for y in (0..height).step_by(4) {
        let mut sy = (f64::from(y) + 0.5) * f64::from(sh) / f64::from(height) - 0.5;
        if let Some((steps, floor)) = quantization {
            sy = if floor {
                (sy * steps).floor()
            } else {
                (sy * steps).round()
            } / steps;
        }
        let y0 = sy.floor();
        let ty = sy - y0;
        let iy0 = y0.clamp(0.0, f64::from(sh - 1)) as usize;
        let iy1 = (y0 + 1.0).clamp(0.0, f64::from(sh - 1)) as usize;
        for x in (0..width).step_by(4) {
            let mut sx = (f64::from(x) + 0.5) * f64::from(sw) / f64::from(width) - 0.5;
            if let Some((steps, floor)) = quantization {
                sx = if floor {
                    (sx * steps).floor()
                } else {
                    (sx * steps).round()
                } / steps;
            }
            let x0 = sx.floor();
            let tx = sx - x0;
            let ix0 = x0.clamp(0.0, f64::from(sw - 1)) as usize;
            let ix1 = (x0 + 1.0).clamp(0.0, f64::from(sw - 1)) as usize;
            let top = f64::from(pixels[iy0 * sw as usize + ix0]) * (1.0 - tx)
                + f64::from(pixels[iy0 * sw as usize + ix1]) * tx;
            let bottom = f64::from(pixels[iy1 * sw as usize + ix0]) * (1.0 - tx)
                + f64::from(pixels[iy1 * sw as usize + ix1]) * tx;
            let actual = (top * (1.0 - ty) + bottom * ty).round() as u8;
            total += u64::from(actual.abs_diff(expected[(y * width + x) as usize]));
            count += 1;
        }
    }
    total as f64 / count as f64
}

/// Run after capture-s02-intermediates.sh. This is a measurement, not an
/// acceptance gate: the existing frozen quality thresholds remain unchanged.
#[test]
#[ignore = "requires locally captured Chromium intermediate pixels"]
fn compare_chromium_and_rust_stages() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    let captured = root.join("../../test-output/s02-canvas");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("reference/browser-canvas.json")).unwrap())
            .unwrap();
    for sample in manifest["samples"].as_array().unwrap() {
        let name = sample["input"].as_str().unwrap();
        let source = image_io::decode_image(&root.join("images").join(name)).unwrap();
        let (width, height) = source.dimensions();
        let fitted_width = sample["fittedWidth"].as_u64().unwrap() as u32;
        let fitted_height = sample["fittedHeight"].as_u64().unwrap() as u32;
        println!("\n{name} ({width}x{height} -> {fitted_width}x{fitted_height})");

        let browser_decoded = read_gzip(&captured.join(format!("{name}.decoded.rgba.gz")));
        summarize_rgba("decode", source.as_raw(), &browser_decoded);

        let resized = isnet::resize_premultiplied(&source, fitted_width, fitted_height);
        let browser_resized = read_gzip(&captured.join(format!("{name}.resized.rgba.gz")));
        summarize_rgba("input resize", resized.as_raw(), &browser_resized);
        let frozen_tensor = read_gzip(
            &root
                .join("reference")
                .join(format!("{name}.tensor.f32le.gz")),
        );
        let means = [0.485, 0.456, 0.406];
        let plane = 1024 * 1024;
        let mut browser_rgb = Vec::with_capacity(browser_resized.len() / 4 * 3);
        let mut frozen_rgb = Vec::with_capacity(browser_resized.len() / 4 * 3);
        for y in 0..fitted_height as usize {
            for x in 0..fitted_width as usize {
                let pixel = &browser_resized[(y * fitted_width as usize + x) * 4..][..4];
                for channel in 0..3 {
                    let index = channel * plane + y * 1024 + x;
                    let value = f32::from_le_bytes(
                        frozen_tensor[index * 4..index * 4 + 4].try_into().unwrap(),
                    );
                    browser_rgb.push(pixel[channel]);
                    frozen_rgb.push(((f64::from(value) + means[channel]) * 255.0).round() as u8);
                }
            }
        }
        summarize(
            "browser capture vs frozen browser RGB",
            &browser_rgb,
            &frozen_rgb,
            1,
        );
        if matches!(name, "wide-alpha.png" | "portrait-strands.png") {
            let source_alpha = GrayImage::from_raw(
                width,
                height,
                source.pixels().map(|pixel| pixel[3]).collect(),
            )
            .unwrap();
            let browser_alpha: Vec<u8> = browser_resized
                .chunks_exact(4)
                .map(|pixel| pixel[3])
                .collect();
            compare_filter_candidates(
                "input alpha candidate",
                &source_alpha,
                &browser_alpha,
                fitted_width,
                fitted_height,
            );
            println!(
                "input alpha sampled baseline MAE {:.3}",
                bilinear_alpha_mae(
                    &source_alpha,
                    &browser_alpha,
                    fitted_width,
                    fitted_height,
                    None,
                )
            );
            let mut quantized = Vec::new();
            for steps in [8.0, 16.0, 32.0, 64.0, 128.0, 256.0, 1024.0] {
                for floor in [false, true] {
                    quantized.push((
                        bilinear_alpha_mae(
                            &source_alpha,
                            &browser_alpha,
                            fitted_width,
                            fitted_height,
                            Some((steps, floor)),
                        ),
                        steps,
                        floor,
                    ));
                }
            }
            quantized.sort_by(|a, b| a.0.total_cmp(&b.0));
            println!(
                "input alpha quantization candidates top 5: {:?}",
                &quantized[..5]
            );
        }

        let restored = read_gzip(
            &root
                .join("reference/model-isnet")
                .join(format!("{name}.mask.u8.gz")),
        );
        let scaled = mask::resize_model_mask(&restored, 1024, 1024, width, height).unwrap();
        let browser_scaled = read_gzip(&captured.join(format!("{name}.scaled-mask.u8.gz")));
        summarize("model mask resize", scaled.as_raw(), &browser_scaled, 1);
        if source.pixels().all(|pixel| pixel[3] == 255) {
            let frozen_output = image::open(
                root.join("reference/model-isnet")
                    .join(format!("{name}.model.png")),
            )
            .unwrap()
            .to_rgba8();
            let frozen_alpha: Vec<u8> = frozen_output.pixels().map(|pixel| pixel[3]).collect();
            summarize(
                "browser mask capture vs frozen output alpha",
                &browser_scaled,
                &frozen_alpha,
                1,
            );
        }
        let original_mask = GrayImage::from_raw(1024, 1024, restored).unwrap();
        compare_filter_candidates(
            "mask candidate",
            &original_mask,
            &browser_scaled,
            width,
            height,
        );
    }
}
