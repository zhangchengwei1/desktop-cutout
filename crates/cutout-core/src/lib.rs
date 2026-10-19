use serde::Serialize;

mod asset;
pub mod batch_folder;
mod batch_zip;
pub mod edit;
pub mod image_io;
pub mod inference;
pub mod isnet;
pub mod jobs;
pub mod mask;
pub mod matte;
pub mod model;
pub mod model_store;
mod parallel;
pub mod preferences;
pub mod recovery;
pub mod refinement;
pub mod runtime;
pub mod sam;
pub mod selection;
pub mod session;
pub mod shortcuts;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub core_version: String,
    pub inference_ready: bool,
}

/// 桌面应用尚未配置模型缓存与原生库；CLI 能推理不代表工作台就绪。
pub fn runtime_info() -> RuntimeInfo {
    RuntimeInfo {
        core_version: env!("CARGO_PKG_VERSION").to_owned(),
        inference_ready: false,
    }
}

pub mod shell;
