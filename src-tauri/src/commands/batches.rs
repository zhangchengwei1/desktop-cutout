use super::access::main_window;
use super::blocking;
use cutout_core::jobs::{BatchFilter, BatchFolderImport, BatchPage, ImportResult, Workspace};
use std::{path::Path, sync::Arc};

#[tauri::command]
pub(crate) async fn create_batch(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    output_dir: String,
    paths: Vec<String>,
) -> Result<u64, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || {
        let paths = paths
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect::<Vec<_>>();
        workspace.create_batch(std::path::Path::new(&output_dir), &paths)
    })
    .await
}

#[tauri::command]
pub(crate) async fn create_batch_from_folder(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    output_dir: String,
    folder: String,
) -> Result<BatchFolderImport, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || {
        workspace.create_batch_from_folder(Path::new(&output_dir), Path::new(&folder))
    })
    .await
}

#[tauri::command]
pub(crate) async fn append_batch_folder(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    batch_id: u64,
    folder: String,
) -> Result<BatchFolderImport, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || workspace.append_batch_folder(batch_id, Path::new(&folder))).await
}

#[tauri::command]
pub(crate) async fn create_batch_from_zip(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    output_dir: String,
    archive: String,
) -> Result<u64, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || {
        workspace.create_batch_from_zip(
            std::path::Path::new(&output_dir),
            std::path::Path::new(&archive),
        )
    })
    .await
}

#[tauri::command]
pub(crate) async fn append_batch(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    batch_id: u64,
    paths: Vec<String>,
) -> Result<usize, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || {
        let paths = paths
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect::<Vec<_>>();
        workspace.append_batch(batch_id, &paths)
    })
    .await
}

#[tauri::command]
pub(crate) async fn append_batch_zip(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    batch_id: u64,
    archive: String,
) -> Result<usize, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || workspace.append_batch_zip(batch_id, std::path::Path::new(&archive)))
        .await
}

#[tauri::command]
pub(crate) async fn export_batch_zip(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    batch_id: u64,
    destination: String,
) -> Result<usize, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || workspace.export_batch_zip(batch_id, std::path::Path::new(&destination)))
        .await
}

#[tauri::command]
pub(crate) fn batch_items(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    batch_id: u64,
    offset: usize,
    limit: usize,
    filter: Option<BatchFilter>,
) -> Result<BatchPage, String> {
    main_window(&window)?;
    workspace.filtered_batch_items(batch_id, offset, limit, filter.unwrap_or_default())
}

#[tauri::command]
pub(crate) async fn review_batch_item(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    item_id: u64,
) -> Result<ImportResult, String> {
    main_window(&window)?;
    let workspace = Arc::clone(&workspace);
    blocking::run(move || workspace.review_batch_item(item_id)).await
}

#[tauri::command]
pub(crate) fn set_batch_paused(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    batch_id: u64,
    paused: bool,
) -> Result<(), String> {
    main_window(&window)?;
    workspace.set_batch_paused(batch_id, paused)
}

#[tauri::command]
pub(crate) fn cancel_batch(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    batch_id: u64,
) -> Result<(), String> {
    main_window(&window)?;
    workspace.cancel_batch(batch_id)
}

#[tauri::command]
pub(crate) fn retry_batch_item(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    item_id: u64,
) -> Result<(), String> {
    main_window(&window)?;
    workspace.retry_batch_item(item_id)
}

#[tauri::command]
pub(crate) fn retry_failed_batch(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    batch_id: u64,
) -> Result<usize, String> {
    main_window(&window)?;
    workspace.retry_failed_batch(batch_id)
}

#[tauri::command]
pub(crate) fn remove_batch(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    batch_id: u64,
) -> Result<(), String> {
    main_window(&window)?;
    workspace.remove_batch(batch_id)
}
