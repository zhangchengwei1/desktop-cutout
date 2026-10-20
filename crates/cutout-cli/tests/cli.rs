use image::{GrayImage, Luma, Rgba, RgbaImage};
use std::process::Command;

#[test]
fn help_and_default_info_work_without_models() {
    let help = Command::new(env!("CARGO_BIN_EXE_cutout-cli"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("apply-mask"));
    let info = Command::new(env!("CARGO_BIN_EXE_cutout-cli"))
        .output()
        .unwrap();
    assert!(info.status.success());
    let json: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(json["coreVersion"], "0.1.0");
}

#[test]
fn invalid_options_are_rejected_before_accessing_files() {
    for args in [
        vec!["cutout", "runtime", "model", "image", "out", "--runs", "0"],
        vec!["cutout", "runtime", "model", "image", "out", "--runs", "21"],
        vec![
            "batch-bench",
            "runtime",
            "models",
            "plan",
            "out",
            "--zip",
            "--zip-during",
        ],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_cutout-cli"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(result.stdout.is_empty());
    }
}

// Exercise byte paths on Linux and Unicode filenames on macOS/Windows.
#[cfg(target_os = "linux")]
#[test]
fn image_paths_need_not_be_utf8() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join(OsString::from_vec(b"image-\xff.png".to_vec()));
    RgbaImage::from_pixel(2, 2, Rgba([20, 40, 60, 255]))
        .save(&path)
        .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_cutout-cli"))
        .arg("inspect")
        .arg(path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn unicode_and_option_like_image_names_are_literal_paths() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("--source 日本 image.png");
    RgbaImage::from_pixel(2, 2, Rgba([20, 40, 60, 255]))
        .save(&path)
        .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_cutout-cli"))
        .current_dir(directory.path())
        .args(["inspect", "--", "--source 日本 image.png"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn file_chain_preserves_source_alpha_and_never_overwrites() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.png");
    let mask_path = directory.path().join("mask.png");
    let result_path = directory.path().join("result.png");
    RgbaImage::from_pixel(1, 1, Rgba([20, 40, 60, 128]))
        .save(&source_path)
        .unwrap();
    GrayImage::from_pixel(1, 1, Luma([128]))
        .save(&mask_path)
        .unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_cutout-cli"))
        .args(["apply-mask"])
        .arg(&source_path)
        .arg(&mask_path)
        .arg(&result_path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        image::open(&result_path)
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)
            .0,
        [20, 40, 60, 64]
    );

    let second = Command::new(env!("CARGO_BIN_EXE_cutout-cli"))
        .args(["apply-mask"])
        .arg(&source_path)
        .arg(&mask_path)
        .arg(&result_path)
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert_eq!(
        image::open(&result_path)
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)[3],
        64
    );
}

#[test]
fn cutout_rejects_invalid_model_before_runtime_and_does_not_publish_output() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.png");
    let result_path = directory.path().join("result.png");
    RgbaImage::from_pixel(2, 2, Rgba([20, 40, 60, 255]))
        .save(&source_path)
        .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_cutout-cli"))
        .arg("cutout")
        .arg(directory.path().join("missing-onnxruntime.dylib"))
        .arg(&source_path)
        .arg(&source_path)
        .arg(&result_path)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("模型字节数不匹配"));
    assert!(!result_path.exists());
}

#[test]
fn s04_commands_reject_missing_models_without_publishing_masks() {
    let directory = tempfile::tempdir().unwrap();
    let image_path = directory.path().join("image.png");
    let mask_path = directory.path().join("coarse.png");
    let output_path = directory.path().join("out.png");
    let points_path = directory.path().join("points.json");
    RgbaImage::from_pixel(2, 2, Rgba([20, 40, 60, 255]))
        .save(&image_path)
        .unwrap();
    GrayImage::from_pixel(2, 2, Luma([128]))
        .save(&mask_path)
        .unwrap();
    std::fs::write(&points_path, b"[[{\"x\":0.5,\"y\":0.5,\"label\":1}]]").unwrap();
    let missing = directory.path().join("missing.onnx");
    let runtime = directory.path().join("missing.dylib");
    let select = Command::new(env!("CARGO_BIN_EXE_cutout-cli"))
        .arg("select")
        .arg(&runtime)
        .arg(&missing)
        .arg(&missing)
        .arg(&image_path)
        .arg(&output_path)
        .arg(&points_path)
        .output()
        .unwrap();
    assert!(!select.status.success());
    assert!(!output_path.exists());
    let refine = Command::new(env!("CARGO_BIN_EXE_cutout-cli"))
        .arg("refine")
        .arg(&runtime)
        .arg(&missing)
        .arg(&image_path)
        .arg(&mask_path)
        .arg(&output_path)
        .output()
        .unwrap();
    assert!(!refine.status.success());
    assert!(!output_path.exists());
}
