use super::access::{preferences_read_access, settings_access};
use super::blocking;
use cutout_core::preferences::{Preferences, PreferencesSnapshot, PreferencesStore};
use cutout_core::recovery::{RecoveryController, RecoveryStatus};
use std::sync::Arc;

#[tauri::command]
pub(crate) fn preferences_snapshot(
    window: tauri::WebviewWindow,
    store: tauri::State<'_, Arc<PreferencesStore>>,
) -> Result<PreferencesSnapshot, String> {
    preferences_read_access(&window)?;
    Ok(store.snapshot())
}

#[tauri::command]
pub(crate) async fn update_preferences(
    window: tauri::WebviewWindow,
    recovery: tauri::State<'_, Arc<RecoveryController>>,
    expected_revision: u64,
    value: Preferences,
) -> Result<PreferencesSnapshot, String> {
    settings_access(&window)?;
    let recovery = Arc::clone(&recovery);
    blocking::run(move || recovery.update_other_preferences(expected_revision, value)).await
}

#[tauri::command]
pub(crate) fn recovery_status(
    window: tauri::WebviewWindow,
    recovery: tauri::State<'_, Arc<RecoveryController>>,
) -> Result<RecoveryStatus, String> {
    settings_access(&window)?;
    Ok(recovery.status())
}

#[tauri::command]
pub(crate) async fn set_session_recovery(
    window: tauri::WebviewWindow,
    recovery: tauri::State<'_, Arc<RecoveryController>>,
    expected_revision: u64,
    enabled: bool,
) -> Result<PreferencesSnapshot, String> {
    settings_access(&window)?;
    let recovery = Arc::clone(&recovery);
    blocking::run(move || recovery.set_enabled(expected_revision, enabled)).await
}

#[tauri::command]
pub(crate) async fn clear_session_data(
    window: tauri::WebviewWindow,
    recovery: tauri::State<'_, Arc<RecoveryController>>,
) -> Result<(), String> {
    settings_access(&window)?;
    let recovery = Arc::clone(&recovery);
    blocking::run(move || recovery.clear_disabled_data()).await
}
