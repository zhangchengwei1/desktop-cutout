use crate::commands::windows::{show_utility, UtilityView};
use cutout_core::preferences::{ExportNaming, Language, PreferencesStore, ShellCompletion};
use cutout_core::recovery::RecoveryController;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tauri::{
    menu::{CheckMenuItem, MenuBuilder, MenuItem, SubmenuBuilder},
    Emitter, Manager,
};
use tauri_plugin_dialog::DialogExt;

#[derive(Deserialize)]
struct MenuResource {
    ui: Labels,
}

#[derive(Deserialize)]
pub(crate) struct Labels {
    pub settings: String,
    pub resources: String,
    pub help: String,
    pub about: String,
    edit: String,
    language: String,
    naming: String,
    completion: String,
    workspace: String,
    system: String,
    progress: String,
    #[serde(rename = "openFolder")]
    open_folder: String,
    silent: String,
    actions: BTreeMap<String, String>,
}

pub(crate) fn labels(language: Language) -> Labels {
    let json = match language {
        Language::En => include_str!("../../src/i18n/locales/en/menu.json"),
        Language::Zh => include_str!("../../src/i18n/locales/zh/menu.json"),
        Language::Ja => include_str!("../../src/i18n/locales/ja/menu.json"),
    };
    serde_json::from_str::<MenuResource>(json)
        .expect("invalid bundled menu translations")
        .ui
}

struct MenuLanguage(Mutex<Language>, std::sync::atomic::AtomicBool);

pub(crate) fn install(app: &tauri::AppHandle) -> tauri::Result<()> {
    let language = app
        .state::<Arc<PreferencesStore>>()
        .snapshot()
        .value
        .language
        .unwrap_or(Language::En);
    app.manage(MenuLanguage(Mutex::new(language), false.into()));
    refresh(app)
}

pub(crate) fn set_language(app: &tauri::AppHandle, language: Language) -> tauri::Result<()> {
    *app.state::<MenuLanguage>().0.lock().unwrap() = language;
    refresh(app)
}

pub(crate) fn set_recording(app: &tauri::AppHandle, recording: bool) -> tauri::Result<()> {
    app.state::<MenuLanguage>()
        .1
        .store(recording, std::sync::atomic::Ordering::Relaxed);
    refresh(app)
}

pub(crate) fn refresh(app: &tauri::AppHandle) -> tauri::Result<()> {
    let language = *app.state::<MenuLanguage>().0.lock().unwrap();
    let text = labels(language);
    let preferences = app.state::<Arc<PreferencesStore>>().snapshot();
    let value = &preferences.value;
    let shortcuts = cutout_core::shortcuts::resolved(&value.shortcuts);
    let item = |id: &str, title: &str| {
        MenuItem::with_id(
            app,
            id,
            title,
            true,
            shortcuts.get(id).map(String::as_str).filter(|key| {
                !key.is_empty()
                    && !app
                        .state::<MenuLanguage>()
                        .1
                        .load(std::sync::atomic::Ordering::Relaxed)
            }),
        )
    };
    let check = |id: &str, title: &str, checked: bool| {
        CheckMenuItem::with_id(
            app,
            id,
            title,
            preferences.load_error.is_none(),
            checked,
            None::<&str>,
        )
    };
    let languages = SubmenuBuilder::new(app, &text.language)
        .item(&check(
            "language:system",
            &text.system,
            value.language.is_none(),
        )?)
        .item(&check(
            "language:en",
            "English",
            value.language == Some(Language::En),
        )?)
        .item(&check(
            "language:zh",
            "中文",
            value.language == Some(Language::Zh),
        )?)
        .item(&check(
            "language:ja",
            "日本語",
            value.language == Some(Language::Ja),
        )?)
        .build()?;
    let naming = SubmenuBuilder::new(app, &text.naming)
        .item(&check(
            "naming:cutout",
            "-cutout.png",
            value.export_naming == ExportNaming::Cutout,
        )?)
        .item(&check(
            "naming:clearframe",
            "-clearframe.png",
            value.export_naming == ExportNaming::Clearframe,
        )?)
        .build()?;
    let completion = SubmenuBuilder::new(app, &text.completion)
        .item(&check(
            "completion:progress",
            &text.progress,
            value.shell_completion == ShellCompletion::Progress,
        )?)
        .item(&check(
            "completion:open_folder",
            &text.open_folder,
            value.shell_completion == ShellCompletion::OpenFolder,
        )?)
        .item(&check(
            "completion:silent",
            &text.silent,
            value.shell_completion == ShellCompletion::Silent,
        )?)
        .build()?;
    let application = SubmenuBuilder::with_id(app, "application", "ClearFrame")
        .item(&item("about", &text.about)?)
        .separator()
        .item(&item("settings", &text.settings)?)
        .item(&languages)
        .item(&naming)
        .item(&completion)
        .separator()
        .item(&item("resources", &text.resources)?)
        .item(&item("help", &text.help)?)
        .separator()
        .quit()
        .build()?;
    let mut workspace = SubmenuBuilder::new(app, &text.workspace);
    for id in [
        "import",
        "export",
        "automatic",
        "toggleOriginal",
        "workbench",
        "batch",
    ] {
        workspace = workspace.item(&item(id, &text.actions[id])?);
    }
    let edit = SubmenuBuilder::new(app, &text.edit)
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .build()?;
    app.set_menu(
        MenuBuilder::new(app)
            .item(&application)
            .item(&workspace.build()?)
            .item(&edit)
            .build()?,
    )?;
    for (view, title) in [
        (UtilityView::Settings, text.settings),
        (UtilityView::Resources, text.resources),
        (UtilityView::Help, text.help),
        (UtilityView::About, text.about),
    ] {
        if let Some(window) = app.get_webview_window(view.label()) {
            window.set_title(&title)?;
        }
    }
    Ok(())
}

pub(crate) fn handle(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    let view = match id {
        "settings" => Some(UtilityView::Settings),
        "resources" => Some(UtilityView::Resources),
        "help" => Some(UtilityView::Help),
        "about" => Some(UtilityView::About),
        _ => None,
    };
    if let Some(view) = view {
        if let Err(error) = show_utility(app, view) {
            app.dialog().message(error).title("ClearFrame").show(|_| {});
        }
        return;
    }
    if [
        "import",
        "export",
        "automatic",
        "toggleOriginal",
        "workbench",
        "batch",
    ]
    .contains(&id)
    {
        if let Some(window) = app.get_webview_window("main") {
            let result = window
                .unminimize()
                .and_then(|_| window.show())
                .and_then(|_| window.set_focus())
                .and_then(|_| window.emit("application-action", id));
            if let Err(error) = result {
                app.dialog()
                    .message(error.to_string())
                    .title("ClearFrame")
                    .show(|_| {});
            }
        }
        return;
    }
    if !["language:", "naming:", "completion:"]
        .iter()
        .any(|prefix| id.starts_with(prefix))
    {
        return;
    }
    let app = app.clone();
    let id = id.to_owned();
    // Atomic disk writes and recovery gating must not block the UI thread.
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<Arc<PreferencesStore>>();
        let snapshot = store.snapshot();
        let mut value = snapshot.value;
        match id.as_str() {
            "language:system" => value.language = None,
            "language:en" => value.language = Some(Language::En),
            "language:zh" => value.language = Some(Language::Zh),
            "language:ja" => value.language = Some(Language::Ja),
            "naming:cutout" => value.export_naming = ExportNaming::Cutout,
            "naming:clearframe" => value.export_naming = ExportNaming::Clearframe,
            "completion:progress" => value.shell_completion = ShellCompletion::Progress,
            "completion:open_folder" => value.shell_completion = ShellCompletion::OpenFolder,
            "completion:silent" => value.shell_completion = ShellCompletion::Silent,
            _ => return,
        }
        if let Err(error) = app
            .state::<Arc<RecoveryController>>()
            .update_other_preferences(snapshot.revision, value)
        {
            app.dialog().message(error).title("ClearFrame").show(|_| {});
            let _ = refresh(&app); // Restore checkmarks after a failed write.
        }
    });
}
