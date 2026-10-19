pub(crate) fn main_window(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("仅主窗口可访问工作区".to_owned())
    }
}

pub(crate) fn settings_access(window: &tauri::WebviewWindow) -> Result<(), String> {
    if matches!(window.label(), "main" | "settings") {
        Ok(())
    } else {
        Err("此窗口不可访问设置".to_owned())
    }
}

pub(crate) fn preferences_read_access(window: &tauri::WebviewWindow) -> Result<(), String> {
    if matches!(
        window.label(),
        "main" | "settings" | "resources" | "help" | "about"
    ) {
        Ok(())
    } else {
        Err("此窗口不可读取偏好设置".to_owned())
    }
}
pub(crate) fn model_access(window: &tauri::WebviewWindow) -> Result<(), String> {
    if matches!(window.label(), "main" | "resources") {
        Ok(())
    } else {
        Err("此窗口不可管理模型".to_owned())
    }
}
