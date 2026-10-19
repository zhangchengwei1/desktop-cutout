//! Shell setup stays separate from model, UI session and image processing.
#[cfg(target_os = "macos")]
pub fn configure(application: &std::path::Path) -> Result<i32, String> {
    let helper = application.join("Contents/Resources/context-menu/Context Menu Setup.app/Contents/MacOS/clearframe-context-setup");
    let status = std::process::Command::new(helper)
        .arg("--application")
        .arg(application)
        .status()
        .map_err(|error| error.to_string())?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(target_os = "macos")]
pub fn offer_after_install() {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(application) = executable
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
    else {
        return;
    };
    let in_applications = application.parent().is_some_and(|parent| {
        parent == std::path::Path::new("/Applications")
            || dirs::home_dir().is_some_and(|home| parent == home.join("Applications"))
    });
    let Ok(data) = cutout_core::shell::application_data() else {
        return;
    };
    if !in_applications || data.join("context-menu-choice.json").exists() {
        return;
    }
    let application = application.to_owned();
    std::thread::spawn(move || {
        if let Err(error) = configure(&application) {
            eprintln!("Context menu setup failed: {error}");
        }
    });
}

#[cfg(target_os = "windows")]
pub fn report_failure(reason: &str) {
    use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};
    let saved_language = cutout_core::shell::application_data()
        .ok()
        .and_then(|data| {
            let bytes = std::fs::read(data.join("preferences.json")).ok()?;
            let preferences: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            preferences.get("language")?.as_str().map(str::to_owned)
        });
    let system_language = unsafe { GetUserDefaultUILanguage() } & 0x3ff;
    let locale = match saved_language.as_deref() {
        Some("zh") => "zh",
        Some("ja") => "ja",
        Some("en") => "en",
        _ if system_language == 4 => "zh",
        _ if system_language == 17 => "ja",
        _ => "en",
    };
    let resource = match locale {
        "zh" => include_str!("../installer/context/locales/zh.json"),
        "ja" => include_str!("../installer/context/locales/ja.json"),
        _ => include_str!("../installer/context/locales/en.json"),
    };
    let messages: serde_json::Value = serde_json::from_str(resource).expect("bundled locale JSON");
    let text: Vec<u16> = format!(
        "{}\n\n{reason}",
        messages["failure"].as_str().unwrap_or_default()
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let title: Vec<u16> = "ClearFrame Desktop".encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONWARNING,
        );
    }
}
