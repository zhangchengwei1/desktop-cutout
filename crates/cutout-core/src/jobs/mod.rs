//! One Rust-owned scheduler. Domain modules share this state and memory budget.
mod assets;
mod batch;
mod checkpoint;
mod contracts;
mod editing;
mod pipeline;
mod processing;
#[cfg(test)]
mod tests;
mod worker;

use crate::{
    asset::{self, Preview},
    edit::{self, Stroke},
    image_io,
    inference::{CancelHandle, IsnetSession},
    mask, matte,
    model_store::{ModelLease, ModelStore},
    refinement::RefinementSession,
    sam,
    selection::SelectionSession,
    session::{
        BlobRef, SavedAsset, SavedBackground, SavedBatch, SavedBatchItem, SavedBatchOutput,
        SavedBatchSource, SavedBatchState, SavedEdits, SavedFileIdentity, SavedWorkflow,
        SessionCheckpoint, SessionStore,
    },
};
use batch::*;
use checkpoint::*;
pub use contracts::*;
use image::{GrayImage, RgbaImage};
use pipeline::*;
use processing::*;
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::Instant,
};
use worker::*;

const QUEUE_LIMIT: usize = 16;
const TASK_HISTORY_LIMIT: usize = 128;
const PREVIEW_BUDGET: usize = 32 * 1024 * 1024;
const MASK_BUDGET: usize = 64 * 1024 * 1024;
const EDIT_HISTORY_LIMIT: usize = 20;
const BATCH_INFLIGHT_BUDGET: u64 = 1024 * 1024 * 1024;
const BATCH_PREPARED_LIMIT: usize = 1;
const BATCH_WRITE_LIMIT: usize = 2;

type Processor = dyn Fn(&Path) -> Result<Preview, String> + Send + Sync;
type Listener = dyn Fn(u64) + Send + Sync;
#[cfg(test)]
type BatchInfer = dyn Fn(&RgbaImage) -> Result<GrayImage, String> + Send + Sync;

#[derive(Clone)]
enum BackgroundState {
    Transparent,
    Solid([u8; 3]),
    Image {
        path: PathBuf,
        display_name: String,
        preview: Arc<RgbaImage>,
    },
    ImagePending {
        path: PathBuf,
        display_name: String,
    },
}

impl BackgroundState {
    fn view(&self) -> BackgroundView {
        match self {
            Self::Transparent => BackgroundView::Transparent,
            Self::Solid(rgb) => BackgroundView::Solid { rgb: *rgb },
            Self::Image { display_name, .. } | Self::ImagePending { display_name, .. } => {
                BackgroundView::Image {
                    display_name: display_name.clone(),
                }
            }
        }
    }

    fn preview(&self) -> Option<mask::Background<'_>> {
        match self {
            // The WebView paints a solid preview background without re-encoding
            // the cutout. Export still composites the chosen color in Rust.
            Self::Transparent | Self::Solid(_) => None,
            Self::Image { preview, .. } => Some(mask::Background::Image(preview)),
            Self::ImagePending { .. } => None,
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Self::Image { preview, .. } => preview.as_raw().len(),
            Self::ImagePending { .. } => 0,
            _ => 0,
        }
    }
}

struct Asset {
    source: PathBuf,
    snapshot: AssetSnapshot,
    preview: Option<Arc<Vec<u8>>>,
    result_preview: Option<Arc<Vec<u8>>>,
    preview_access: u64,
    mask: Option<GrayImage>,
    undo: Vec<GrayImage>,
    redo: Vec<GrayImage>,
    inactive: EditState,
    background: BackgroundState,
}

#[derive(Default)]
struct EditState {
    mask: Option<GrayImage>,
    result_preview: Option<Arc<Vec<u8>>>,
    undo: Vec<GrayImage>,
    redo: Vec<GrayImage>,
}

struct CachedSelection {
    asset_id: u64,
    generation: u64,
    session: SelectionSession,
    _encoder: ModelLease,
    _decoder: ModelLease,
}

struct Batch {
    output_dir: PathBuf,
    paused: bool,
    items: Vec<u64>,
    imported_archives: Vec<tempfile::TempDir>,
}

struct BatchItem {
    source: PathBuf,
    origin: SavedBatchSource,
    destination: PathBuf,
    output: Option<SavedBatchOutput>,
    reserved_bytes: u64,
    timings: BatchItemTimings,
    snapshot: BatchItemSnapshot,
}

struct BatchInput {
    source: PathBuf,
    display_name: String,
    origin: SavedBatchSource,
}

enum TaskRequest {
    Automatic,
    Selection(Vec<sam::Point>),
    Refinement { expected_revision: u64, edge: u32 },
    Export(PathBuf),
}

impl TaskRequest {
    fn kind(&self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Selection(_) => "selection",
            Self::Refinement { .. } => "refinement",
            Self::Export(_) => "export",
        }
    }
}

struct Inner {
    revision: u64,
    next_id: u64,
    access_tick: u64,
    selected_asset_id: Option<u64>,
    selection_generation: u64,
    assets: BTreeMap<u64, Asset>,
    tasks: BTreeMap<u64, TaskSnapshot>,
    queue: VecDeque<u64>,
    batches: BTreeMap<u64, Batch>,
    batch_items: BTreeMap<u64, BatchItem>,
    batch_zip_exports: HashSet<u64>,
    batch_queue: VecDeque<u64>,
    batch_prepared: VecDeque<(u64, RgbaImage)>,
    batch_writes: VecDeque<(u64, RgbaImage)>,
    batch_inflight_bytes: u64,
    exports: BTreeMap<u64, PathBuf>,
    selections: BTreeMap<u64, Vec<sam::Point>>,
    refinements: BTreeMap<u64, u32>,
    decoded_source: Option<(u64, Arc<RgbaImage>)>,
    preview_bytes: usize,
    mask_bytes: usize,
    shutdown: bool,
}

impl Inner {
    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn changed(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            revision: self.revision,
            selected_asset_id: self.selected_asset_id,
            selection_generation: self.selection_generation,
            assets: self
                .assets
                .values()
                .map(|asset| asset.snapshot.clone())
                .collect(),
            tasks: self.tasks.values().cloned().collect(),
            batches: self
                .batches
                .iter()
                .map(|(batch_id, batch)| {
                    let mut summary = BatchSnapshot {
                        batch_id: *batch_id,
                        output_directory: batch.output_dir.to_string_lossy().into_owned(),
                        paused: batch.paused,
                        total: batch.items.len(),
                        queued: 0,
                        running: 0,
                        succeeded: 0,
                        failed: 0,
                        cancelled: 0,
                    };
                    for item_id in &batch.items {
                        match self.batch_items[item_id].snapshot.state {
                            TaskState::Queued => summary.queued += 1,
                            TaskState::Running | TaskState::CancelRequested => summary.running += 1,
                            TaskState::Succeeded => summary.succeeded += 1,
                            TaskState::Failed | TaskState::Stale => summary.failed += 1,
                            TaskState::Cancelled => summary.cancelled += 1,
                        }
                    }
                    summary
                })
                .collect(),
        }
    }

    fn trim_history(&mut self) {
        while self.tasks.len() > TASK_HISTORY_LIMIT {
            let Some(id) = self.tasks.iter().find_map(|(id, task)| {
                matches!(
                    task.state,
                    TaskState::Cancelled
                        | TaskState::Succeeded
                        | TaskState::Failed
                        | TaskState::Stale
                )
                .then_some(*id)
            }) else {
                break;
            };
            self.tasks.remove(&id);
            self.selections.remove(&id);
            self.refinements.remove(&id);
        }
    }

    fn cancel_asset_tasks(&mut self, asset_id: u64) {
        self.queue.retain(|task_id| {
            self.tasks
                .get(task_id)
                .is_none_or(|task| task.asset_id != asset_id)
        });
        for task in self
            .tasks
            .values_mut()
            .filter(|task| task.asset_id == asset_id)
        {
            match task.state {
                TaskState::Queued => {
                    task.state = TaskState::Cancelled;
                    task.stage = "cancelled";
                }
                TaskState::Running => {
                    task.state = TaskState::CancelRequested;
                    task.stage = "waiting_for_worker";
                }
                _ => {}
            }
        }
    }

    fn clear_preview(&mut self, asset_id: u64) {
        if self
            .decoded_source
            .as_ref()
            .is_some_and(|(id, _)| *id == asset_id)
        {
            self.decoded_source = None;
        }
        if let Some(asset) = self.assets.get_mut(&asset_id) {
            if let Some(bytes) = asset.preview.take() {
                self.preview_bytes -= bytes.len();
            }
            asset.snapshot.preview_available = false;
            asset.snapshot.preview_revision += 1;
        }
    }

    fn clear_result(&mut self, asset_id: u64) {
        if let Some(asset) = self.assets.get_mut(&asset_id) {
            if let Some(bytes) = asset.result_preview.take() {
                self.preview_bytes -= bytes.len();
            }
            asset.snapshot.result_available = false;
        }
    }

    fn clear_background(&mut self, asset_id: u64) {
        if let Some(asset) = self.assets.get_mut(&asset_id) {
            self.preview_bytes -= asset.background.bytes();
            asset.background = BackgroundState::Transparent;
            asset.snapshot.background = BackgroundView::Transparent;
            asset.snapshot.background_revision += 1;
            asset.snapshot.result_revision += 1;
        }
    }

    fn clear_edits(&mut self, asset_id: u64) {
        self.clear_background(asset_id);
        if let Some(asset) = self.assets.get_mut(&asset_id) {
            let history_bytes: usize = asset
                .undo
                .iter()
                .chain(asset.redo.iter())
                .map(|mask| mask.as_raw().len())
                .sum();
            let current_bytes = asset.mask.as_ref().map_or(0, |mask| mask.as_raw().len());
            let inactive_bytes = asset
                .inactive
                .mask
                .as_ref()
                .map_or(0, |mask| mask.as_raw().len())
                + asset
                    .inactive
                    .undo
                    .iter()
                    .chain(asset.inactive.redo.iter())
                    .map(|mask| mask.as_raw().len())
                    .sum::<usize>();
            self.mask_bytes -= history_bytes + current_bytes + inactive_bytes;
            if let Some(bytes) = asset.inactive.result_preview.as_ref() {
                self.preview_bytes -= bytes.len();
            }
            asset.mask = None;
            asset.undo.clear();
            asset.redo.clear();
            asset.inactive = EditState::default();
            asset.snapshot.selection_points.clear();
            asset.snapshot.can_undo = false;
            asset.snapshot.can_redo = false;
        }
    }

    fn evict_previews(&mut self, newest: u64) {
        while self.preview_bytes > PREVIEW_BUDGET {
            let oldest = self
                .assets
                .iter()
                .filter(|(id, asset)| {
                    **id != newest
                        && (asset.preview.is_some()
                            || asset.result_preview.is_some()
                            || asset.inactive.result_preview.is_some()
                            || asset.background.bytes() > 0)
                })
                .min_by_key(|(_, asset)| asset.preview_access)
                .map(|(id, _)| *id);
            let Some(oldest) = oldest else { break };
            self.clear_preview(oldest);
            self.clear_result(oldest);
            self.clear_background(oldest);
            if let Some(asset) = self.assets.get_mut(&oldest) {
                if let Some(bytes) = asset.inactive.result_preview.take() {
                    self.preview_bytes -= bytes.len();
                }
            }
            if let Some(asset) = self.assets.get_mut(&oldest) {
                asset.snapshot.state = AssetState::Released;
            }
        }
    }
}

struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
    listener: Box<Listener>,
    processor: Box<Processor>,
    inference: Option<(PathBuf, Arc<ModelStore>)>,
    active_inference: Mutex<Option<(u64, CancelHandle)>>,
    automatic_preparation: Mutex<Option<(u64, CancelHandle)>>,
    automatic_preparation_epoch: AtomicU64,
    #[cfg(test)]
    batch_infer: Mutex<Option<Box<BatchInfer>>>,
}

impl Shared {
    fn interrupt_automatic_preparation(&self) {
        self.automatic_preparation_epoch
            .fetch_add(1, Ordering::AcqRel);
        if let Some((_, handle)) = self.automatic_preparation.lock().unwrap().as_ref() {
            let _ = handle.cancel();
        }
    }

    fn interrupt_asset(&self, asset_id: u64) {
        self.interrupt_automatic_preparation();
        let active = self.active_inference.lock().unwrap().clone();
        if let Some((task_id, handle)) = active {
            let belongs = self
                .inner
                .lock()
                .unwrap()
                .tasks
                .get(&task_id)
                .is_some_and(|task| task.asset_id == asset_id);
            if belongs {
                let _ = handle.cancel();
            }
        }
    }
}

/// All asset and task state belongs to this Rust object. Clone only the
/// snapshots and small encoded preview references needed by the caller.
pub struct Workspace {
    shared: Arc<Shared>,
    clipboard_dir: tempfile::TempDir,
}

struct BatchZipLease<'a> {
    shared: &'a Shared,
    batch_id: u64,
}

impl Drop for BatchZipLease<'_> {
    fn drop(&mut self) {
        self.shared
            .inner
            .lock()
            .unwrap()
            .batch_zip_exports
            .remove(&self.batch_id);
    }
}

fn edit_bytes(edits: &EditState) -> usize {
    edits.mask.as_ref().map_or(0, |mask| mask.as_raw().len())
        + edits
            .undo
            .iter()
            .map(|mask| mask.as_raw().len())
            .sum::<usize>()
        + edits
            .redo
            .iter()
            .map(|mask| mask.as_raw().len())
            .sum::<usize>()
}

impl Workspace {
    pub fn new(listener: impl Fn(u64) + Send + Sync + 'static) -> Result<Self, String> {
        Self::with_processor_and_inference(
            listener,
            |path| asset::build_preview(path).map_err(|error| error.to_string()),
            None,
        )
    }

    pub fn new_with_inference(
        listener: impl Fn(u64) + Send + Sync + 'static,
        runtime_path: PathBuf,
        model_store: Arc<ModelStore>,
    ) -> Result<Self, String> {
        Self::with_processor_and_inference(
            listener,
            |path| asset::build_preview(path).map_err(|error| error.to_string()),
            Some((runtime_path, model_store)),
        )
    }

    #[cfg(test)]
    fn with_processor(
        listener: impl Fn(u64) + Send + Sync + 'static,
        processor: impl Fn(&Path) -> Result<Preview, String> + Send + Sync + 'static,
    ) -> Result<Self, String> {
        Self::with_processor_and_inference(listener, processor, None)
    }

    fn with_processor_and_inference(
        listener: impl Fn(u64) + Send + Sync + 'static,
        processor: impl Fn(&Path) -> Result<Preview, String> + Send + Sync + 'static,
        inference: Option<(PathBuf, Arc<ModelStore>)>,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                revision: 0,
                next_id: 1,
                access_tick: 0,
                selected_asset_id: None,
                selection_generation: 0,
                assets: BTreeMap::new(),
                tasks: BTreeMap::new(),
                queue: VecDeque::new(),
                batches: BTreeMap::new(),
                batch_items: BTreeMap::new(),
                batch_zip_exports: HashSet::new(),
                batch_queue: VecDeque::new(),
                batch_prepared: VecDeque::new(),
                batch_writes: VecDeque::new(),
                batch_inflight_bytes: 0,
                exports: BTreeMap::new(),
                selections: BTreeMap::new(),
                refinements: BTreeMap::new(),
                decoded_source: None,
                preview_bytes: 0,
                mask_bytes: 0,
                shutdown: false,
            }),
            wake: Condvar::new(),
            listener: Box::new(listener),
            processor: Box::new(processor),
            inference,
            active_inference: Mutex::new(None),
            automatic_preparation: Mutex::new(None),
            automatic_preparation_epoch: AtomicU64::new(0),
            #[cfg(test)]
            batch_infer: Mutex::new(None),
        });
        let clipboard_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let started = (|| -> Result<(), String> {
            let worker = Arc::clone(&shared);
            thread::Builder::new()
                .name("clearframe-asset-worker".to_owned())
                .spawn(move || run_worker(worker))
                .map_err(|error| error.to_string())?;
            let decoder = Arc::clone(&shared);
            thread::Builder::new()
                .name("clearframe-batch-decoder".to_owned())
                .spawn(move || run_batch_decoder(decoder))
                .map_err(|error| error.to_string())?;
            let writer = Arc::clone(&shared);
            thread::Builder::new()
                .name("clearframe-batch-writer".to_owned())
                .spawn(move || run_batch_writer(writer))
                .map_err(|error| error.to_string())?;
            Ok(())
        })();
        if let Err(error) = started {
            shared.inner.lock().unwrap().shutdown = true;
            shared.wake.notify_all();
            return Err(error);
        }
        Ok(Self {
            shared,
            clipboard_dir,
        })
    }

    pub fn snapshot(&self) -> WorkspaceSnapshot {
        self.shared.inner.lock().unwrap().snapshot()
    }

    pub fn export_png(&self, asset_id: u64, destination: PathBuf) -> Result<u64, String> {
        if destination
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|value| !value.eq_ignore_ascii_case("png"))
        {
            return Err("输出文件必须是 PNG".to_owned());
        }
        self.enqueue(asset_id, TaskRequest::Export(destination))
    }

    fn enqueue(&self, asset_id: u64, request: TaskRequest) -> Result<u64, String> {
        let is_automatic = matches!(request, TaskRequest::Automatic);
        let (task_id, revision) = {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.queue.len() >= QUEUE_LIMIT {
                return Err("待处理任务已满，请稍后重试".to_owned());
            }
            if inner.selected_asset_id != Some(asset_id) {
                return Err("请先选中图片".to_owned());
            }
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if asset.snapshot.state != AssetState::Ready || asset.preview.is_none() {
                return Err("图片预览尚未就绪".to_owned());
            }
            if matches!(
                request,
                TaskRequest::Export(_) | TaskRequest::Refinement { .. }
            ) && asset.mask.is_none()
            {
                return Err("请先完成抠图".to_owned());
            }
            if (matches!(request, TaskRequest::Automatic)
                && asset.snapshot.workflow != Workflow::Automatic)
                || (matches!(request, TaskRequest::Selection(_))
                    && asset.snapshot.workflow != Workflow::Selection)
            {
                return Err("工作流已切换".to_owned());
            }
            if let TaskRequest::Refinement {
                expected_revision, ..
            } = request
            {
                if asset.snapshot.mask_revision != expected_revision {
                    return Err("图片已更新，请重试精修".to_owned());
                }
            }
            let kind = request.kind();
            let workflow_id = asset.snapshot.workflow_id;
            let generation = asset.snapshot.generation;
            let mask_revision = asset.snapshot.mask_revision;
            let selection_generation = inner.selection_generation;
            let task_id = inner.next_id();
            inner.tasks.insert(
                task_id,
                TaskSnapshot {
                    task_id,
                    workflow_id,
                    asset_id,
                    generation,
                    selection_generation,
                    mask_revision,
                    kind,
                    state: TaskState::Queued,
                    stage: "queued",
                    error: None,
                },
            );
            match request {
                TaskRequest::Export(destination) => {
                    inner.exports.insert(task_id, destination);
                }
                TaskRequest::Selection(points) => {
                    inner.selections.insert(task_id, points);
                }
                TaskRequest::Refinement { edge, .. } => {
                    inner.refinements.insert(task_id, edge);
                }
                TaskRequest::Automatic => {}
            }
            inner.queue.push_back(task_id);
            inner.trim_history();
            (task_id, inner.changed())
        };
        if !is_automatic {
            self.shared.interrupt_automatic_preparation();
        }
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(task_id)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let mut inner = self.shared.inner.lock().unwrap();
        inner.shutdown = true;
        drop(inner);
        self.shared.interrupt_automatic_preparation();
        self.shared.wake.notify_all();
    }
}
