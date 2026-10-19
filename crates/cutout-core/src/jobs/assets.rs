use super::*;

impl Workspace {
    pub fn import(&self, path: &Path) -> Result<ImportResult, String> {
        let source = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
        if !source.is_file() {
            return Err("请选择图片文件".to_owned());
        }
        let display_name = source
            .file_name()
            .ok_or("文件名无法显示")?
            .to_string_lossy()
            .into_owned();
        let (result, revision) = {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.queue.len() >= QUEUE_LIMIT {
                return Err("待处理任务已满，请稍后重试".to_owned());
            }
            let asset_id = inner.next_id();
            let workflow_id = inner.next_id();
            let task_id = inner.next_id();
            let generation = 1;
            inner.selected_asset_id = Some(asset_id);
            inner.decoded_source = None;
            inner.selection_generation += 1;
            let selection_generation = inner.selection_generation;
            inner.assets.insert(
                asset_id,
                Asset {
                    source,
                    snapshot: AssetSnapshot {
                        asset_id,
                        workflow_id,
                        display_name,
                        width: None,
                        height: None,
                        generation,
                        mask_revision: 0,
                        result_revision: 0,
                        background_revision: 0,
                        background: BackgroundView::Transparent,
                        preview_revision: 0,
                        preview_available: false,
                        result_available: false,
                        workflow: Workflow::Automatic,
                        selection_points: Vec::new(),
                        can_undo: false,
                        can_redo: false,
                        state: AssetState::Pending,
                        error: None,
                    },
                    preview: None,
                    result_preview: None,
                    preview_access: 0,
                    mask: None,
                    undo: Vec::new(),
                    redo: Vec::new(),
                    inactive: EditState::default(),
                    background: BackgroundState::Transparent,
                },
            );
            inner.tasks.insert(
                task_id,
                TaskSnapshot {
                    task_id,
                    workflow_id,
                    asset_id,
                    generation,
                    selection_generation,
                    mask_revision: 0,
                    kind: "preview",
                    state: TaskState::Queued,
                    stage: "queued",
                    error: None,
                },
            );
            inner.queue.push_back(task_id);
            inner.trim_history();
            let result = ImportResult {
                asset_id,
                workflow_id,
                task_id,
                selection_generation,
            };
            (result, inner.changed())
        };
        self.shared.interrupt_automatic_preparation();
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(result)
    }

    /// Native clipboard pixels are encoded to a session-scoped file, then
    /// enter the exact same asset/preview queue as chosen or dropped files.
    pub fn import_clipboard_image(
        &self,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<ImportResult, String> {
        let count = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or("剪贴板图片尺寸无效")?;
        if width == 0 || height == 0 || width > 20_000 || height > 20_000 || count > 50_000_000 {
            return Err("剪贴板图片超过尺寸预算".to_owned());
        }
        let image = RgbaImage::from_raw(width, height, pixels).ok_or("剪贴板 RGBA 数据长度无效")?;
        let mut temporary = tempfile::Builder::new()
            .prefix("clipboard-image-")
            .suffix(".png")
            .tempfile_in(self.clipboard_dir.path())
            .map_err(|error| error.to_string())?;
        image_io::encode_png(&image, temporary.as_file_mut()).map_err(|error| error.to_string())?;
        if temporary
            .as_file()
            .metadata()
            .map_err(|error| error.to_string())?
            .len()
            > 200 * 1024 * 1024
        {
            return Err("剪贴板图片超过文件预算".to_owned());
        }
        let path = temporary.path().to_path_buf();
        let result = self.import(&path);
        if result.is_ok() {
            let _ = temporary.keep();
        }
        result
    }

    pub fn select(&self, asset_id: u64) -> Result<u64, String> {
        let (selection_generation, revision, old_selection) = {
            let mut inner = self.shared.inner.lock().unwrap();
            if !inner.assets.contains_key(&asset_id) {
                return Err("资产不存在".to_owned());
            }
            if inner.selected_asset_id == Some(asset_id) {
                return Ok(inner.selection_generation);
            }
            let old_selection = inner.selected_asset_id;
            inner.selected_asset_id = Some(asset_id);
            inner.decoded_source = None;
            inner.selection_generation += 1;
            (inner.selection_generation, inner.changed(), old_selection)
        };
        if let Some(old) = old_selection {
            self.shared.interrupt_asset(old);
        }
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(selection_generation)
    }

    pub fn switch_workflow(&self, asset_id: u64, workflow: Workflow) -> Result<(), String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.selected_asset_id != Some(asset_id) {
                return Err("请先选中图片".to_owned());
            }
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if asset.snapshot.workflow == workflow {
                return Ok(());
            }
            inner.cancel_asset_tasks(asset_id);
            let asset = inner.assets.get_mut(&asset_id).unwrap();
            std::mem::swap(&mut asset.mask, &mut asset.inactive.mask);
            std::mem::swap(
                &mut asset.result_preview,
                &mut asset.inactive.result_preview,
            );
            std::mem::swap(&mut asset.undo, &mut asset.inactive.undo);
            std::mem::swap(&mut asset.redo, &mut asset.inactive.redo);
            asset.snapshot.workflow = workflow;
            asset.snapshot.mask_revision += 1;
            asset.snapshot.result_available = asset.result_preview.is_some();
            asset.snapshot.result_revision += 1;
            asset.snapshot.can_undo = !asset.undo.is_empty();
            asset.snapshot.can_redo = !asset.redo.is_empty();
            inner.changed()
        };
        self.shared.interrupt_asset(asset_id);
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(())
    }

    pub fn cancel(&self, task_id: u64) -> Result<(), String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let task = inner.tasks.get_mut(&task_id).ok_or("任务不存在")?;
            let asset_id = task.asset_id;
            match task.state {
                TaskState::Queued => {
                    task.state = TaskState::Cancelled;
                    task.stage = "cancelled";
                    inner.queue.retain(|id| *id != task_id);
                }
                TaskState::Running => {
                    task.state = TaskState::CancelRequested;
                    task.stage = "waiting_for_worker";
                }
                TaskState::CancelRequested => return Ok(()),
                _ => return Err("任务已结束".to_owned()),
            }
            if let Some(asset) = inner.assets.get_mut(&asset_id) {
                if asset.preview.is_none() {
                    asset.snapshot.state = AssetState::Released;
                }
            }
            inner.changed()
        };
        self.shared.interrupt_automatic_preparation();
        if let Some((id, handle)) = self.shared.active_inference.lock().unwrap().as_ref() {
            if *id == task_id {
                let _ = handle.cancel();
            }
        }
        (self.shared.listener)(revision);
        Ok(())
    }

    pub fn retry_preview(&self, asset_id: u64) -> Result<u64, String> {
        let (task_id, revision) = {
            let mut inner = self.shared.inner.lock().unwrap();
            if !inner.assets.contains_key(&asset_id) {
                return Err("资产不存在".to_owned());
            }
            let other_queued = inner
                .queue
                .iter()
                .filter(|id| {
                    inner
                        .tasks
                        .get(id)
                        .is_some_and(|task| task.asset_id != asset_id)
                })
                .count();
            if other_queued >= QUEUE_LIMIT {
                return Err("待处理任务已满，请稍后重试".to_owned());
            }
            inner.cancel_asset_tasks(asset_id);
            inner.clear_preview(asset_id);
            inner.clear_result(asset_id);
            inner.clear_edits(asset_id);
            let task_id = inner.next_id();
            let asset = inner.assets.get_mut(&asset_id).unwrap();
            asset.snapshot.generation += 1;
            asset.snapshot.state = AssetState::Pending;
            asset.snapshot.error = None;
            let workflow_id = asset.snapshot.workflow_id;
            let generation = asset.snapshot.generation;
            let mask_revision = asset.snapshot.mask_revision;
            let selection_generation = inner.selection_generation;
            inner.tasks.insert(
                task_id,
                TaskSnapshot {
                    task_id,
                    workflow_id,
                    asset_id,
                    generation,
                    selection_generation,
                    mask_revision,
                    kind: "preview",
                    state: TaskState::Queued,
                    stage: "queued",
                    error: None,
                },
            );
            inner.queue.push_back(task_id);
            inner.trim_history();
            (task_id, inner.changed())
        };
        self.shared.interrupt_asset(asset_id);
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(task_id)
    }

    pub fn release_preview(&self, asset_id: u64) -> Result<(), String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            if !inner.assets.contains_key(&asset_id) {
                return Err("资产不存在".to_owned());
            }
            inner.cancel_asset_tasks(asset_id);
            inner.clear_preview(asset_id);
            inner.clear_result(asset_id);
            inner.clear_edits(asset_id);
            let asset = inner.assets.get_mut(&asset_id).unwrap();
            asset.snapshot.generation += 1;
            asset.snapshot.state = AssetState::Released;
            asset.snapshot.error = None;
            inner.changed()
        };
        self.shared.interrupt_asset(asset_id);
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(())
    }

    pub fn remove(&self, asset_id: u64) -> Result<(), String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            if !inner.assets.contains_key(&asset_id) {
                return Err("资产不存在".to_owned());
            }
            inner.cancel_asset_tasks(asset_id);
            inner.clear_preview(asset_id);
            inner.clear_result(asset_id);
            inner.clear_edits(asset_id);
            inner.assets.remove(&asset_id).unwrap();
            if inner.selected_asset_id == Some(asset_id) {
                inner.selected_asset_id = None;
                inner.selection_generation += 1;
            }
            inner.changed()
        };
        self.shared.interrupt_asset(asset_id);
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(())
    }
}
