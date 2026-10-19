#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod application_menu;
mod commands;
mod context_menu;
mod lifecycle;
mod previews;
mod runtime_paths;
mod shell_tasks;

use cutout_core::{
    jobs::Workspace, model_store::ModelStore, preferences::PreferencesStore,
    recovery::RecoveryController, session::SessionStore,
};
use lifecycle::finish_exit;
use previews::preview_response;
use runtime_paths::runtime_library_path;
use shell_tasks::{
    shell_cancel, shell_open_result, shell_open_workbench, shell_retry, shell_snapshot, ShellTasks,
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--context-cutout")) {
        let result = cutout_core::shell::run_context(std::env::args_os().skip(2).collect());
        let code = match result {
            Ok(code) => {
                #[cfg(target_os = "windows")]
                if code != 0 {
                    context_menu::report_failure("Some inputs could not be processed.");
                }
                code
            }
            Err(error) => {
                eprintln!("ClearFrame: {error}");
                #[cfg(target_os = "windows")]
                context_menu::report_failure(&error);
                1
            }
        };
        std::process::exit(code);
    }
    #[cfg(target_os = "macos")]
    if std::env::args().nth(1).as_deref() == Some("--configure-context-menu") {
        let application = std::env::current_exe().ok().and_then(|path| {
            path.parent()?
                .parent()?
                .parent()
                .map(std::path::Path::to_owned)
        });
        let code = application
            .ok_or("Application path unavailable".to_owned())
            .and_then(|application| context_menu::configure(&application));
        std::process::exit(code.unwrap_or_else(|error| {
            eprintln!("{error}");
            1
        }));
    }
    // Installer smoke check uses the real signed executable, without a GUI or models.
    if std::env::args().nth(1).as_deref() == Some("--verify-runtime") {
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let executable = std::env::current_exe()?;
            let directory = executable.parent().ok_or("应用路径无效")?;
            #[cfg(target_os = "macos")]
            let resources = directory.parent().ok_or("应用路径无效")?.join("Resources");
            #[cfg(not(target_os = "macos"))]
            let resources = directory.to_path_buf();
            let runtime = runtime_library_path(&resources, None, None);
            cutout_core::runtime::initialize(&runtime)?;
            println!("Packaged ONNX Runtime loaded: {}", runtime.display());
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let shell_start =
        std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--shell-cutout"));
    let shell_tasks = Arc::new(ShellTasks::new(shell_start));
    let mut context = tauri::generate_context!();
    if shell_start {
        if let Some(window) = context
            .config_mut()
            .app
            .windows
            .iter_mut()
            .find(|window| window.label == "main")
        {
            window.visible = false;
            window.focus = false;
            window.url = tauri::WebviewUrl::App("index.html?background=1".into());
        }
    }
    tauri::Builder::default()
        .manage(Arc::clone(&shell_tasks))
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let tasks = app.state::<Arc<ShellTasks>>();
            if args
                .get(1)
                .is_some_and(|argument| argument == "--shell-cutout")
            {
                if let Err(error) =
                    tasks.submit(app, args.into_iter().skip(2).map(OsString::from).collect())
                {
                    app.dialog().message(error).title("ClearFrame").show(|_| {});
                }
            } else {
                let _ = tasks.show_main(app, false);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            if window.label() == "settings"
                && matches!(
                    event,
                    tauri::WindowEvent::Destroyed | tauri::WindowEvent::Focused(false)
                )
            {
                let _ = application_menu::set_recording(window.app_handle(), false);
            }
            if window.label() == "shell-status" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            #[cfg(target_os = "macos")]
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (window, event);
        })
        .register_asynchronous_uri_scheme_protocol(
            "clearframe-preview",
            |context, request, responder| {
                let main_webview = context.webview_label() == "main";
                let workspace = Arc::clone(&context.app_handle().state::<Arc<Workspace>>());
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(preview_response(main_webview, &workspace, request));
                });
            },
        )
        .on_menu_event(application_menu::handle)
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            context_menu::offer_after_install();
            let handle = app.handle().clone();
            let preferences = PreferencesStore::open(
                app.path().app_local_data_dir()?.join("preferences.json"),
                move |revision| {
                    let _ = handle.emit("preferences-changed", revision);
                    let menu_handle = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        if let Err(error) = application_menu::refresh(&menu_handle) {
                            menu_handle
                                .dialog()
                                .message(error.to_string())
                                .title("ClearFrame")
                                .show(|_| {});
                        }
                    });
                },
            );
            let preferences = Arc::new(preferences);
            app.manage(Arc::clone(&preferences));
            application_menu::install(app.handle())?;
            let model_dir = if cfg!(debug_assertions) {
                std::env::var_os("CLEARFRAME_MODEL_DIR")
                    .map(std::path::PathBuf::from)
                    .unwrap_or(app.path().app_local_data_dir()?.join("models"))
            } else {
                app.path().app_local_data_dir()?.join("models")
            };
            let handle = app.handle().clone();
            let model_store = Arc::new(
                ModelStore::new(model_dir, move |revision| {
                    let _ = handle.emit("models-changed", revision);
                })
                .map_err(std::io::Error::other)?,
            );
            app.manage(Arc::clone(&model_store));
            #[cfg(all(debug_assertions, target_os = "macos"))]
            let development_runtime = Some(Path::new(env!("CARGO_MANIFEST_DIR")).join(
                "../test-output/onnxruntime-osx-arm64-1.28.0/lib/libonnxruntime.1.28.0.dylib",
            ));
            #[cfg(not(all(debug_assertions, target_os = "macos")))]
            let development_runtime: Option<PathBuf> = None;
            let runtime_path = runtime_library_path(
                &app.path().resource_dir()?,
                if cfg!(debug_assertions) {
                    std::env::var_os("CLEARFRAME_ORT_LIBRARY").map(PathBuf::from)
                } else {
                    None
                },
                development_runtime.as_deref(),
            );
            let (save_tx, save_rx) = mpsc::sync_channel::<()>(1);
            let handle = app.handle().clone();
            let workspace = Workspace::new_with_inference(
                move |revision| {
                    let _ = handle.emit("workspace-changed", revision);
                    let _ = save_tx.try_send(());
                },
                runtime_path,
                model_store,
            )
            .map_err(std::io::Error::other)?;
            let workspace = Arc::new(workspace);
            let handle = app.handle().clone();
            let recovery = Arc::new(RecoveryController::new(
                preferences,
                Arc::clone(&workspace),
                SessionStore::new(app.path().app_local_data_dir()?.join("session")),
                move || {
                    let _ = handle.emit("recovery-changed", ());
                },
            ));
            let restored = if recovery.status().enabled {
                recovery.restore_at_startup().unwrap_or(false)
            } else {
                // Finish cleanup after a crash between opting out and deleting
                // the old private copies.
                let _ = recovery.clear_disabled_data();
                false
            };
            app.manage(Arc::clone(&workspace));
            app.manage(Arc::clone(&recovery));
            shell_tasks.start(app.handle().clone())?;
            if shell_start {
                if let Err(error) =
                    shell_tasks.submit(app.handle(), std::env::args_os().skip(2).collect())
                {
                    app.dialog().message(error).title("ClearFrame").show(|_| {});
                }
            }
            let background_recovery = Arc::downgrade(&recovery);
            std::thread::Builder::new()
                .name("clearframe-session-save".to_owned())
                .spawn(move || {
                    while save_rx.recv().is_ok() {
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while Instant::now() < deadline
                            && save_rx.recv_timeout(Duration::from_millis(1200)).is_ok()
                        {
                        }
                        let Some(recovery) = background_recovery.upgrade() else {
                            break;
                        };
                        let _ = recovery.save_if_enabled();
                    }
                })?;
            if restored {
                if let Some(asset_id) = workspace.snapshot().selected_asset_id {
                    let recovery = Arc::clone(&recovery);
                    tauri::async_runtime::spawn_blocking(move || {
                        if let Err(error) = workspace.hydrate_preview(asset_id) {
                            recovery.report_preview_error(error);
                        }
                    });
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::models::runtime_info,
            commands::models::model_snapshot,
            commands::models::download_model,
            commands::models::cancel_model_download,
            commands::models::clear_model,
            commands::models::open_model_license,
            commands::preferences::preferences_snapshot,
            commands::preferences::update_preferences,
            commands::preferences::recovery_status,
            commands::preferences::set_session_recovery,
            commands::preferences::clear_session_data,
            commands::windows::open_settings,
            commands::windows::open_utility_window,
            commands::windows::set_menu_language,
            commands::windows::set_shortcut_recording,
            commands::windows::configure_context_menu,
            commands::assets::workspace_snapshot,
            commands::assets::import_asset,
            commands::batches::create_batch,
            commands::batches::create_batch_from_folder,
            commands::batches::append_batch_folder,
            commands::batches::create_batch_from_zip,
            commands::batches::append_batch,
            commands::batches::append_batch_zip,
            commands::batches::export_batch_zip,
            commands::batches::batch_items,
            commands::batches::review_batch_item,
            commands::batches::set_batch_paused,
            commands::batches::cancel_batch,
            commands::batches::retry_batch_item,
            commands::batches::retry_failed_batch,
            commands::batches::remove_batch,
            commands::assets::paste_asset,
            commands::assets::select_asset,
            commands::assets::cancel_task,
            commands::assets::retry_preview,
            commands::assets::release_preview,
            commands::assets::remove_asset,
            commands::assets::auto_cutout,
            commands::assets::switch_workflow,
            commands::assets::select_points,
            commands::assets::refine_mask,
            commands::assets::set_background,
            commands::assets::paint_mask,
            commands::assets::step_history,
            commands::assets::export_png,
            previews::preview_url,
            previews::batch_result_preview_url,
            shell_snapshot,
            shell_cancel,
            shell_retry,
            shell_open_result,
            shell_open_workbench
        ])
        .build(context)
        .expect("无法构建 ClearFrame Desktop")
        .run(|app, event| match event {
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => {
                let _ = app.state::<Arc<ShellTasks>>().show_main(app, false);
            }
            tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } => {
                api.prevent_exit();
                let workspace = app.state::<Arc<Workspace>>();
                let snapshot = workspace.snapshot();
                let active = snapshot.tasks.iter().any(|task| {
                    matches!(
                        task.state,
                        cutout_core::jobs::TaskState::Queued
                            | cutout_core::jobs::TaskState::Running
                            | cutout_core::jobs::TaskState::CancelRequested
                    )
                }) || snapshot
                    .batches
                    .iter()
                    .any(|batch| batch.queued + batch.running > 0)
                    || app.state::<Arc<ShellTasks>>().has_pending();
                if active {
                    let handle = app.clone();
                    app.dialog()
                        .message("仍有图片任务正在处理。确定退出并放弃未完成的任务吗？")
                        .title("退出 ClearFrame")
                        .buttons(MessageDialogButtons::OkCancelCustom(
                            "退出".into(),
                            "继续处理".into(),
                        ))
                        .show(move |quit| {
                            if quit {
                                finish_exit(handle);
                            }
                        });
                } else {
                    finish_exit(app.clone());
                }
            }
            _ => {}
        });
}
