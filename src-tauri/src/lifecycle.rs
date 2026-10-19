use cutout_core::recovery::RecoveryController;
use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

pub(crate) fn finish_exit(handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let recovery = Arc::clone(&handle.state::<Arc<RecoveryController>>());
        let result = tauri::async_runtime::spawn_blocking(move || recovery.save_if_enabled()).await;
        match result {
            Ok(Ok(())) => handle.exit(0),
            other => {
                let reason = match other {
                    Ok(Err(error)) => error,
                    Err(error) => error.to_string(),
                    Ok(Ok(())) => unreachable!(),
                };
                let exiting = handle.clone();
                handle
                    .dialog()
                    .message(format!("会话未能保存：{reason}\n仍要退出吗？"))
                    .title("退出 ClearFrame")
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "退出".into(),
                        "返回工作台".into(),
                    ))
                    .show(move |quit| {
                        if quit {
                            exiting.exit(0);
                        }
                    });
            }
        }
    });
}
