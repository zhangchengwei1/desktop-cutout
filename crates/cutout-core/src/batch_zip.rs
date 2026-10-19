//! Bounded ZIP import and streaming export for native batch jobs.

use crate::session::SavedBatchOutput;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const MAX_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

pub(crate) struct ImportedArchive {
    pub directory: TempDir,
    pub sources: Vec<ImportedImage>,
}

pub(crate) struct ImportedImage {
    pub path: PathBuf,
    pub display_name: String,
    pub entry_index: usize,
    pub sha256: String,
}

pub(crate) fn import_images(path: &Path) -> Result<ImportedArchive, String> {
    let source = File::open(path).map_err(|error| error.to_string())?;
    if source.metadata().map_err(|error| error.to_string())?.len() > MAX_ARCHIVE_BYTES {
        return Err("ZIP 文件超过 4 GiB 上限".to_owned());
    }
    let mut archive = ZipArchive::new(source).map_err(|error| error.to_string())?;
    if archive.len() > MAX_ENTRIES {
        return Err("ZIP 条目超过 10000 项上限".to_owned());
    }
    if archive
        .has_overlapping_files()
        .map_err(|error| error.to_string())?
    {
        return Err("ZIP 存在重叠条目".to_owned());
    }
    let directory = tempfile::Builder::new()
        .prefix("clearframe-batch-zip-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let mut sources = Vec::new();
    let mut expanded = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let (display_name, extension) = checked_image_name(entry.name(), entry.is_dir())?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("ZIP 不允许符号链接".to_owned());
        }
        let (Some(display_name), Some(extension)) = (display_name, extension) else {
            continue;
        };
        if entry.size() > MAX_IMAGE_BYTES {
            return Err(format!("ZIP 图片超过 256 MiB 上限：{display_name}"));
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or("ZIP 解压量超出上限")?;
        if expanded > MAX_EXPANDED_BYTES {
            return Err("ZIP 解压量超过 4 GiB 上限".to_owned());
        }
        let extracted = directory.path().join(format!("{index:06}.{extension}"));
        let mut output = File::create_new(&extracted).map_err(|error| error.to_string())?;
        let mut reader = entry.by_ref().take(MAX_IMAGE_BYTES + 1);
        let mut hasher = Sha256::new();
        let mut actual = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = reader
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            actual += read as u64;
            if actual > MAX_IMAGE_BYTES {
                return Err(format!("ZIP 图片超过 256 MiB 上限：{display_name}"));
            }
            output
                .write_all(&buffer[..read])
                .map_err(|error| error.to_string())?;
            hasher.update(&buffer[..read]);
        }
        if actual != entry.size() || actual > MAX_IMAGE_BYTES {
            return Err(format!("ZIP 图片大小不符：{display_name}"));
        }
        crate::image_io::detected_format(&extracted).map_err(|error| error.to_string())?;
        sources.push(ImportedImage {
            path: extracted,
            display_name,
            entry_index: index,
            sha256: format!("{:x}", hasher.finalize()),
        });
    }
    if sources.is_empty() {
        return Err("ZIP 中没有可处理的 PNG、JPEG 或 WebP 图片".to_owned());
    }
    Ok(ImportedArchive { directory, sources })
}

fn checked_image_name(
    name: &str,
    directory: bool,
) -> Result<(Option<String>, Option<&'static str>), String> {
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains('\0') {
        return Err("ZIP 路径无效".to_owned());
    }
    let segments = normalized
        .trim_end_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if segments.is_empty()
        || segments
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == ".." || part.contains(':'))
    {
        return Err("ZIP 包含越界或无效路径".to_owned());
    }
    if directory
        || segments
            .iter()
            .any(|part| part.starts_with('.') || *part == "__MACOSX")
    {
        return Ok((None, None));
    }
    let basename = *segments.last().unwrap();
    let extension = Path::new(basename)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let Some(extension) = extension
        .as_deref()
        .and_then(crate::image_io::supported_extension)
    else {
        return Ok((None, None));
    };
    Ok((Some(basename.to_owned()), Some(extension)))
}

#[cfg(test)]
pub(crate) fn export_results(
    destination: &Path,
    ordered: &[(String, PathBuf)],
) -> Result<usize, String> {
    export_results_streaming(destination, ordered.len(), |index| {
        let (name, path) = &ordered[index];
        Ok(Some((name.clone(), path.clone(), None)))
    })
}

pub(crate) fn export_results_streaming(
    destination: &Path,
    items: usize,
    mut next: impl FnMut(usize) -> Result<Option<(String, PathBuf, Option<SavedBatchOutput>)>, String>,
) -> Result<usize, String> {
    if items == 0 {
        return Err("此批次还没有可打包的 PNG".to_owned());
    }
    if destination
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case("zip"))
    {
        return Err("请选择 ZIP 保存路径".to_owned());
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    let mut included = 0;
    let mut source_bytes = 0_u64;
    {
        let mut writer = ZipWriter::new(temporary.as_file_mut());
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for index in 0..items {
            let Some((name, path, expected)) = next(index)? else {
                continue;
            };
            if name.contains(['/', '\\']) || name.is_empty() {
                return Err("PNG 文件名无法安全写入 ZIP".to_owned());
            }
            if !std::fs::symlink_metadata(&path)
                .map_err(|error| error.to_string())?
                .file_type()
                .is_file()
            {
                return Err(format!("{name}: PNG 文件类型已变化"));
            }
            let source = File::open(path).map_err(|error| format!("{name}: {error}"))?;
            let size = source.metadata().map_err(|error| error.to_string())?.len();
            if expected.as_ref().is_some_and(|digest| digest.bytes != size) {
                return Err(format!("{name}: PNG 内容已变化"));
            }
            source_bytes = source_bytes.checked_add(size).ok_or("ZIP 大小超出上限")?;
            // Reserve ample room for entry headers and the central directory.
            if source_bytes > MAX_ARCHIVE_BYTES - 16 * 1024 * 1024 || included >= MAX_ENTRIES {
                return Err("ZIP 超过 4 GiB 或 10000 项上限".to_owned());
            }
            writer
                .start_file(&name, options)
                .map_err(|error| error.to_string())?;
            let mut source = source.take(size + 1);
            let mut hasher = expected.as_ref().map(|_| Sha256::new());
            let mut copied = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = source
                    .read(&mut buffer)
                    .map_err(|error| format!("{name}: {error}"))?;
                if read == 0 {
                    break;
                }
                writer
                    .write_all(&buffer[..read])
                    .map_err(|error| format!("{name}: {error}"))?;
                if let Some(hasher) = &mut hasher {
                    hasher.update(&buffer[..read]);
                }
                copied += read as u64;
            }
            if copied != size {
                return Err(format!("{name}: PNG 在打包时发生变化"));
            }
            if let (Some(digest), Some(hasher)) = (expected, hasher) {
                if format!("{:x}", hasher.finalize()) != digest.sha256 {
                    return Err(format!("{name}: PNG 内容已变化"));
                }
            }
            included += 1;
        }
        if included == 0 {
            return Err("此批次还没有可打包的 PNG".to_owned());
        }
        writer.finish().map_err(|error| error.to_string())?;
    }
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary
        .persist_noclobber(destination)
        .map_err(|error| error.to_string())?;
    Ok(included)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{self, Write};

    #[test]
    fn zip_paths_reject_traversal_and_hidden_files() {
        for extension in [
            "GIF", "bmp", "tiff", "ico", "tga", "dds", "ppm", "qoi", "ff",
        ] {
            assert!(
                checked_image_name(&format!("folder/photo.{extension}"), false)
                    .unwrap()
                    .1
                    .is_some()
            );
        }
        assert!(checked_image_name("../escape.png", false).is_err());
        assert!(checked_image_name("C:\\escape.png", false).is_err());
        assert!(checked_image_name("/escape.png", false).is_err());
        assert!(checked_image_name("folder/.hidden.png", false)
            .unwrap()
            .0
            .is_none());
    }

    #[test]
    fn malicious_archive_is_rejected_without_writing_outside_the_temporary_directory() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("traversal.zip");
        let mut writer = ZipWriter::new(File::create(&input).unwrap());
        writer
            .start_file("../escape.png", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"bad image").unwrap();
        writer.finish().unwrap();
        assert!(import_images(&input).is_err());
        assert!(!root.path().join("escape.png").exists());
    }

    #[test]
    fn zip_import_and_export_stream_files_in_order_without_overwriting() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("input.zip");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/images/hard-edge-opaque.png");
        {
            let mut writer = ZipWriter::new(File::create(&input).unwrap());
            writer
                .start_file("nested/one.png", SimpleFileOptions::default())
                .unwrap();
            io::copy(&mut File::open(&fixture).unwrap(), &mut writer).unwrap();
            writer
                .start_file("nested/two.png", SimpleFileOptions::default())
                .unwrap();
            io::copy(&mut File::open(&fixture).unwrap(), &mut writer).unwrap();
            writer.finish().unwrap();
        }
        let imported = import_images(&input).unwrap();
        assert_eq!(imported.sources.len(), 2);
        assert_eq!(imported.sources[0].display_name, "one.png");
        let destination = root.path().join("result.zip");
        let ordered = imported
            .sources
            .iter()
            .enumerate()
            .map(|(index, image)| (format!("{index}-cutout.png"), image.path.clone()))
            .collect::<Vec<_>>();
        assert_eq!(export_results(&destination, &ordered).unwrap(), 2);
        assert!(export_results(&destination, &ordered).is_err());
        let incomplete = root.path().join("incomplete.zip");
        let missing = vec![
            ("one.png".to_owned(), fixture.clone()),
            ("missing.png".to_owned(), root.path().join("missing.png")),
        ];
        assert!(export_results(&incomplete, &missing).is_err());
        assert!(!incomplete.exists());
        let mut result = ZipArchive::new(File::open(&destination).unwrap()).unwrap();
        assert_eq!(result.by_index(0).unwrap().name(), "0-cutout.png");
        assert_eq!(result.by_index(1).unwrap().name(), "1-cutout.png");
        let mut bytes = Vec::new();
        result.by_index(1).unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, fs::read(&fixture).unwrap());
    }

    #[test]
    fn export_rejects_oversized_source_without_publishing_an_archive() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("sparse.png");
        File::create(&source)
            .unwrap()
            .set_len(MAX_ARCHIVE_BYTES)
            .unwrap();
        let destination = root.path().join("oversized.zip");
        let ordered = [("sparse.png".to_owned(), source)];
        assert!(export_results(&destination, &ordered).is_err());
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
