//! Headless shell entry point shared by the installed app and standalone CLI.
use crate::{
    batch_folder,
    jobs::Workspace,
    model_store::{ModelState, ModelStore},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

const APP_ID: &str = "dev.silta.clearframe.desktop";
const MAX_IMAGES: usize = 10_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Single,
    Batch,
}

#[derive(Debug)]
struct Options {
    mode: Mode,
    paths: Vec<PathBuf>,
    runtime: Option<PathBuf>,
    models: Option<PathBuf>,
}

fn parse(args: Vec<OsString>) -> Result<Options, String> {
    let mode = match args.first().and_then(|value| value.to_str()) {
        Some("single") => Mode::Single,
        Some("batch") => Mode::Batch,
        _ => return Err("Usage: context-cutout <single|batch> [--runtime <library>] [--models <cache>] -- <images or folders>".into()),
    };
    let mut result = Options {
        mode,
        paths: vec![],
        runtime: None,
        models: None,
    };
    let mut index = 1;
    while index < args.len() {
        match args[index].to_str() {
            Some("--") => {
                result
                    .paths
                    .extend(args[index + 1..].iter().map(PathBuf::from));
                break;
            }
            Some("--runtime" | "--models") if index + 1 < args.len() => {
                let destination = if args[index] == "--runtime" {
                    &mut result.runtime
                } else {
                    &mut result.models
                };
                if destination.is_some() {
                    return Err("Duplicate CLI option".into());
                }
                *destination = Some(PathBuf::from(&args[index + 1]));
            }
            Some("--selection-file") if index + 1 < args.len() => {
                // Only consume a manifest created by our Windows shell adapter.
                let path = fs::canonicalize(PathBuf::from(&args[index + 1]))
                    .map_err(|error| error.to_string())?;
                let temporary =
                    fs::canonicalize(std::env::temp_dir()).map_err(|error| error.to_string())?;
                if path.parent() != Some(temporary.as_path())
                    || !path
                        .file_name()
                        .and_then(|v| v.to_str())
                        .is_some_and(|name| {
                            name.starts_with("clearframe-selection-") && name.ends_with(".json")
                        })
                {
                    return Err("Invalid shell selection manifest location".into());
                }
                let owned =
                    tempfile::TempPath::try_from_path(path).map_err(|error| error.to_string())?;
                if fs::metadata(&owned)
                    .map_err(|error| error.to_string())?
                    .len()
                    > 2 * 1024 * 1024
                {
                    return Err("Shell selection is too large".into());
                }
                let paths: Vec<PathBuf> =
                    serde_json::from_slice(&fs::read(&owned).map_err(|error| error.to_string())?)
                        .map_err(|error| error.to_string())?;
                result.paths.extend(paths);
            }
            _ => return Err("Unknown context-cutout option; put selected paths after --".into()),
        }
        index += 2;
    }
    if result.paths.is_empty() || result.paths.len() > MAX_IMAGES {
        return Err("Select 1–10000 images, or a containing folder".into());
    }
    if result.mode == Mode::Single && result.paths.len() != 1 {
        return Err("Single-image removal requires exactly one image".into());
    }
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub mode: Mode,
    pub paths: Vec<PathBuf>,
}

pub fn parse_selection(args: Vec<OsString>) -> Result<Selection, String> {
    let options = parse(args)?;
    if options.runtime.is_some() || options.models.is_some() {
        return Err("Shell requests cannot change the runtime or model cache".into());
    }
    Ok(Selection {
        mode: options.mode,
        paths: options.paths,
    })
}

pub fn plan_selection(
    selection: &Selection,
) -> Result<BTreeMap<PathBuf, BTreeSet<PathBuf>>, String> {
    let groups = plan(&selection.mode, &selection.paths)?;
    if groups.is_empty() {
        return Err("No supported images found in the selected folders".into());
    }
    Ok(groups)
}

pub fn installed_runtime(executable: &Path) -> PathBuf {
    let directory = executable.parent().unwrap_or(Path::new("."));
    if cfg!(target_os = "macos") {
        directory.join("../Frameworks/libonnxruntime.dylib")
    } else {
        directory.join("onnxruntime.dll")
    }
}

pub fn application_data() -> Result<PathBuf, String> {
    dirs::data_local_dir()
        .map(|directory| directory.join(APP_ID))
        .ok_or("Cannot locate application data directory".into())
}

fn plan(mode: &Mode, selected: &[PathBuf]) -> Result<BTreeMap<PathBuf, BTreeSet<PathBuf>>, String> {
    let mut groups: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for path in selected {
        let path =
            fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let (directory, inputs) = if path.is_dir() && *mode == Mode::Batch {
            let inputs = batch_folder::scan(&path, &path)?.paths;
            (path, inputs)
        } else if path.is_file() && supported(&path) {
            (
                path.parent()
                    .ok_or("Image has no containing directory")?
                    .to_path_buf(),
                vec![path],
            )
        } else {
            return Err("Choose PNG, JPEG or WebP images; folders require batch removal".into());
        };
        for input in inputs {
            if seen.insert(input.clone()) {
                groups.entry(directory.clone()).or_default().insert(input);
            }
            if seen.len() > MAX_IMAGES {
                return Err("Import at most 10000 images per operation".into());
            }
        }
    }
    Ok(groups)
}

fn supported(path: &Path) -> bool {
    crate::image_io::supported_path(path)
}

pub fn output_directory(parent: &Path) -> Result<PathBuf, String> {
    for number in 1..=10000 {
        let name = if number == 1 {
            "ClearFrame Cutouts".into()
        } else {
            format!("ClearFrame Cutouts {number}")
        };
        let path = parent.join(name);
        match fs::create_dir(&path) {
            Ok(()) => {
                fs::write(
                    path.join(batch_folder::OUTPUT_MARKER),
                    batch_folder::OUTPUT_MARKER_CONTENT,
                )
                .map_err(|error| error.to_string())?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Output folder is not writable: {error}")),
        }
    }
    Err("Too many existing output folders".into())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    output_directory: PathBuf,
    succeeded: usize,
    failed: usize,
    cancelled: usize,
    items: Vec<crate::jobs::BatchItemSnapshot>,
}

/// Keep a separate report after each run/retry; never overwrite a user's file.
pub fn save_batch_report(workspace: &Workspace, batch_id: u64) -> Result<(), String> {
    let snapshot = workspace.snapshot();
    let summary = snapshot
        .batches
        .iter()
        .find(|batch| batch.batch_id == batch_id)
        .ok_or("Batch disappeared")?;
    let mut items = Vec::new();
    for offset in (0..summary.total).step_by(200) {
        items.extend(workspace.batch_items(batch_id, offset, 200)?.items);
    }
    let report = Report {
        output_directory: PathBuf::from(&summary.output_directory),
        succeeded: summary.succeeded,
        failed: summary.failed,
        cancelled: summary.cancelled,
        items,
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    for number in 1..=10000 {
        let name = if number == 1 {
            "ClearFrame Report.json".into()
        } else {
            format!("ClearFrame Report {number}.json")
        };
        let mut temporary =
            tempfile::NamedTempFile::new_in(&report.output_directory).map_err(|e| e.to_string())?;
        use std::io::Write;
        temporary.write_all(&bytes).map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        match temporary.persist_noclobber(report.output_directory.join(name)) {
            Ok(_) => return Ok(()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("Too many existing reports".into())
}

pub fn context_main(args: impl Iterator<Item = OsString>) -> i32 {
    match run_context(args.collect()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("ClearFrame: {error}");
            1
        }
    }
}

pub fn run_context(args: Vec<OsString>) -> Result<i32, String> {
    let options = parse(args)?;
    let groups = plan(&options.mode, &options.paths)?;
    if groups.is_empty() {
        return Err("No supported images found in the selected folders".into());
    }
    // Shell invocations are independent processes. Prevent repeated right-clicks
    // from loading several full model sessions concurrently; OS releases on exit.
    let _lease = context_lock(&std::env::temp_dir())?;
    let cache = options.models.unwrap_or(application_data()?.join("models"));
    let runtime = options.runtime.unwrap_or(installed_runtime(
        &std::env::current_exe().map_err(|error| error.to_string())?,
    ));
    let spec = crate::model::find("isnet-auto-high").ok_or("IS-Net manifest missing")?;
    let model = cache.join(format!("{}-{}.onnx", spec.id, spec.revision));
    if !model.is_file() {
        return Err("IS-Net is not installed. Open ClearFrame Desktop and finish model setup, then retry. No images were processed.".into());
    }
    crate::runtime::initialize(&runtime).map_err(|error| error.to_string())?;
    let store = Arc::new(ModelStore::new(cache, |_| {})?);
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let snapshot = store.snapshot();
        let model = snapshot
            .models
            .iter()
            .find(|model| model.spec.id == "isnet-auto-high")
            .ok_or("IS-Net manifest missing")?;
        match model.state {
            ModelState::FileVerified | ModelState::SessionReady => break,
            ModelState::Missing | ModelState::Failed => {
                return Err(format!(
                    "IS-Net could not be verified. Open ClearFrame Desktop to repair the model. {}",
                    model.error.as_deref().unwrap_or("")
                ))
            }
            _ if Instant::now() >= deadline => {
                return Err("Model verification timed out; no jobs were accepted".into())
            }
            _ => thread::sleep(Duration::from_millis(50)),
        }
    }
    // The headless process uses the same bounded Rust scheduler and output rules.
    // It does not load, write or restore the desktop application's user session.
    let workspace = Workspace::new_with_inference(|_| {}, runtime, store)?;
    let mut failed = false;
    for (parent, inputs) in groups {
        let output = if options.mode == Mode::Batch {
            output_directory(&parent)?
        } else {
            parent
        };
        let paths: Vec<_> = inputs.into_iter().collect();
        let batch_id = workspace.create_batch(&output, &paths)?;
        eprintln!(
            "ClearFrame: processing {} image(s) → {}",
            paths.len(),
            output.display()
        );
        let summary = loop {
            let snapshot = workspace.snapshot();
            let summary = snapshot
                .batches
                .iter()
                .find(|batch| batch.batch_id == batch_id)
                .ok_or("Batch disappeared")?;
            if summary.paused {
                workspace.cancel_batch(batch_id)?;
            }
            if summary.queued + summary.running == 0 {
                break summary.clone();
            }
            thread::sleep(Duration::from_millis(80));
        };
        let mut items = Vec::new();
        for offset in (0..summary.total).step_by(200) {
            for item in workspace.batch_items(batch_id, offset, 200)?.items {
                if let Some(error) = &item.error {
                    eprintln!("{}: {error}", item.display_name);
                }
                items.push(item);
            }
        }
        failed |= summary.failed + summary.cancelled > 0;
        let report = Report {
            output_directory: output,
            succeeded: summary.succeeded,
            failed: summary.failed,
            cancelled: summary.cancelled,
            items,
        };
        if options.mode == Mode::Batch {
            let mut temporary = tempfile::NamedTempFile::new_in(&report.output_directory)
                .map_err(|error| error.to_string())?;
            serde_json::to_writer_pretty(&mut temporary, &report)
                .map_err(|error| error.to_string())?;
            temporary
                .as_file()
                .sync_all()
                .map_err(|error| error.to_string())?;
            temporary
                .persist_noclobber(report.output_directory.join("ClearFrame Report.json"))
                .map_err(|error| error.to_string())?;
        }
        println!(
            "{}",
            serde_json::to_string(&report).map_err(|error| error.to_string())?
        );
        workspace.remove_batch(batch_id)?;
    }
    Ok(if failed { 3 } else { 0 })
}

fn context_lock(directory: &Path) -> Result<fs::File, String> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("dev.silta.clearframe.desktop-context.lock"))
        .map_err(|error| error.to_string())?;
    file.try_lock().map_err(|error| {
        format!("Another ClearFrame right-click operation is running. Wait for it to finish and retry. {error}")
    })?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_shell_requests_cannot_replace_the_runtime_or_model_cache() {
        assert!(parse_selection(vec![
            "single".into(),
            "--models".into(),
            "other".into(),
            "--".into(),
            "image.png".into()
        ])
        .is_err());
        assert!(parse_selection(vec![
            "single".into(),
            "--runtime".into(),
            "other".into(),
            "--".into(),
            "image.png".into()
        ])
        .is_err());
    }

    #[test]
    fn reports_never_overwrite_an_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("sample.png");
        fs::write(&input, b"source").unwrap();
        let output = output_directory(directory.path()).unwrap();
        let report = output.join("ClearFrame Report.json");
        fs::write(&report, b"existing user file").unwrap();
        let workspace = Workspace::new(|_| {}).unwrap();
        let batch = workspace.create_paused_batch(&output, &[input]).unwrap();
        save_batch_report(&workspace, batch).unwrap();
        assert_eq!(fs::read(report).unwrap(), b"existing user file");
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(output.join("ClearFrame Report 2.json")).unwrap())
                .unwrap();
        assert_eq!(saved["items"].as_array().unwrap().len(), 1);
        assert_eq!(saved["succeeded"], 0);
    }
    #[test]
    fn concurrent_shell_invocations_are_rejected_and_exit_releases_lease() {
        let root = tempfile::tempdir().unwrap();
        let lease = context_lock(root.path()).unwrap();
        assert!(context_lock(root.path()).is_err());
        drop(lease);
        assert!(context_lock(root.path()).is_ok());
    }
    #[test]
    fn selection_paths_are_literal_and_single_rejects_multiple_or_folders() {
        let parsed = parse(vec!["single".into(), "--".into(), "--models.png".into()]).unwrap();
        assert_eq!(parsed.paths, vec![PathBuf::from("--models.png")]);
        assert!(parse(vec![
            "single".into(),
            "--".into(),
            "a.png".into(),
            "b.png".into()
        ])
        .is_err());
        let root = tempfile::tempdir().unwrap();
        assert!(plan(&Mode::Single, &[root.path().to_path_buf()]).is_err());
    }
    #[test]
    fn batches_group_by_source_folder_and_skip_previous_outputs_without_overwrite() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(root.path().join("one.PNG"), b"image").unwrap();
        fs::write(nested.join("two.webp"), b"image").unwrap();
        let first = output_directory(root.path()).unwrap();
        fs::write(first.join("old.png"), b"saved").unwrap();
        let second = output_directory(root.path()).unwrap();
        assert_ne!(first, second);
        let groups = plan(
            &Mode::Batch,
            &[root.path().to_path_buf(), root.path().join("one.PNG")],
        )
        .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups.values().next().unwrap().len(), 2);
        assert_eq!(fs::read(first.join("old.png")).unwrap(), b"saved");
        let groups = plan(
            &Mode::Batch,
            &[root.path().join("one.PNG"), nested.join("two.webp")],
        )
        .unwrap();
        assert_eq!(groups.len(), 2);
    }
    #[test]
    fn manifest_is_consumed_only_from_owned_temporary_location() {
        let file = tempfile::Builder::new()
            .prefix("clearframe-selection-")
            .suffix(".json")
            .tempfile()
            .unwrap();
        fs::write(file.path(), r#"["space and 日本語.png"]"#.as_bytes()).unwrap();
        let parsed = parse(vec![
            "batch".into(),
            "--selection-file".into(),
            file.path().into(),
        ])
        .unwrap();
        assert_eq!(parsed.paths.len(), 1);
        assert!(!file.path().exists());
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("clearframe-selection-input.json");
        fs::write(&path, b"[]").unwrap();
        assert!(parse(vec![
            "batch".into(),
            "--selection-file".into(),
            path.clone().into()
        ])
        .is_err());
        assert!(path.exists());
    }
}
