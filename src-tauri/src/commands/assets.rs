use super::access::main_window;
use super::blocking;
use cutout_core::{
    edit::Stroke,
    jobs::{BackgroundChoice, ImportResult, Workflow, Workspace, WorkspaceSnapshot},
    sam::Point,
};
use std::sync::Arc;

#[tauri::command]
pub(crate) fn workspace_snapshot(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
) -> Result<WorkspaceSnapshot, String> {
    main_window(&window)?;
    Ok(workspace.snapshot())
}

#[tauri::command]
pub(crate) fn import_asset(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    path: String,
) -> Result<ImportResult, String> {
    main_window(&window)?;
    workspace.import(std::path::Path::new(&path))
}

#[tauri::command]
pub(crate) async fn paste_asset(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
) -> Result<Vec<ImportResult>, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || {
        let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
        if let Ok(paths) = clipboard.get().file_list() {
            if !paths.is_empty() {
                return paths.iter().map(|path| workspace.import(path)).collect();
            }
        }
        let image = clipboard
            .get_image()
            .map_err(|_| "剪贴板中没有可用图片".to_owned())?;
        let width = u32::try_from(image.width).map_err(|_| "剪贴板图片尺寸无效")?;
        let height = u32::try_from(image.height).map_err(|_| "剪贴板图片尺寸无效")?;
        workspace
            .import_clipboard_image(width, height, image.bytes.into_owned())
            .map(|item| vec![item])
    })
    .await
}

#[tauri::command]
pub(crate) async fn select_asset(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
) -> Result<u64, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || {
        let generation = workspace.select(asset_id)?;
        if workspace.snapshot().assets.iter().any(|asset| {
            asset.asset_id == asset_id && asset.state == cutout_core::jobs::AssetState::Released
        }) {
            workspace.hydrate_preview(asset_id)?;
        }
        Ok(generation)
    })
    .await
}

#[tauri::command]
pub(crate) fn cancel_task(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    task_id: u64,
) -> Result<(), String> {
    main_window(&window)?;
    workspace.cancel(task_id)
}

#[tauri::command]
pub(crate) fn retry_preview(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
) -> Result<u64, String> {
    main_window(&window)?;
    workspace.retry_preview(asset_id)
}

#[tauri::command]
pub(crate) fn release_preview(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
) -> Result<(), String> {
    main_window(&window)?;
    workspace.release_preview(asset_id)
}

#[tauri::command]
pub(crate) fn remove_asset(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
) -> Result<(), String> {
    main_window(&window)?;
    workspace.remove(asset_id)
}

#[tauri::command]
pub(crate) fn auto_cutout(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
) -> Result<u64, String> {
    main_window(&window)?;
    workspace.auto_cutout(asset_id)
}

#[tauri::command]
pub(crate) fn switch_workflow(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
    workflow: Workflow,
) -> Result<(), String> {
    main_window(&window)?;
    workspace.switch_workflow(asset_id, workflow)
}

#[tauri::command]
pub(crate) fn select_points(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
    points: Vec<Point>,
) -> Result<u64, String> {
    main_window(&window)?;
    workspace.select_points(asset_id, points)
}

#[tauri::command]
pub(crate) fn refine_mask(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
    expected_revision: u64,
    edge: u32,
) -> Result<u64, String> {
    main_window(&window)?;
    workspace.refine_mask(asset_id, expected_revision, edge)
}

#[tauri::command]
pub(crate) async fn set_background(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
    expected_mask_revision: u64,
    expected_background_revision: u64,
    choice: BackgroundChoice,
) -> Result<bool, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || {
        workspace.set_background(
            asset_id,
            expected_mask_revision,
            expected_background_revision,
            choice,
        )
    })
    .await
}

#[tauri::command]
pub(crate) async fn paint_mask(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
    expected_revision: u64,
    stroke: Stroke,
) -> Result<bool, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || workspace.paint_mask(asset_id, expected_revision, stroke)).await
}

#[tauri::command]
pub(crate) async fn step_history(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
    expected_revision: u64,
    redo: bool,
) -> Result<bool, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || workspace.step_history(asset_id, expected_revision, redo)).await
}

#[tauri::command]
pub(crate) fn export_png(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
    path: String,
) -> Result<u64, String> {
    main_window(&window)?;
    workspace.export_png(asset_id, std::path::PathBuf::from(path))
}
