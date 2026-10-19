use crate::commands::access::main_window;
use cutout_core::jobs::Workspace;
use std::sync::Arc;
use tauri::http;

#[tauri::command]
pub(crate) fn batch_result_preview_url(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    item_id: u64,
) -> Result<String, String> {
    main_window(&window)?;
    if !workspace.batch_result_available(item_id) {
        return Err("此图片尚未保存处理结果".to_owned());
    }
    Ok(if cfg!(target_os = "windows") {
        format!("http://clearframe-preview.localhost/batch/{item_id}/0")
    } else {
        format!("clearframe-preview://localhost/batch/{item_id}/0")
    })
}

#[tauri::command]
pub(crate) fn preview_url(
    window: tauri::WebviewWindow,
    workspace: tauri::State<'_, Arc<Workspace>>,
    asset_id: u64,
    result: bool,
) -> Result<String, String> {
    main_window(&window)?;
    let revision = if result {
        workspace.result_revision(asset_id)
    } else {
        workspace.preview_revision(asset_id)
    }
    .ok_or("预览不存在或已释放")?;
    let kind = if result { "result" } else { "original" };
    if cfg!(target_os = "windows") {
        Ok(format!(
            "http://clearframe-preview.localhost/{kind}/{asset_id}/{revision}"
        ))
    } else {
        Ok(format!(
            "clearframe-preview://localhost/{kind}/{asset_id}/{revision}"
        ))
    }
}

pub(crate) fn preview_response(
    main_webview: bool,
    workspace: &Workspace,
    request: http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    let not_found = || {
        http::Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .header(http::header::CACHE_CONTROL, "no-store")
            .body(Vec::new())
            .unwrap()
    };
    if !main_webview || request.method() != http::Method::GET {
        return not_found();
    }
    let mut parts = request.uri().path().trim_start_matches('/').split('/');
    let parsed = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(kind @ ("original" | "result" | "batch")), Some(asset_id), Some(revision), None) => {
            asset_id
                .parse::<u64>()
                .ok()
                .zip(revision.parse::<u64>().ok())
                .map(|(id, rev)| (kind, id, rev))
        }
        _ => None,
    };
    let Some((kind, asset_id, revision)) = parsed else {
        return not_found();
    };
    let bytes = match kind {
        "batch" if revision == 0 => workspace.batch_result_preview_png(asset_id).ok(),
        "result" => workspace
            .result_preview_png(asset_id, revision)
            .map(|bytes| bytes.as_ref().clone()),
        "original" => workspace
            .preview_png(asset_id, revision)
            .map(|bytes| bytes.as_ref().clone()),
        _ => None,
    };
    let Some(bytes) = bytes else {
        return not_found();
    };
    http::Response::builder()
        .status(http::StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "image/png")
        .header(http::header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(bytes)
        .unwrap()
}
