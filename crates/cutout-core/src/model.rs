use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpec {
    pub id: &'static str,
    pub revision: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub download_url: &'static str,
    pub license: &'static str,
    pub license_url: &'static str,
    pub tensor_contract: &'static str,
    pub verified_backend: &'static str,
}

/// Pinned high-quality weights from browser-cutout/public/models/model-manifest.json.
/// The low-quality U²-Netp and INT8 ViTMatte entries are intentionally omitted.
pub const MODELS: [ModelSpec; 4] = [
    ModelSpec {
        id: "isnet-auto-high",
        revision: "9783722d9f964c0286a411e7e8e6fede947d5a53",
        bytes: 178_648_008,
        sha256: "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a",
        download_url: "https://huggingface.co/BritishWerewolf/IS-Net/resolve/9783722d9f964c0286a411e7e8e6fede947d5a53/onnx/model.onnx",
        license: "Apache-2.0",
        license_url: "https://huggingface.co/BritishWerewolf/IS-Net/blob/9783722d9f964c0286a411e7e8e6fede947d5a53/README.md",
        tensor_contract: "input_image FP32 [1,3,1024,1024] -> output_image FP32 [1,1,1024,1024]",
        verified_backend: "macOS arm64 / ONNX Runtime 1.28.0 CPU + CoreML MLProgram CPU/GPU FP32",
    },
    ModelSpec {
        id: "mobilesam-image-encoder",
        revision: "0d3b403339b4674a82493d5e97964dd78089ddc8",
        bytes: 28_157_093,
        sha256: "580f5fb648ea1062c0aabc26217aed56921985f03f0cbbd852bba81d760cc749",
        download_url: "https://huggingface.co/Acly/MobileSAM/resolve/0d3b403339b4674a82493d5e97964dd78089ddc8/mobile_sam_image_encoder.onnx",
        license: "MIT (ONNX export); Apache-2.0 (MobileSAM/SAM)",
        license_url: "https://huggingface.co/Acly/MobileSAM/blob/0d3b403339b4674a82493d5e97964dd78089ddc8/README.md",
        tensor_contract: "input_image FP32 [height,width,3] -> image_embeddings FP32 [1,256,64,64]",
        verified_backend: "macOS arm64 / ONNX Runtime 1.28.0 CPU",
    },
    ModelSpec {
        id: "mobilesam-mask-decoder",
        revision: "0d3b403339b4674a82493d5e97964dd78089ddc8",
        bytes: 16_496_559,
        sha256: "8976b90a87ba50a6a72217a5ff994f7d25ce16f2229fcc1ed259e1294c622ffe",
        download_url: "https://huggingface.co/Acly/MobileSAM/resolve/0d3b403339b4674a82493d5e97964dd78089ddc8/sam_mask_decoder_multi.onnx",
        license: "MIT (ONNX export); Apache-2.0 (MobileSAM/SAM)",
        license_url: "https://huggingface.co/Acly/MobileSAM/blob/0d3b403339b4674a82493d5e97964dd78089ddc8/README.md",
        tensor_contract: "image_embeddings FP32 [1,256,64,64] + point prompts -> masks FP32 [1,4,H,W]",
        verified_backend: "macOS arm64 / ONNX Runtime 1.28.0 CPU",
    },
    ModelSpec {
        id: "vitmatte-high",
        revision: "358d428c452e5e0cd52955011a8b51944731d28e",
        bytes: 103_885_865,
        sha256: "a1cf48234c369faa3ea1711981d961fe1ec71f51e593f9d6553aa5a0e7d557e3",
        download_url: "https://huggingface.co/Xenova/vitmatte-small-distinctions-646/resolve/358d428c452e5e0cd52955011a8b51944731d28e/onnx/model.onnx",
        license: "Apache-2.0",
        license_url: "https://huggingface.co/hustvl/vitmatte-small-distinctions-646",
        tensor_contract: "pixel_values FP32 [1,4,H,W] -> alphas FP32 [1,1,H,W] (H/W padded to 32)",
        verified_backend: "macOS arm64 / ONNX Runtime 1.28.0 CPU",
    },
];

pub fn find(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|spec| spec.id == id)
}

#[derive(Debug)]
pub enum VerifyError {
    Io(io::Error),
    WrongSize { expected: u64, actual: u64 },
    WrongHash,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "读取模型失败: {error}"),
            Self::WrongSize { expected, actual } => {
                write!(f, "模型字节数不匹配: 预期 {expected}, 实际 {actual}")
            }
            Self::WrongHash => write!(f, "模型 SHA-256 不匹配"),
        }
    }
}

impl std::error::Error for VerifyError {}

impl From<io::Error> for VerifyError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Stream the file through SHA-256. Size is checked before and after reading,
/// so a partially downloaded or concurrently changed file cannot be accepted.
pub fn verify_file(path: &Path, spec: &ModelSpec) -> Result<(), VerifyError> {
    let mut file = File::open(path)?;
    let actual = file.metadata()?.len();
    if actual != spec.bytes {
        return Err(VerifyError::WrongSize {
            expected: spec.bytes,
            actual,
        });
    }
    let mut hasher = Sha256::new();
    let mut bytes_read = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes_read += count as u64;
        hasher.update(&buffer[..count]);
    }
    if bytes_read != spec.bytes {
        return Err(VerifyError::WrongSize {
            expected: spec.bytes,
            actual: bytes_read,
        });
    }
    let actual_hash = format!("{:x}", hasher.finalize());
    if actual_hash != spec.sha256 {
        return Err(VerifyError::WrongHash);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, Write};

    #[test]
    fn streamed_verification_rejects_partial_and_corrupt_files() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"abc").unwrap();
        file.flush().unwrap();
        let spec = ModelSpec {
            id: "test",
            revision: "test",
            bytes: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            download_url: "https://example.invalid/test.onnx",
            license: "test",
            license_url: "https://example.invalid/license",
            tensor_contract: "test",
            verified_backend: "test",
        };
        verify_file(file.path(), &spec).unwrap();
        assert!(matches!(
            verify_file(file.path(), &ModelSpec { bytes: 4, ..spec }),
            Err(VerifyError::WrongSize { .. })
        ));
        file.as_file_mut().set_len(0).unwrap();
        file.as_file_mut().rewind().unwrap();
        file.write_all(b"xyz").unwrap();
        file.flush().unwrap();
        assert!(matches!(
            verify_file(file.path(), &spec),
            Err(VerifyError::WrongHash)
        ));
    }

    #[test]
    fn quality_manifest_contains_only_selected_models() {
        assert_eq!(MODELS.len(), 4);
        assert!(find("u2netp-auto-low").is_none());
        assert!(find("vitmatte-standard").is_none());
    }
}
