use cutout_core::{image_io, sam, selection::SelectionSession};
use flate2::read::GzDecoder;
use serde_json::Value;
use sha2::{Digest, Sha256};
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

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
fn frozen_s08_sam_masks_match_their_manifest() {
    let root = fixtures().join("reference/model-s08");
    let reference: Value =
        serde_json::from_slice(&std::fs::read(root.join("sam-cross-size.json")).unwrap()).unwrap();
    assert_eq!(reference["backend"], "wasm");
    assert_eq!(reference["samples"].as_array().unwrap().len(), 2);
    for sample in reference["samples"].as_array().unwrap() {
        let name = sample["input"].as_str().unwrap();
        let width = sample["width"].as_u64().unwrap() as u32;
        let height = sample["height"].as_u64().unwrap() as u32;
        let points: Vec<sam::Point> = serde_json::from_value(sample["points"].clone()).unwrap();
        sam::prompt(&points, width, height).unwrap();
        let mask = gunzip(root.join(format!("{name}.mask.u8.gz")));
        assert_eq!(mask.len(), width as usize * height as usize);
        assert_eq!(sha256(&mask), sample["maskSha256"]);
    }
}

#[test]
fn native_sam_cross_size_matches_frozen_browser_canvas_when_models_are_available() {
    let Ok(runtime) = std::env::var("S04_NATIVE_RUNTIME") else {
        return;
    };
    let root = fixtures();
    let runtime = std::fs::canonicalize(root.join("../..").join(runtime)).unwrap();
    let models = root.join("../../test-output/models");
    let reference_root = root.join("reference/model-s08");
    let reference: Value =
        serde_json::from_slice(&std::fs::read(reference_root.join("sam-cross-size.json")).unwrap())
            .unwrap();
    for sample in reference["samples"].as_array().unwrap() {
        let name = sample["input"].as_str().unwrap();
        let width = sample["width"].as_u64().unwrap() as u32;
        let height = sample["height"].as_u64().unwrap() as u32;
        let fitted_width = sample["fittedWidth"].as_u64().unwrap() as u32;
        let fitted_height = sample["fittedHeight"].as_u64().unwrap() as u32;
        let image_path = root.join("images").join(name);
        let source = gunzip(
            root.join("reference")
                .join(format!("{name}.tensor.f32le.gz")),
        );
        assert_eq!(sha256(&source), sample["canvasTensorSha256"]);
        let chw: Vec<f32> = source
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect();
        let means = [0.485_f64, 0.456, 0.406];
        let mut input = Vec::with_capacity(fitted_width as usize * fitted_height as usize * 3);
        let plane = 1024_usize * 1024;
        for y in 0..fitted_height as usize {
            for x in 0..fitted_width as usize {
                for (channel, mean) in means.iter().enumerate() {
                    input.push(
                        ((f64::from(chw[channel * plane + y * 1024 + x]) + mean) * 255.0).round()
                            as f32,
                    );
                }
            }
        }
        let input_bytes: Vec<u8> = input.iter().flat_map(|value| value.to_le_bytes()).collect();
        assert_eq!(sha256(&input_bytes), sample["samInputSha256"]);
        let points: Vec<sam::Point> = serde_json::from_value(sample["points"].clone()).unwrap();
        let (mut prepared, _) = SelectionSession::load_prepared(
            &runtime,
            &models.join("mobilesam-image-encoder.onnx"),
            &models.join("mobilesam-mask-decoder.onnx"),
            (width, height),
            (fitted_width, fitted_height),
            &input,
        )
        .unwrap();
        let (prepared_mask, _) = prepared.select(&points, (width, height)).unwrap();
        let frozen = gunzip(reference_root.join(format!("{name}.mask.u8.gz")));
        assert_eq!(sha256(&frozen), sample["maskSha256"]);
        assert_eq!(
            prepared_mask, frozen,
            "{name}: native ORT changed on Canvas RGB"
        );

        let image = image_io::decode_image(&image_path).unwrap();
        let (mut actual, _) = SelectionSession::load(
            &runtime,
            &models.join("mobilesam-image-encoder.onnx"),
            &models.join("mobilesam-mask-decoder.onnx"),
            &image,
        )
        .unwrap();
        let (native_mask, _) = actual.select(&points, (width, height)).unwrap();
        let mismatches = native_mask
            .iter()
            .zip(&frozen)
            .filter(|(a, b)| a != b)
            .count();
        println!(
            "{name}: actual image vs frozen Canvas SAM mask {mismatches}/{} pixels differ",
            frozen.len()
        );
    }
}
