use super::access::model_access;
use super::blocking;
use cutout_core::model_store::{ModelStore, ModelStoreSnapshot};
use std::sync::Arc;

#[tauri::command]
pub(crate) fn runtime_info(
    model_store: tauri::State<'_, Arc<ModelStore>>,
) -> cutout_core::RuntimeInfo {
    let mut info = cutout_core::runtime_info();
    info.inference_ready = model_store.snapshot().features.automatic.session_ready;
    info
}

#[tauri::command]
pub(crate) fn model_snapshot(
    window: tauri::WebviewWindow,
    model_store: tauri::State<'_, Arc<ModelStore>>,
) -> Result<ModelStoreSnapshot, String> {
    model_access(&window)?;
    Ok(model_store.snapshot())
}

#[tauri::command]
pub(crate) fn download_model(
    window: tauri::WebviewWindow,
    model_store: tauri::State<'_, Arc<ModelStore>>,
    model_id: String,
) -> Result<(), String> {
    model_access(&window)?;
    model_store.download(&model_id)
}

#[tauri::command]
pub(crate) fn cancel_model_download(
    window: tauri::WebviewWindow,
    model_store: tauri::State<'_, Arc<ModelStore>>,
    model_id: String,
) -> Result<(), String> {
    model_access(&window)?;
    model_store.cancel(&model_id)
}

#[tauri::command]
pub(crate) fn clear_model(
    window: tauri::WebviewWindow,
    model_store: tauri::State<'_, Arc<ModelStore>>,
    model_id: String,
) -> Result<(), String> {
    model_access(&window)?;
    model_store.clear(&model_id)
}

#[tauri::command]
pub(crate) async fn open_model_license(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    model_id: String,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    model_access(&window)?;
    let url = cutout_core::model::find(&model_id)
        .ok_or("模型不存在")?
        .license_url;
    blocking::run(move || {
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|error| error.to_string())
    })
    .await
}
