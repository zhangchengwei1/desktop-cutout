use super::*;

impl Workspace {
    /// Batch admission stores only canonical source paths and reserved output
    /// names. Image bytes enter memory when the shared worker reaches an item.
    pub fn create_batch(&self, output_dir: &Path, paths: &[PathBuf]) -> Result<u64, String> {
        self.create_batch_sources(output_dir, canonical_batch_sources(paths)?, None, false)
    }

    pub fn create_paused_batch(&self, output_dir: &Path, paths: &[PathBuf]) -> Result<u64, String> {
        self.create_batch_sources(output_dir, canonical_batch_sources(paths)?, None, true)
    }

    pub fn create_batch_from_folder(
        &self,
        output: &Path,
        folder: &Path,
    ) -> Result<BatchFolderImport, String> {
        let found = crate::batch_folder::scan(folder, output)?;
        let batch_id = self.create_batch(output, &found.paths)?;
        Ok(BatchFolderImport {
            batch_id,
            added: found.paths.len(),
            skipped: found.skipped,
        })
    }

    pub fn append_batch_folder(
        &self,
        batch_id: u64,
        folder: &Path,
    ) -> Result<BatchFolderImport, String> {
        let output = self
            .shared
            .inner
            .lock()
            .unwrap()
            .batches
            .get(&batch_id)
            .ok_or("批次不存在")?
            .output_dir
            .clone();
        let found = crate::batch_folder::scan(folder, &output)?;
        self.append_batch(batch_id, &found.paths)?;
        Ok(BatchFolderImport {
            batch_id,
            added: found.paths.len(),
            skipped: found.skipped,
        })
    }

    pub fn create_batch_from_zip(&self, output_dir: &Path, archive: &Path) -> Result<u64, String> {
        let archive = std::fs::canonicalize(archive).map_err(|error| error.to_string())?;
        let imported = crate::batch_zip::import_images(&archive)?;
        self.create_batch_sources(
            output_dir,
            archive_batch_inputs(&archive, imported.sources),
            Some(imported.directory),
            false,
        )
    }

    fn create_batch_sources(
        &self,
        output_dir: &Path,
        sources: Vec<BatchInput>,
        imported_archive: Option<tempfile::TempDir>,
        paused: bool,
    ) -> Result<u64, String> {
        let output_dir = std::fs::canonicalize(output_dir)
            .map_err(|error| format!("[output_unavailable] {error}"))?;
        if !output_dir.is_dir() {
            return Err("[output_unavailable] 请选择输出文件夹".to_owned());
        }
        // Check write access before accepting jobs; never create an empty batch
        // that would fail every image because the remembered folder is unusable.
        let probe = tempfile::NamedTempFile::new_in(&output_dir)
            .map_err(|error| format!("[output_unavailable] {error}"))?;
        drop(probe);
        let (batch_id, revision) = {
            let mut inner = self.shared.inner.lock().unwrap();
            let batch_id = inner.next_id();
            inner.batches.insert(
                batch_id,
                Batch {
                    output_dir,
                    paused,
                    items: Vec::new(),
                    imported_archives: imported_archive.into_iter().collect(),
                },
            );
            insert_batch_items(&mut inner, batch_id, sources)?;
            (batch_id, inner.changed())
        };
        self.shared.interrupt_automatic_preparation();
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(batch_id)
    }

    pub fn append_batch(&self, batch_id: u64, paths: &[PathBuf]) -> Result<usize, String> {
        self.append_batch_sources(batch_id, canonical_batch_sources(paths)?, None)
    }

    pub fn append_batch_zip(&self, batch_id: u64, archive: &Path) -> Result<usize, String> {
        let archive = std::fs::canonicalize(archive).map_err(|error| error.to_string())?;
        let imported = crate::batch_zip::import_images(&archive)?;
        self.append_batch_sources(
            batch_id,
            archive_batch_inputs(&archive, imported.sources),
            Some(imported.directory),
        )
    }

    fn append_batch_sources(
        &self,
        batch_id: u64,
        sources: Vec<BatchInput>,
        imported_archive: Option<tempfile::TempDir>,
    ) -> Result<usize, String> {
        let (total, revision) = {
            let mut inner = self.shared.inner.lock().unwrap();
            if !inner.batches.contains_key(&batch_id) {
                return Err("批次不存在".to_owned());
            }
            insert_batch_items(&mut inner, batch_id, sources)?;
            if let Some(directory) = imported_archive {
                inner
                    .batches
                    .get_mut(&batch_id)
                    .unwrap()
                    .imported_archives
                    .push(directory);
            }
            (inner.batches[&batch_id].items.len(), inner.changed())
        };
        self.shared.interrupt_automatic_preparation();
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(total)
    }

    pub fn export_batch_zip(&self, batch_id: u64, destination: &Path) -> Result<usize, String> {
        let items = {
            let mut inner = self.shared.inner.lock().unwrap();
            let batch = inner.batches.get(&batch_id).ok_or("批次不存在")?;
            if batch.paused
                && batch.items.iter().any(|id| {
                    matches!(
                        inner.batch_items[id].snapshot.state,
                        TaskState::Queued | TaskState::Running | TaskState::CancelRequested
                    )
                })
            {
                return Err("请先继续处理批次，再打包 ZIP".to_owned());
            }
            let items = batch.items.clone();
            if !inner.batch_zip_exports.insert(batch_id) {
                return Err("此批次正在打包 ZIP".to_owned());
            }
            items
        };
        let _lease = BatchZipLease {
            shared: &self.shared,
            batch_id,
        };
        crate::batch_zip::export_results_streaming(destination, items.len(), |index| {
            let item_id = items[index];
            let mut inner = self.shared.inner.lock().unwrap();
            loop {
                if inner.shutdown {
                    return Err("工作区已关闭，ZIP 打包已停止".to_owned());
                }
                let item = inner.batch_items.get(&item_id).ok_or("批量项不存在")?;
                match item.snapshot.state {
                    TaskState::Succeeded => {
                        return Ok(Some((
                            item.snapshot.output_name.clone(),
                            item.destination.clone(),
                            item.output.clone(),
                        )));
                    }
                    TaskState::Failed | TaskState::Cancelled | TaskState::Stale => return Ok(None),
                    TaskState::Queued | TaskState::Running | TaskState::CancelRequested => {
                        if inner
                            .batches
                            .get(&batch_id)
                            .is_some_and(|batch| batch.paused)
                        {
                            return Err("批次已暂停，ZIP 打包已停止".to_owned());
                        }
                        inner = self.shared.wake.wait(inner).unwrap();
                    }
                }
            }
        })
    }

    pub fn batch_items(
        &self,
        batch_id: u64,
        offset: usize,
        limit: usize,
    ) -> Result<BatchPage, String> {
        self.filtered_batch_items(batch_id, offset, limit, BatchFilter::All)
    }

    pub fn filtered_batch_items(
        &self,
        batch_id: u64,
        offset: usize,
        limit: usize,
        filter: BatchFilter,
    ) -> Result<BatchPage, String> {
        if limit == 0 || limit > 200 {
            return Err("每页应包含 1–200 项".to_owned());
        }
        let inner = self.shared.inner.lock().unwrap();
        let batch = inner.batches.get(&batch_id).ok_or("批次不存在")?;
        let matches = || {
            batch
                .items
                .iter()
                .map(|id| &inner.batch_items[id].snapshot)
                .filter(|item| filter.includes(item.state))
        };
        Ok(BatchPage {
            total: matches().count(),
            items: matches().skip(offset).take(limit).cloned().collect(),
        })
    }

    /// Import the original for manual review, retaining the batch and its PNGs.
    pub fn review_batch_item(&self, item_id: u64) -> Result<ImportResult, String> {
        let source = {
            let inner = self.shared.inner.lock().unwrap();
            inner
                .batch_items
                .get(&item_id)
                .ok_or("批量项不存在")?
                .source
                .clone()
        };
        self.import(&source)
    }

    pub fn batch_result_available(&self, item_id: u64) -> bool {
        self.shared
            .inner
            .lock()
            .unwrap()
            .batch_items
            .get(&item_id)
            .is_some_and(|item| item.snapshot.state == TaskState::Succeeded)
    }

    /// Read only the committed result, returning a bounded PNG via the preview
    /// protocol. The original and interactive workspace remain unchanged.
    pub fn batch_result_preview_png(&self, item_id: u64) -> Result<Vec<u8>, String> {
        let (path, output) = {
            let inner = self.shared.inner.lock().unwrap();
            let item = inner.batch_items.get(&item_id).ok_or("批量项不存在")?;
            if item.snapshot.state != TaskState::Succeeded {
                return Err("此图片尚未保存处理结果".to_owned());
            }
            (item.destination.clone(), item.output.clone())
        };
        if !committed_batch_output_exists(&path, output.as_ref()) {
            return Err("已保存的结果不存在或已被修改".to_owned());
        }
        let preview = asset::build_preview(&path).map_err(|error| error.to_string())?;
        if !self.batch_result_available(item_id) {
            return Err("批量结果已移除".to_owned());
        }
        Ok(preview.png)
    }

    pub fn batch_timings(&self, batch_id: u64) -> Result<Vec<BatchItemTimings>, String> {
        let inner = self.shared.inner.lock().unwrap();
        let batch = inner.batches.get(&batch_id).ok_or("批次不存在")?;
        Ok(batch
            .items
            .iter()
            .map(|id| inner.batch_items[id].timings.clone())
            .collect())
    }

    pub fn set_batch_paused(&self, batch_id: u64, paused: bool) -> Result<(), String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let batch = inner.batches.get_mut(&batch_id).ok_or("批次不存在")?;
            if batch.paused == paused {
                return Ok(());
            }
            batch.paused = paused;
            inner.changed()
        };
        self.shared.interrupt_automatic_preparation();
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(())
    }

    pub fn cancel_batch(&self, batch_id: u64) -> Result<(), String> {
        let (ids, discarded, revision) = {
            let mut inner = self.shared.inner.lock().unwrap();
            let ids: HashSet<u64> = inner
                .batches
                .get(&batch_id)
                .ok_or("批次不存在")?
                .items
                .iter()
                .copied()
                .collect();
            for id in &ids {
                let item = &mut inner.batch_items.get_mut(id).unwrap().snapshot;
                match item.state {
                    TaskState::Queued => {
                        item.state = TaskState::Cancelled;
                        item.stage = "cancelled";
                    }
                    TaskState::Running => {
                        item.state = TaskState::CancelRequested;
                        item.stage = "waiting_for_worker";
                    }
                    _ => {}
                }
            }
            inner.batch_queue.retain(|id| !ids.contains(id));
            let mut discarded = Vec::new();
            let mut prepared = VecDeque::new();
            for (id, image) in std::mem::take(&mut inner.batch_prepared) {
                if ids.contains(&id) {
                    let item = inner.batch_items.get_mut(&id).unwrap();
                    item.snapshot.state = TaskState::Cancelled;
                    item.snapshot.stage = "finished";
                    let reserved = std::mem::take(&mut item.reserved_bytes);
                    inner.batch_inflight_bytes -= reserved;
                    discarded.push(image);
                } else {
                    prepared.push_back((id, image));
                }
            }
            inner.batch_prepared = prepared;
            let mut writes = VecDeque::new();
            for (id, image) in std::mem::take(&mut inner.batch_writes) {
                if ids.contains(&id) {
                    let item = inner.batch_items.get_mut(&id).unwrap();
                    item.snapshot.state = TaskState::Cancelled;
                    item.snapshot.stage = "finished";
                    let reserved = std::mem::take(&mut item.reserved_bytes);
                    inner.batch_inflight_bytes -= reserved;
                    discarded.push(image);
                } else {
                    writes.push_back((id, image));
                }
            }
            inner.batch_writes = writes;
            (ids, discarded, inner.changed())
        };
        if let Some((id, handle)) = self.shared.active_inference.lock().unwrap().as_ref() {
            if ids.contains(id) {
                let _ = handle.cancel();
            }
        }
        drop(discarded);
        self.shared.interrupt_automatic_preparation();
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(())
    }

    pub fn retry_batch_item(&self, item_id: u64) -> Result<(), String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let batch_id = inner
                .batch_items
                .get(&item_id)
                .ok_or("批量项不存在")?
                .snapshot
                .batch_id;
            if inner.batch_zip_exports.contains(&batch_id) {
                return Err("请等待 ZIP 打包结束后重试".to_owned());
            }
            let item = inner.batch_items.get_mut(&item_id).ok_or("批量项不存在")?;
            if !matches!(
                item.snapshot.state,
                TaskState::Failed | TaskState::Cancelled
            ) {
                return Err("只有失败或取消的批量项可重试".to_owned());
            }
            item.snapshot.state = TaskState::Queued;
            item.snapshot.stage = "queued";
            item.snapshot.error = None;
            item.output = None;
            item.timings = BatchItemTimings {
                item_id,
                ..BatchItemTimings::default()
            };
            inner.batch_queue.push_back(item_id);
            inner.changed()
        };
        self.shared.interrupt_automatic_preparation();
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(())
    }

    /// Requeue only failed items atomically; keep saved outputs and cancellations.
    pub fn retry_failed_batch(&self, batch_id: u64) -> Result<usize, String> {
        let (count, revision) = {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.batch_zip_exports.contains(&batch_id) {
                return Err("请等待 ZIP 打包结束后重试".to_owned());
            }
            let batch = inner.batches.get(&batch_id).ok_or("批次不存在")?;
            let ids: Vec<_> = batch
                .items
                .iter()
                .copied()
                .filter(|id| inner.batch_items[id].snapshot.state == TaskState::Failed)
                .collect();
            if ids.is_empty() {
                return Ok(0);
            }
            for id in &ids {
                let item = inner.batch_items.get_mut(id).unwrap();
                item.snapshot.state = TaskState::Queued;
                item.snapshot.stage = "queued";
                item.snapshot.error = None;
                item.output = None;
                item.timings = BatchItemTimings {
                    item_id: *id,
                    ..BatchItemTimings::default()
                };
            }
            inner.batch_queue.extend(ids.iter().copied());
            inner.batches.get_mut(&batch_id).unwrap().paused = false;
            (ids.len(), inner.changed())
        };
        self.shared.interrupt_automatic_preparation();
        self.shared.wake.notify_all();
        (self.shared.listener)(revision);
        Ok(count)
    }

    /// Forget terminal metadata without touching any user output files.
    pub fn remove_batch(&self, batch_id: u64) -> Result<(), String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            if inner.batch_zip_exports.contains(&batch_id) {
                return Err("请等待 ZIP 打包结束后移除批次".to_owned());
            }
            let batch = inner.batches.get(&batch_id).ok_or("批次不存在")?;
            if batch.items.iter().any(|id| {
                matches!(
                    inner.batch_items[id].snapshot.state,
                    TaskState::Queued | TaskState::Running | TaskState::CancelRequested
                )
            }) {
                return Err("请先等待批次结束或取消批次".to_owned());
            }
            let batch = inner.batches.remove(&batch_id).unwrap();
            for id in batch.items {
                inner.batch_items.remove(&id);
            }
            inner.changed()
        };
        (self.shared.listener)(revision);
        Ok(())
    }
}

pub(super) fn canonical_batch_sources(paths: &[PathBuf]) -> Result<Vec<BatchInput>, String> {
    if paths.is_empty() {
        return Err("请选择至少一张图片".to_owned());
    }
    paths
        .iter()
        .map(|path| {
            let source = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
            if !source.is_file() {
                return Err("批量输入必须是文件".to_owned());
            }
            let display_name = source
                .file_name()
                .ok_or("文件名无法显示")?
                .to_string_lossy()
                .into_owned();
            Ok(BatchInput {
                origin: SavedBatchSource::File {
                    path: source.clone(),
                    identity: Some(batch_file_identity(&source)?),
                },
                source,
                display_name,
            })
        })
        .collect()
}

pub(super) fn batch_file_identity(path: &Path) -> Result<SavedFileIdentity, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("批量输入必须是文件".to_owned());
    }
    let modified = metadata.modified().map_err(|error| error.to_string())?;
    let modified_ns = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_nanos()),
        Err(error) => i64::try_from(error.duration().as_nanos()).map(|value| -value),
    }
    .map_err(|_| "图片修改时间超出范围")?;
    #[cfg(unix)]
    let (volume_id, file_id) = {
        use std::os::unix::fs::MetadataExt;
        (Some(metadata.dev()), Some(metadata.ino()))
    };
    #[cfg(windows)]
    let (volume_id, file_id) = {
        use std::os::windows::fs::MetadataExt;
        (
            metadata.volume_serial_number().map(u64::from),
            metadata.file_index(),
        )
    };
    #[cfg(not(any(unix, windows)))]
    let (volume_id, file_id) = (None, None);
    Ok(SavedFileIdentity {
        bytes: metadata.len(),
        modified_ns,
        volume_id,
        file_id,
    })
}

pub(super) fn verify_batch_file_source(
    path: &Path,
    expected: Option<&SavedFileIdentity>,
) -> Result<(), String> {
    if !std::fs::canonicalize(path).is_ok_and(|current| current == path) {
        return Err("原始图片已移动，请重新导入".to_owned());
    }
    let current = batch_file_identity(path).map_err(|_| "原始图片已移动，请重新导入")?;
    if expected.is_some_and(|expected| *expected != current) {
        return Err("[source_changed] 原始图片已被替换或修改，请重新导入".to_owned());
    }
    Ok(())
}

pub(super) fn insert_batch_items(
    inner: &mut Inner,
    batch_id: u64,
    sources: Vec<BatchInput>,
) -> Result<(), String> {
    let batch = inner.batches.get(&batch_id).ok_or("批次不存在")?;
    let output_dir = batch.output_dir.clone();
    let mut reserved: HashSet<String> = batch
        .items
        .iter()
        .map(|id| inner.batch_items[id].snapshot.output_name.to_lowercase())
        .collect();
    for BatchInput {
        source,
        display_name,
        origin,
    } in sources
    {
        let stem = Path::new(&display_name)
            .file_stem()
            .ok_or("文件名无法生成输出名")?
            .to_string_lossy();
        let mut suffix = 1_u64;
        let (output_name, destination) = loop {
            let name = if suffix == 1 {
                format!("{stem}-cutout.png")
            } else {
                format!("{stem}-cutout-{suffix}.png")
            };
            let destination = output_dir.join(&name);
            if !reserved.contains(&name.to_lowercase())
                && !destination.exists()
                && destination != source
            {
                break (name, destination);
            }
            suffix = suffix.checked_add(1).ok_or("输出名称空间已用尽")?;
        };
        reserved.insert(output_name.to_lowercase());
        let item_id = inner.next_id();
        inner.batch_items.insert(
            item_id,
            BatchItem {
                source,
                origin,
                destination,
                output: None,
                reserved_bytes: 0,
                timings: BatchItemTimings {
                    item_id,
                    ..BatchItemTimings::default()
                },
                snapshot: BatchItemSnapshot {
                    item_id,
                    batch_id,
                    display_name,
                    output_name,
                    state: TaskState::Queued,
                    stage: "queued",
                    error: None,
                },
            },
        );
        inner
            .batches
            .get_mut(&batch_id)
            .unwrap()
            .items
            .push(item_id);
        inner.batch_queue.push_back(item_id);
    }
    Ok(())
}
