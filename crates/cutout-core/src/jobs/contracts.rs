use crate::sam;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetState {
    Pending,
    Ready,
    Released,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    CancelRequested,
    Cancelled,
    Succeeded,
    Failed,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Workflow {
    Automatic,
    Selection,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackgroundChoice {
    Transparent,
    Solid { rgb: [u8; 3] },
    Image { path: PathBuf },
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackgroundView {
    Transparent,
    Solid {
        rgb: [u8; 3],
    },
    Image {
        #[serde(rename = "displayName")]
        display_name: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetSnapshot {
    pub asset_id: u64,
    pub workflow_id: u64,
    pub display_name: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub generation: u64,
    pub mask_revision: u64,
    pub result_revision: u64,
    pub background_revision: u64,
    pub background: BackgroundView,
    pub preview_revision: u64,
    pub preview_available: bool,
    pub result_available: bool,
    pub workflow: Workflow,
    pub selection_points: Vec<sam::Point>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub state: AssetState,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub task_id: u64,
    pub workflow_id: u64,
    pub asset_id: u64,
    pub generation: u64,
    pub selection_generation: u64,
    pub mask_revision: u64,
    pub kind: &'static str,
    pub state: TaskState,
    pub stage: &'static str,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub revision: u64,
    pub selected_asset_id: Option<u64>,
    pub selection_generation: u64,
    pub assets: Vec<AssetSnapshot>,
    pub tasks: Vec<TaskSnapshot>,
    pub batches: Vec<BatchSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSnapshot {
    pub batch_id: u64,
    pub output_directory: String,
    pub paused: bool,
    pub total: usize,
    pub queued: usize,
    pub running: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub cancelled: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemSnapshot {
    pub item_id: u64,
    pub batch_id: u64,
    pub display_name: String,
    pub output_name: String,
    pub state: TaskState,
    pub stage: &'static str,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchPage {
    pub total: usize,
    pub items: Vec<BatchItemSnapshot>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchFolderImport {
    pub batch_id: u64,
    pub added: usize,
    pub skipped: usize,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchFilter {
    #[default]
    All,
    Active,
    Completed,
    Issues,
}

impl BatchFilter {
    pub(super) fn includes(self, state: TaskState) -> bool {
        match self {
            Self::All => true,
            Self::Active => matches!(
                state,
                TaskState::Queued | TaskState::Running | TaskState::CancelRequested
            ),
            Self::Completed => state == TaskState::Succeeded,
            Self::Issues => matches!(
                state,
                TaskState::Failed | TaskState::Cancelled | TaskState::Stale
            ),
        }
    }
}

/// Volatile timing data for one run of a batch item. These values are not
/// checkpointed because a restored item starts a new run if retried.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemTimings {
    pub item_id: u64,
    pub prepare_us: Option<u64>,
    pub model_load_us: Option<u64>,
    pub inference_us: Option<u64>,
    pub compose_queue_us: Option<u64>,
    pub write_us: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub asset_id: u64,
    pub workflow_id: u64,
    pub task_id: u64,
    pub selection_generation: u64,
}
