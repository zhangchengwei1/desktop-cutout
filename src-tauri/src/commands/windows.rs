use super::access::{main_window, settings_access};
#[cfg(target_os = "macos")]
use super::blocking;
#[cfg(target_os = "macos")]
use crate::context_menu;
use cutout_core::preferences::{Language, PreferencesStore};
use serde::Deserialize;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::sync::Arc;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UtilityView {
    Settings,
    Resources,
    Help,
    About,
}

impl UtilityView {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Resources => "resources",
            Self::Help => "help",
            Self::About => "about",
        }
    }
}

pub(crate) fn show_utility(app: &tauri::AppHandle, view: UtilityView) -> Result<(), String> {
    let label = view.label();
    if let Some(window) = app.get_webview_window(label) {
        window.unminimize().map_err(|error| error.to_string())?;
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
    } else {
        let language = app
            .state::<Arc<PreferencesStore>>()
            .snapshot()
            .value
            .language
            .unwrap_or(Language::En);
        let text = crate::application_menu::labels(language);
        let (title, width, height) = match view {
            UtilityView::Settings => (text.settings, 520.0, 540.0),
            UtilityView::Resources => (text.resources, 850.0, 680.0),
            UtilityView::Help => (text.help, 850.0, 680.0),
            UtilityView::About => (text.about, 700.0, 600.0),
        };
        WebviewWindowBuilder::new(
            app,
            label,
            WebviewUrl::App(format!("index.html?utility={label}").into()),
        )
        .title(title)
        .inner_size(width, height)
        .min_inner_size(420.0, 400.0)
        .build()
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn open_settings(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    main_window(&window)?;
    show_utility(&app, UtilityView::Settings)?;
    Ok(true)
}

#[tauri::command]
pub(crate) fn open_utility_window(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    view: UtilityView,
) -> Result<(), String> {
    if !matches!(
        window.label(),
        "main" | "settings" | "resources" | "help" | "about"
    ) {
        return Err("此窗口不可打开辅助窗口".into());
    }
    show_utility(&app, view)
}

#[tauri::command]
pub(crate) fn set_menu_language(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    language: Language,
) -> Result<(), String> {
    if !matches!(
        window.label(),
        "main" | "settings" | "resources" | "help" | "about"
    ) {
        return Err("此窗口不可修改菜单".into());
    }
    crate::application_menu::set_language(&app, language).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn configure_context_menu(window: tauri::WebviewWindow) -> Result<String, String> {
    settings_access(&window)?;
    #[cfg(target_os = "macos")]
    {
        blocking::run(|| {
            let executable = std::env::current_exe().map_err(|error| error.to_string())?;
            let application = executable
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .ok_or("Application path unavailable")?;
            match context_menu::configure(application)? {
                0 => Ok("configured".into()),
                2 => Ok("cancelled".into()),
                _ => Err("Could not update Finder actions".into()),
            }
        })
        .await
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok("installer_required".into())
    }
}

#[tauri::command]
pub(crate) fn set_shortcut_recording(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    recording: bool,
) -> Result<(), String> {
    settings_access(&window)?;
    crate::application_menu::set_recording(&app, recording).map_err(|error| error.to_string())
}
