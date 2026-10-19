//! Shared native-runtime boundary, including actionable diagnostics for failed loads.
use std::{io, path::Path};

pub const UNAVAILABLE: &str = "[runtime_unavailable]";

pub fn initialize(path: &Path) -> io::Result<()> {
    let absolute = std::fs::canonicalize(path)
        .map_err(|error| io::Error::other(format!("{UNAVAILABLE} {}: {error}", path.display())))?;
    // Resolve app-local CRT/provider dependencies beside ORT, including in dev
    // builds where the runtime directory differs from the executable directory.
    #[cfg(target_os = "windows")]
    let _local_runtime = windows_library(&absolute)
        .map_err(|error| io::Error::other(format!("{UNAVAILABLE} {error}")))?;
    let environment = ort::init_from(&absolute).map_err(|error| {
        // libloading's Display omits dlerror; Debug retains the system reason
        // (including Team ID/signature, architecture and dependency failures).
        io::Error::other(format!("{UNAVAILABLE} {error}; {error:?}"))
    })?;
    environment.with_telemetry(false).commit();
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_library(path: &Path) -> io::Result<libloading::os::windows::Library> {
    use libloading::os::windows::{
        Library, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    // Only an absolute, caller-owned runtime path and system dependencies enter
    // this loader. Do not change the process-wide DLL search path or use CWD.
    unsafe {
        Library::load_with_flags(
            path,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    }
    .map_err(|error| io::Error::other(format!("{}: {error}; {error:?}", path.display())))
}

#[cfg(target_os = "windows")]
pub(crate) fn load_directml(runtime: &Path) -> io::Result<()> {
    use std::sync::{Mutex, OnceLock};
    static DIRECTML: OnceLock<Mutex<Option<libloading::os::windows::Library>>> = OnceLock::new();
    let mut library = DIRECTML
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| io::Error::other("DirectML library lock poisoned"))?;
    if library.is_none() {
        let runtime = std::fs::canonicalize(runtime)?;
        let path = runtime
            .parent()
            .ok_or_else(|| io::Error::other("Missing runtime directory"))?
            .join("DirectML.dll");
        // Retain the DLL for all sessions; unloading it while ORT still owns GPU
        // resources is unsafe. Missing/broken DirectML leaves CPU usable.
        *library = Some(windows_library(&path)?);
    }
    Ok(())
}

pub fn is_unavailable(error: &str) -> bool {
    error.starts_with(UNAVAILABLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_library_reports_recoverable_runtime_error_with_path() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing-runtime.dylib");
        let error = initialize(&missing).unwrap_err().to_string();
        assert!(is_unavailable(&error));
        assert!(error.contains("missing-runtime.dylib"));
    }

    #[test]
    fn ordinary_image_errors_are_not_runtime_failures() {
        assert!(!is_unavailable("图片格式无效"));
    }
}
