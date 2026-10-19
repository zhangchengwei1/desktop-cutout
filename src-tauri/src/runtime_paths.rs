use std::path::{Path, PathBuf};

pub(crate) fn runtime_library_path(
    resource_dir: &Path,
    override_path: Option<PathBuf>,
    development_path: Option<&Path>,
) -> PathBuf {
    let library = if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    if let Some(path) = override_path {
        return path;
    }
    // macOS dylibs are bundled and signed in Contents/Frameworks.
    let packaged = if cfg!(target_os = "macos") {
        resource_dir.join("../Frameworks").join(library)
    } else if cfg!(target_os = "windows") {
        // App-local MSVC dependencies must live beside the executable as well.
        resource_dir.join(library)
    } else {
        resource_dir.join("runtime").join(library)
    };
    if packaged.is_file() {
        return packaged;
    }
    if let Some(development) = development_path {
        if development.is_file() {
            return development.to_path_buf();
        }
    }
    packaged
}

#[cfg(test)]
mod runtime_library_tests {
    use super::*;

    #[test]
    fn explicit_path_takes_priority() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let fallback = root.join("Cargo.toml");
        let explicit = root.join("custom-runtime.dylib");
        assert_eq!(
            runtime_library_path(root, Some(explicit.clone()), Some(&fallback)),
            explicit
        );
    }

    #[test]
    fn uses_existing_development_library_when_bundle_is_missing() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let fallback = root.join("Cargo.toml");
        assert_eq!(runtime_library_path(root, None, Some(&fallback)), fallback);
    }

    #[test]
    fn installed_library_takes_priority_over_development_fallback() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("clearframe-bundle-{}-{unique}", std::process::id()));
        let resources = root.join("Resources");
        std::fs::create_dir_all(&resources).unwrap();
        let packaged = runtime_library_path(&resources, None, None);
        std::fs::create_dir_all(packaged.parent().unwrap()).unwrap();
        std::fs::write(&packaged, b"bundled runtime").unwrap();
        let development = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert_eq!(
            runtime_library_path(&resources, None, Some(&development)),
            packaged
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
