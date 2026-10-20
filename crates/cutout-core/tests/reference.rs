use cutout_core::{image_io, isnet, mask};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;

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

fn p99(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    values[(values.len() - 1) * 99 / 100]
}

#[test]
fn frozen_browser_samples_measure_preprocess_and_alpha() {
    let root = fixtures();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("reference/browser-canvas.json")).unwrap())
            .unwrap();
    for sample in manifest["samples"].as_array().unwrap() {
        let name = sample["input"].as_str().unwrap();
        let source = image_io::decode_image(&root.join("images").join(name)).unwrap();
        assert_eq!(
            source.width(),
            sample["width"].as_u64().unwrap() as u32,
            "{name}"
        );
        assert_eq!(
            source.height(),
            sample["height"].as_u64().unwrap() as u32,
            "{name}"
        );
        let prepared = isnet::prepare(&source).unwrap();
        assert_eq!(
            prepared.fitted_width,
            sample["fittedWidth"].as_u64().unwrap() as u32,
            "{name}"
        );
        assert_eq!(
            prepared.fitted_height,
            sample["fittedHeight"].as_u64().unwrap() as u32,
            "{name}"
        );
        let tensor = gunzip(
            root.join("reference")
                .join(format!("{name}.tensor.f32le.gz")),
        );
        assert_eq!(tensor.len(), prepared.tensor.len() * 4);
        let plane = 1024 * 1024;
        let mut errors = Vec::new();
        for channel in 0..3 {
            for y in 0..prepared.fitted_height as usize {
                for x in 0..prepared.fitted_width as usize {
                    let index = channel * plane + y * 1024 + x;
                    let reference =
                        f32::from_le_bytes(tensor[index * 4..index * 4 + 4].try_into().unwrap());
                    let error = f64::from((prepared.tensor[index] - reference).abs());
                    errors.push(error);
                }
            }
        }
        let mean = errors.iter().sum::<f64>() / errors.len() as f64;
        println!(
            "{name}: tensor MAE {mean:.6}, p99 {:.6}, max {:.6}",
            p99(errors.clone()),
            errors.iter().copied().fold(0.0, f64::max)
        );
        for channel in 0..3 {
            for y in 0..1024 {
                for x in 0..1024 {
                    if x >= prepared.fitted_width as usize || y >= prepared.fitted_height as usize {
                        assert_eq!(
                            prepared.tensor[channel * plane + y * 1024 + x],
                            0.0,
                            "{name}: padding"
                        );
                    }
                }
            }
        }
        let restored = gunzip(root.join("reference").join(format!("{name}.mask.u8.gz")));
        let cutout = mask::apply_model_mask(&source, &restored, 1024, 1024).unwrap();
        let expected = image::open(root.join("reference").join(format!("{name}.synthetic.png")))
            .unwrap()
            .to_rgba8();
        let alpha: Vec<f64> = cutout
            .pixels()
            .zip(expected.pixels())
            .map(|(a, b)| f64::from(a[3].abs_diff(b[3])))
            .collect();
        let mut opaque_rgb = Vec::new();
        let mut premul_rgb = Vec::new();
        for (actual, reference) in cutout.pixels().zip(expected.pixels()) {
            for channel in 0..3 {
                if actual[3] == 255 && reference[3] == 255 {
                    opaque_rgb.push(f64::from(actual[channel].abs_diff(reference[channel])));
                } else {
                    let a = f64::from(actual[channel]) * f64::from(actual[3]) / 255.0;
                    let b = f64::from(reference[channel]) * f64::from(reference[3]) / 255.0;
                    premul_rgb.push((a - b).abs());
                }
            }
        }
        println!(
            "{name}: alpha p99 {:.1}, max {:.1}, opaque RGB {} samples / p99 {:.1}, premul RGB p99 {:.1}",
            p99(alpha.clone()),
            alpha.iter().copied().fold(0.0, f64::max),
            opaque_rgb.len(),
            if opaque_rgb.is_empty() { 0.0 } else { p99(opaque_rgb.clone()) },
            p99(premul_rgb.clone())
        );
        if !opaque_rgb.is_empty() {
            assert!(p99(opaque_rgb.clone()) <= 1.0);
            assert!(opaque_rgb.iter().all(|value| *value <= 3.0));
        }
        assert!(p99(premul_rgb) <= 2.0);
        assert!(
            p99(alpha.clone()) <= 1.0 && alpha.iter().all(|value| *value <= 2.0),
            "{name}: synthetic alpha"
        );
        let model_mask = gunzip(
            root.join("reference/model-isnet")
                .join(format!("{name}.mask.u8.gz")),
        );
        let model_cutout = mask::apply_model_mask(&source, &model_mask, 1024, 1024).unwrap();
        let model_expected = image::open(
            root.join("reference/model-isnet")
                .join(format!("{name}.model.png")),
        )
        .unwrap()
        .to_rgba8();
        let model_alpha: Vec<f64> = model_cutout
            .pixels()
            .zip(model_expected.pixels())
            .map(|(a, b)| f64::from(a[3].abs_diff(b[3])))
            .collect();
        println!(
            "{name}: model alpha p99 {:.1}, max {:.1}",
            p99(model_alpha.clone()),
            model_alpha.iter().copied().fold(0.0, f64::max)
        );
    }
}
