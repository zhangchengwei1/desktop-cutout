//! Bounded directory discovery. Images enter the existing Rust batch scheduler.
use std::path::{Path, PathBuf};

pub const OUTPUT_MARKER: &str = ".clearframe-output";
pub const OUTPUT_MARKER_CONTENT: &[u8] = b"ClearFrame Desktop context output v1\n";

pub struct FolderImages {
    pub paths: Vec<PathBuf>,
    pub skipped: usize,
}

pub fn scan(root: &Path, output: &Path) -> Result<FolderImages, String> {
    let root =
        std::fs::canonicalize(root).map_err(|error| format!("[folder_unavailable] {error}"))?;
    if !root.is_dir() {
        return Err("[folder_unavailable] 图片来源必须是文件夹".to_owned());
    }
    let excluded = std::fs::canonicalize(output)
        .ok()
        .filter(|path| *path != root && path.starts_with(&root));
    let mut pending = vec![root.clone()];
    let mut result = FolderImages {
        paths: Vec::new(),
        skipped: 0,
    };
    let mut visited = 0;
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if directory == root => return Err(format!("[folder_unavailable] {error}")),
            Err(_) => {
                result.skipped += 1;
                continue;
            }
        };
        for entry in entries {
            visited += 1;
            if visited > 100_000 {
                return Err("[folder_limit] 文件夹包含过多项目，请选择更小的子文件夹".to_owned());
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    result.skipped += 1;
                    continue;
                }
            };
            let path = entry.path();
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => {
                    result.skipped += 1;
                    continue;
                }
            };
            if kind.is_dir() {
                if excluded.as_ref() != Some(&path) && !is_generated_output(&path) {
                    pending.push(path);
                }
            } else if kind.is_file() && crate::image_io::supported_path(&path) {
                result.paths.push(path);
                if result.paths.len() > 10_000 {
                    return Err(
                        "[folder_limit] 文件夹包含超过 10000 张图片，请分批选择子文件夹".to_owned(),
                    );
                }
            } else {
                // Do not follow links, including links back into the source tree.
                result.skipped += 1;
            }
        }
    }
    result.paths.sort();
    if result.paths.is_empty() {
        return Err("[folder_empty] 文件夹中没有支持的图片文件".to_owned());
    }
    Ok(result)
}

fn is_generated_output(directory: &Path) -> bool {
    let marker = directory.join(OUTPUT_MARKER);
    std::fs::symlink_metadata(&marker).is_ok_and(|metadata| {
        metadata.is_file() && metadata.len() == OUTPUT_MARKER_CONTENT.len() as u64
    }) && std::fs::read(marker).is_ok_and(|content| content == OUTPUT_MARKER_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_nested_images_case_insensitively_and_excludes_output() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        let output = nested.join("results");
        std::fs::create_dir_all(&output).unwrap();
        for path in [
            directory.path().join("first.PNG"),
            nested.join("second.JpEg"),
            nested.join("third.webp"),
            output.join("already-saved.png"),
            nested.join("notes.txt"),
        ] {
            std::fs::write(path, b"test file").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(directory.path(), nested.join("cycle")).unwrap();
        let found = scan(directory.path(), &output).unwrap();
        assert_eq!(found.paths.len(), 3);
        assert!(found
            .paths
            .iter()
            .all(|path| !path.starts_with(output.as_path())));
        assert!(found.skipped >= 1);
    }

    #[test]
    fn empty_and_missing_folders_report_actionable_errors() {
        let directory = tempfile::tempdir().unwrap();
        assert!(scan(directory.path(), directory.path())
            .err()
            .unwrap()
            .starts_with("[folder_empty]"));
        assert!(scan(&directory.path().join("missing"), directory.path())
            .err()
            .unwrap()
            .starts_with("[folder_unavailable]"));
    }
}
