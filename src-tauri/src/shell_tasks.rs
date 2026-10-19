//! OS selections enter the existing scheduler; no second inference process.
use cutout_core::{
    jobs::Workspace,
    model_store::{ModelState, ModelStore},
    preferences::{Language, PreferencesStore, ShellCompletion},
    shell::{self, Mode},
};
use serde::Serialize;
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

struct Operation {
    id: u64,
    args: Vec<OsString>,
    phase: &'static str,
    batches: Vec<u64>,
    directories: Vec<PathBuf>,
    batch_mode: bool,
    cancelled: bool,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationSnapshot {
    id: u64,
    phase: &'static str,
    total: usize,
    succeeded: usize,
    failed: usize,
    cancelled: usize,
    output_directories: Vec<PathBuf>,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct Snapshot {
    language: Option<Language>,
    operations: Vec<OperationSnapshot>,
}

pub struct ShellTasks {
    operations: Mutex<Vec<Operation>>,
    sender: mpsc::SyncSender<u64>,
    receiver: Mutex<Option<mpsc::Receiver<u64>>>,
    background: AtomicBool,
}

impl ShellTasks {
    pub fn new(background: bool) -> Self {
        let (sender, receiver) = mpsc::sync_channel(8);
        Self {
            operations: Mutex::new(vec![]),
            sender,
            receiver: Mutex::new(Some(receiver)),
            background: AtomicBool::new(background),
        }
    }

    pub fn submit(&self, app: &tauri::AppHandle, args: Vec<OsString>) -> Result<(), String> {
        let mut operations = self.operations.lock().unwrap();
        let id = operations.last().map_or(1, |operation| operation.id + 1);
        // Holding this lock keeps the worker from seeing an incomplete request.
        self.sender
            .try_send(id)
            .map_err(|_| "Too many pending right-click operations; wait and retry".to_owned())?;
        operations.push(Operation {
            id,
            args,
            phase: "queued",
            batches: vec![],
            directories: vec![],
            batch_mode: false,
            cancelled: false,
            error: None,
        });
        if operations.len() > 20 {
            if let Some(index) = operations
                .iter()
                .position(|operation| matches!(operation.phase, "done" | "cancelled" | "failed"))
            {
                operations.remove(index);
            }
        }
        drop(operations);
        if app
            .try_state::<Arc<PreferencesStore>>()
            .is_some_and(|store| {
                store.snapshot().value.shell_completion == ShellCompletion::Progress
            })
        {
            show_status(app);
        }
        Ok(())
    }

    pub fn start(self: &Arc<Self>, app: tauri::AppHandle) -> Result<(), std::io::Error> {
        let receiver = self
            .receiver
            .lock()
            .unwrap()
            .take()
            .expect("one shell worker");
        let tasks = Arc::clone(self);
        std::thread::Builder::new()
            .name("clearframe-shell-submissions".into())
            .spawn(move || {
                while let Ok(id) = receiver.recv() {
                    let result = tasks.process(&app, id);
                    if let Err(error) = result {
                        tasks.update(id, |operation| {
                            operation.phase = "failed";
                            operation.error = Some(error);
                        });
                    }
                    let status = tasks.snapshot(&app);
                    let Some(operation) = status
                        .operations
                        .iter()
                        .find(|operation| operation.id == id)
                    else {
                        continue;
                    };
                    if operation.phase == "failed" {
                        show_status(&app);
                    } else if operation.phase == "done" {
                        match app
                            .state::<Arc<PreferencesStore>>()
                            .snapshot()
                            .value
                            .shell_completion
                        {
                            ShellCompletion::Progress => show_status(&app),
                            ShellCompletion::OpenFolder => {
                                if let Err(error) = tasks.open_result(&app, id, 0) {
                                    tasks.update(id, |operation| {
                                        operation.phase = "failed";
                                        operation.error = Some(error);
                                    });
                                    show_status(&app);
                                } else if operation.output_directories.len() > 1 {
                                    // Keep every location accessible without opening a
                                    // Finder/Explorer window for each selected parent.
                                    show_status(&app);
                                }
                            }
                            ShellCompletion::Silent => {}
                        }
                    }
                }
            })?;
        Ok(())
    }

    fn update(&self, id: u64, change: impl FnOnce(&mut Operation)) {
        if let Some(operation) = self
            .operations
            .lock()
            .unwrap()
            .iter_mut()
            .find(|operation| operation.id == id)
        {
            change(operation);
        }
    }

    fn is_cancelled(&self, id: u64) -> bool {
        self.operations
            .lock()
            .unwrap()
            .iter()
            .find(|operation| operation.id == id)
            .is_none_or(|operation| operation.cancelled)
    }

    fn process(&self, app: &tauri::AppHandle, id: u64) -> Result<(), String> {
        if self.is_cancelled(id) {
            return Ok(());
        }
        let workspace = app.state::<Arc<Workspace>>();
        let (args, existing) = {
            let mut operations = self.operations.lock().unwrap();
            let operation = operations
                .iter_mut()
                .find(|operation| operation.id == id)
                .ok_or("Operation unavailable")?;
            operation.phase = "preparing";
            (operation.args.clone(), operation.batches.clone())
        };
        let selection = shell::parse_selection(args)?;
        // A consumed Windows manifest cannot be used again on retry.
        self.update(id, |operation| {
            operation.args = vec![
                if selection.mode == Mode::Batch {
                    "batch".into()
                } else {
                    "single".into()
                },
                "--".into(),
            ];
            operation.args.extend(
                selection
                    .paths
                    .iter()
                    .map(|path| path.as_os_str().to_owned()),
            );
            operation.batch_mode = selection.mode == Mode::Batch;
        });
        let store = app.state::<Arc<ModelStore>>();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if self.is_cancelled(id) {
                return Ok(());
            }
            let snapshot = store.snapshot();
            let model = snapshot
                .models
                .iter()
                .find(|model| model.spec.id == "isnet-auto-high")
                .ok_or("IS-Net manifest missing")?;
            match model.state {
                ModelState::FileVerified | ModelState::SessionReady => break,
                ModelState::Missing | ModelState::Failed | ModelState::Cancelled => return Err(format!("IS-Net is not ready. Open ClearFrame to prepare or repair the model, then retry. {}", model.error.as_deref().unwrap_or(""))),
                _ if Instant::now() >= deadline => return Err("Model preparation timed out. Open ClearFrame and retry after setup.".into()),
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        if existing.is_empty() {
            let groups = shell::plan_selection(&selection)?;
            let admitted = (|| -> Result<(), String> {
                for (parent, inputs) in groups {
                    // Cancellation and acceptance share a lock: cancelled imports
                    // cannot publish a new batch after the cancel button returned.
                    let mut operations = self.operations.lock().unwrap();
                    let operation = operations
                        .iter_mut()
                        .find(|operation| operation.id == id)
                        .ok_or("Operation unavailable")?;
                    if operation.cancelled {
                        return Ok(());
                    }
                    let output = if selection.mode == Mode::Batch {
                        shell::output_directory(&parent)?
                    } else {
                        parent
                    };
                    let batch =
                        workspace.create_batch(&output, &inputs.into_iter().collect::<Vec<_>>())?;
                    operation.directories.push(output);
                    operation.batches.push(batch);
                }
                Ok(())
            })();
            if let Err(error) = admitted {
                self.update(id, |operation| {
                    for batch in &operation.batches {
                        let _ = workspace.cancel_batch(*batch);
                        let _ = workspace.remove_batch(*batch);
                    }
                    operation.batches.clear();
                    operation.directories.clear();
                });
                return Err(error);
            }
            let operations = self.operations.lock().unwrap();
            let operation = operations
                .iter()
                .find(|operation| operation.id == id)
                .ok_or("Operation unavailable")?;
            if operation.cancelled {
                return Ok(());
            }
            for batch in &operation.batches {
                workspace.set_batch_paused(*batch, false)?;
            }
        } else {
            let operations = self.operations.lock().unwrap();
            if operations
                .iter()
                .find(|operation| operation.id == id)
                .is_none_or(|operation| operation.cancelled)
            {
                return Ok(());
            }
            for batch in existing {
                workspace.retry_failed_batch(batch)?;
                workspace.set_batch_paused(batch, false)?;
            }
        }
        self.update(id, |operation| operation.phase = "processing");
        loop {
            let snapshot = workspace.snapshot();
            let ids = self
                .operations
                .lock()
                .unwrap()
                .iter()
                .find(|operation| operation.id == id)
                .map(|operation| operation.batches.clone())
                .unwrap_or_default();
            let summaries: Vec<_> = snapshot
                .batches
                .iter()
                .filter(|batch| ids.contains(&batch.batch_id))
                .collect();
            if summaries.len() != ids.len() {
                return Err("A task was removed from the workbench. Saved images are kept.".into());
            }
            let paused = summaries
                .iter()
                .any(|batch| batch.paused && batch.queued + batch.running > 0);
            if paused
                || summaries
                    .iter()
                    .all(|batch| batch.queued + batch.running == 0)
            {
                let failed = paused || summaries.iter().any(|batch| batch.failed > 0);
                let batch_mode = self
                    .operations
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|operation| operation.id == id)
                    .is_some_and(|operation| operation.batch_mode);
                if batch_mode {
                    for batch in &ids {
                        shell::save_batch_report(&workspace, *batch)?;
                    }
                }
                self.update(id, |operation| {
                    operation.phase = if operation.cancelled { "cancelled" } else if failed { "failed" } else { "done" };
                    if paused { operation.error = Some("Processing paused after a runtime failure. Saved images are kept; repair the runtime or model, then retry.".into()); }
                });
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn snapshot(&self, app: &tauri::AppHandle) -> Snapshot {
        let workspace = app.state::<Arc<Workspace>>().snapshot();
        let operations = self
            .operations
            .lock()
            .unwrap()
            .iter()
            .rev()
            .map(|operation| {
                let summaries: Vec<_> = workspace
                    .batches
                    .iter()
                    .filter(|batch| operation.batches.contains(&batch.batch_id))
                    .collect();
                let item_error = if operation.phase == "failed" {
                    operation.batches.iter().find_map(|batch| {
                        app.state::<Arc<Workspace>>()
                            .filtered_batch_items(
                                *batch,
                                0,
                                1,
                                cutout_core::jobs::BatchFilter::Issues,
                            )
                            .ok()?
                            .items
                            .first()?
                            .error
                            .clone()
                    })
                } else {
                    None
                };
                OperationSnapshot {
                    id: operation.id,
                    phase: if operation.cancelled {
                        if summaries
                            .iter()
                            .any(|batch| batch.queued + batch.running > 0)
                        {
                            "cancelling"
                        } else {
                            "cancelled"
                        }
                    } else {
                        operation.phase
                    },
                    total: summaries.iter().map(|batch| batch.total).sum(),
                    succeeded: summaries.iter().map(|batch| batch.succeeded).sum(),
                    failed: summaries.iter().map(|batch| batch.failed).sum(),
                    cancelled: summaries.iter().map(|batch| batch.cancelled).sum(),
                    output_directories: operation.directories.clone(),
                    error: operation.error.clone().or(item_error),
                }
            })
            .collect();
        Snapshot {
            language: app
                .state::<Arc<PreferencesStore>>()
                .snapshot()
                .value
                .language,
            operations,
        }
    }

    pub fn has_pending(&self) -> bool {
        self.operations
            .lock()
            .unwrap()
            .iter()
            .any(|operation| matches!(operation.phase, "queued" | "preparing" | "processing"))
    }

    pub fn cancel(&self, app: &tauri::AppHandle, id: u64) -> Result<(), String> {
        let mut operations = self.operations.lock().unwrap();
        let operation = operations
            .iter_mut()
            .find(|operation| operation.id == id)
            .ok_or("Operation unavailable")?;
        operation.cancelled = true;
        operation.phase = if operation.batches.is_empty() {
            "cancelled"
        } else {
            "cancelling"
        };
        for batch in &operation.batches {
            app.state::<Arc<Workspace>>().cancel_batch(*batch)?;
        }
        Ok(())
    }

    pub fn retry(&self, id: u64) -> Result<(), String> {
        let mut operations = self.operations.lock().unwrap();
        let operation = operations
            .iter_mut()
            .find(|operation| operation.id == id)
            .ok_or("Operation unavailable")?;
        if operation.phase != "failed" {
            return Err("Only failed operations can be retried".into());
        }
        self.sender
            .try_send(id)
            .map_err(|_| "Too many pending operations; wait and retry".to_owned())?;
        operation.phase = "queued";
        operation.error = None;
        Ok(())
    }

    pub fn open_result(&self, app: &tauri::AppHandle, id: u64, index: usize) -> Result<(), String> {
        let directory = self
            .operations
            .lock()
            .unwrap()
            .iter()
            .find(|operation| operation.id == id)
            .and_then(|operation| operation.directories.get(index))
            .cloned()
            .ok_or("Result directory unavailable")?;
        if !directory.is_dir() {
            return Err("Result folder has moved or is no longer available".into());
        }
        app.opener()
            .open_path(directory.to_string_lossy().as_ref(), None::<&str>)
            .map_err(|error| error.to_string())
    }

    pub fn show_main(&self, app: &tauri::AppHandle, batch: bool) -> Result<(), String> {
        let window = app
            .get_webview_window("main")
            .ok_or("Main window unavailable")?;
        if self.background.swap(false, Ordering::SeqCst) {
            let mut url = window.url().map_err(|error| error.to_string())?;
            url.set_query(if batch { Some("batch=1") } else { None });
            window.navigate(url).map_err(|error| error.to_string())?;
        } else if batch {
            let _ = app.emit_to("main", "shell-open-workbench", ());
        }
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())
    }
}

fn show_status(app: &tauri::AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window("shell-status") {
            let _ = window.show();
        } else {
            let _ = tauri::WebviewWindowBuilder::new(
                &handle,
                "shell-status",
                tauri::WebviewUrl::App("index.html?shell-status=1".into()),
            )
            .title("ClearFrame")
            .inner_size(420.0, 360.0)
            .min_inner_size(360.0, 320.0)
            .resizable(true)
            .focused(false)
            .skip_taskbar(true)
            .build();
        }
    });
}

fn access(window: &tauri::WebviewWindow) -> Result<(), String> {
    if matches!(window.label(), "shell-status" | "main") {
        Ok(())
    } else {
        Err("This window cannot access shell operations".into())
    }
}

#[tauri::command]
pub fn shell_snapshot(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    tasks: tauri::State<'_, Arc<ShellTasks>>,
) -> Result<Snapshot, String> {
    access(&window)?;
    Ok(tasks.snapshot(&app))
}
#[tauri::command]
pub fn shell_cancel(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    tasks: tauri::State<'_, Arc<ShellTasks>>,
    id: u64,
) -> Result<(), String> {
    access(&window)?;
    tasks.cancel(&app, id)
}
#[tauri::command]
pub fn shell_retry(
    window: tauri::WebviewWindow,
    tasks: tauri::State<'_, Arc<ShellTasks>>,
    id: u64,
) -> Result<(), String> {
    access(&window)?;
    tasks.retry(id)
}
#[tauri::command]
pub fn shell_open_result(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    tasks: tauri::State<'_, Arc<ShellTasks>>,
    id: u64,
    index: usize,
) -> Result<(), String> {
    access(&window)?;
    tasks.open_result(&app, id, index)
}
#[tauri::command]
pub fn shell_open_workbench(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    tasks: tauri::State<'_, Arc<ShellTasks>>,
) -> Result<(), String> {
    access(&window)?;
    tasks.show_main(&app, true)
}
