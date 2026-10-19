use super::*;

impl Workspace {
    pub(super) fn materialize_recovered_file(&self, source: &Path) -> Result<PathBuf, String> {
        let pending = tempfile::Builder::new()
            .prefix("recovered-")
            .tempfile_in(self.clipboard_dir.path())
            .map_err(|error| error.to_string())?;
        let path = pending.path().to_path_buf();
        drop(pending);
        if std::fs::hard_link(source, &path).is_err() {
            std::fs::copy(source, &path).map_err(|error| error.to_string())?;
        }
        Ok(path)
    }

    /// Copies a consistent view of committed assets to private storage. The
    /// caller runs this off the UI thread and may schedule another checkpoint
    /// if the returned workspace revision is no longer current.
    pub fn checkpoint(&self, store: &SessionStore) -> Result<u64, String> {
        struct CapturedAsset {
            id: u64,
            name: String,
            source: PathBuf,
            workflow: Workflow,
            active: (Option<GrayImage>, Vec<GrayImage>, Vec<GrayImage>),
            inactive: (Option<GrayImage>, Vec<GrayImage>, Vec<GrayImage>),
            points: Vec<sam::Point>,
            background: BackgroundState,
        }
        let (revision, selected, assets, batches) = {
            let inner = self.shared.inner.lock().unwrap();
            let assets = inner
                .assets
                .values()
                .map(|asset| CapturedAsset {
                    id: asset.snapshot.asset_id,
                    name: asset.snapshot.display_name.clone(),
                    source: asset.source.clone(),
                    workflow: asset.snapshot.workflow,
                    active: (asset.mask.clone(), asset.undo.clone(), asset.redo.clone()),
                    inactive: (
                        asset.inactive.mask.clone(),
                        asset.inactive.undo.clone(),
                        asset.inactive.redo.clone(),
                    ),
                    points: asset.snapshot.selection_points.clone(),
                    background: asset.background.clone(),
                })
                .collect::<Vec<_>>();
            let batches = inner
                .batches
                .iter()
                .map(|(batch_id, batch)| SavedBatch {
                    batch_id: *batch_id,
                    output_directory: batch.output_dir.clone(),
                    paused: batch.paused,
                    items: batch
                        .items
                        .iter()
                        .map(|id| {
                            let item = &inner.batch_items[id];
                            SavedBatchItem {
                                item_id: *id,
                                source: item.origin.clone(),
                                display_name: item.snapshot.display_name.clone(),
                                output_name: item.snapshot.output_name.clone(),
                                state: match item.snapshot.state {
                                    TaskState::Queued => SavedBatchState::Queued,
                                    TaskState::Running => SavedBatchState::Running,
                                    TaskState::CancelRequested => SavedBatchState::CancelRequested,
                                    TaskState::Cancelled => SavedBatchState::Cancelled,
                                    TaskState::Succeeded => SavedBatchState::Succeeded,
                                    TaskState::Failed | TaskState::Stale => SavedBatchState::Failed,
                                },
                                error: item.snapshot.error.clone(),
                                output: item.output.clone(),
                            }
                        })
                        .collect(),
                })
                .collect::<Vec<_>>();
            (inner.revision, inner.selected_asset_id, assets, batches)
        };
        let mut checkpoint = SessionCheckpoint::empty();
        checkpoint.workspace_revision = revision;
        checkpoint.selected_asset_id = selected;
        checkpoint.batches = batches;
        for asset in assets {
            let source = if asset.source.exists() {
                store.put_file(&asset.source)?
            } else {
                // The original can be moved after the first successful save.
                // Keep using its verified private copy for subsequent edits.
                store
                    .load()?
                    .and_then(|saved| {
                        saved
                            .assets
                            .into_iter()
                            .find(|item| item.asset_id == asset.id)
                    })
                    .map(|saved| saved.source)
                    .ok_or("原图已移动，且没有可恢复的私有副本")?
            };
            let active = save_edits(store, asset.active)?;
            let inactive = save_edits(store, asset.inactive)?;
            let (automatic, selection) = match asset.workflow {
                Workflow::Automatic => (active, inactive),
                Workflow::Selection => (inactive, active),
            };
            let background = match asset.background {
                BackgroundState::Transparent => SavedBackground::Transparent,
                BackgroundState::Solid(rgb) => SavedBackground::Solid { rgb },
                BackgroundState::Image {
                    path, display_name, ..
                }
                | BackgroundState::ImagePending {
                    path, display_name, ..
                } => SavedBackground::Image {
                    display_name,
                    file: if path.exists() {
                        store.put_file(&path)?
                    } else {
                        store
                            .load()?
                            .and_then(|saved| {
                                saved
                                    .assets
                                    .into_iter()
                                    .find(|item| item.asset_id == asset.id)
                            })
                            .and_then(|saved| match saved.background {
                                SavedBackground::Image { file, .. } => Some(file),
                                _ => None,
                            })
                            .ok_or("背景图已移动，且没有可恢复的私有副本")?
                    },
                },
            };
            checkpoint.assets.push(SavedAsset {
                asset_id: asset.id,
                display_name: asset.name,
                source,
                workflow: match asset.workflow {
                    Workflow::Automatic => SavedWorkflow::Automatic,
                    Workflow::Selection => SavedWorkflow::Selection,
                },
                automatic,
                selection,
                selection_points: asset.points,
                background,
            });
        }
        store.commit(&checkpoint)?;
        Ok(revision)
    }

    /// Restore into a fresh workspace only. No interrupted task or ORT
    /// session is resumed; previews are rebuilt on demand from private blobs.
    pub fn restore(&self, store: &SessionStore) -> Result<bool, String> {
        {
            let inner = self.shared.inner.lock().unwrap();
            if !inner.assets.is_empty() || !inner.tasks.is_empty() || !inner.batches.is_empty() {
                return Err("只能恢复到空工作区".to_owned());
            }
        }
        let Some(checkpoint) = store.load()? else {
            return Ok(false);
        };
        if checkpoint.workspace_revision >= u64::MAX - 1 {
            return Err("会话修订号无效".to_owned());
        }
        let mut assets = BTreeMap::new();
        let mut mask_bytes = 0_usize;
        for saved in checkpoint.assets {
            let source =
                self.materialize_recovered_file(&store.verified_blob_path(&saved.source)?)?;
            let automatic = load_edits(store, saved.automatic)?;
            let selection = load_edits(store, saved.selection)?;
            let asset_mask_bytes = edit_bytes(&automatic)
                .checked_add(edit_bytes(&selection))
                .ok_or("恢复蒙版超过内存预算")?;
            mask_bytes = mask_bytes
                .checked_add(asset_mask_bytes)
                .ok_or("恢复蒙版超过内存预算")?;
            if mask_bytes > MASK_BUDGET {
                return Err("恢复蒙版超过内存预算".to_owned());
            }
            let (workflow, active, inactive) = match saved.workflow {
                SavedWorkflow::Automatic => (Workflow::Automatic, automatic, selection),
                SavedWorkflow::Selection => (Workflow::Selection, selection, automatic),
            };
            let background = match saved.background {
                SavedBackground::Transparent => BackgroundState::Transparent,
                SavedBackground::Solid { rgb } => BackgroundState::Solid(rgb),
                SavedBackground::Image { display_name, file } => BackgroundState::ImagePending {
                    path: self.materialize_recovered_file(&store.verified_blob_path(&file)?)?,
                    display_name,
                },
            };
            let asset_id = saved.asset_id;
            assets.insert(
                asset_id,
                Asset {
                    source,
                    snapshot: AssetSnapshot {
                        asset_id,
                        workflow_id: asset_id,
                        display_name: saved.display_name,
                        width: None,
                        height: None,
                        generation: 1,
                        mask_revision: 0,
                        result_revision: 0,
                        background_revision: 0,
                        background: background.view(),
                        preview_revision: 0,
                        preview_available: false,
                        result_available: false,
                        workflow,
                        selection_points: saved.selection_points,
                        can_undo: !active.undo.is_empty(),
                        can_redo: !active.redo.is_empty(),
                        state: AssetState::Released,
                        error: None,
                    },
                    preview: None,
                    result_preview: None,
                    preview_access: 0,
                    mask: active.mask,
                    undo: active.undo,
                    redo: active.redo,
                    inactive,
                    background,
                },
            );
        }
        let (batches, batch_items, batch_queue) = restore_batch_metadata(checkpoint.batches);
        let selected = checkpoint
            .selected_asset_id
            .or_else(|| assets.keys().next().copied());
        let next_id = assets
            .keys()
            .chain(batches.keys())
            .chain(batch_items.keys())
            .copied()
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("会话资产标识无效")?;
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            if !inner.assets.is_empty() || !inner.tasks.is_empty() || !inner.batches.is_empty() {
                return Err("只能恢复到空工作区".to_owned());
            }
            inner.assets = assets;
            inner.batches = batches;
            inner.batch_items = batch_items;
            inner.batch_queue = batch_queue;
            inner.selected_asset_id = selected;
            inner.selection_generation = u64::from(selected.is_some());
            inner.next_id = next_id;
            inner.mask_bytes = mask_bytes;
            inner.revision = checkpoint.workspace_revision;
            inner.changed()
        };
        (self.shared.listener)(revision);
        Ok(true)
    }

    /// Decode the chosen image and render both workflow previews after a
    /// restore or cache eviction. Expensive work runs outside the state lock.
    pub fn hydrate_preview(&self, asset_id: u64) -> Result<bool, String> {
        let (
            source,
            mask,
            inactive_mask,
            history_sizes,
            background,
            generation,
            mask_revision,
            background_revision,
        ) = {
            let inner = self.shared.inner.lock().unwrap();
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if asset.preview.is_some() {
                return Ok(true);
            }
            (
                asset.source.clone(),
                asset.mask.clone(),
                asset.inactive.mask.clone(),
                asset
                    .undo
                    .iter()
                    .chain(&asset.redo)
                    .chain(&asset.inactive.undo)
                    .chain(&asset.inactive.redo)
                    .map(GrayImage::dimensions)
                    .collect::<Vec<_>>(),
                asset.background.clone(),
                asset.snapshot.generation,
                asset.snapshot.mask_revision,
                asset.snapshot.background_revision,
            )
        };
        let preview = asset::build_preview(&source).map_err(|error| error.to_string())?;
        if mask
            .iter()
            .chain(inactive_mask.iter())
            .any(|mask| mask.dimensions() != preview.source_size)
            || history_sizes
                .iter()
                .any(|size| *size != preview.source_size)
        {
            return Err("恢复蒙版尺寸与原图不一致".to_owned());
        }
        let background = match background {
            BackgroundState::ImagePending { path, display_name } => {
                let original =
                    image::load_from_memory_with_format(&preview.png, image::ImageFormat::Png)
                        .map_err(|error| error.to_string())?;
                let image = image_io::decode_image(&path).map_err(|error| error.to_string())?;
                let fitted = image::DynamicImage::ImageRgba8(image)
                    .resize_to_fill(
                        original.width(),
                        original.height(),
                        image::imageops::FilterType::CatmullRom,
                    )
                    .to_rgba8();
                BackgroundState::Image {
                    path,
                    display_name,
                    preview: Arc::new(fitted),
                }
            }
            other => other,
        };
        let render = |mask: Option<&GrayImage>| -> Result<Option<Arc<Vec<u8>>>, String> {
            mask.map(|value| {
                asset::build_result_preview(&preview.png, value, background.preview())
                    .map(Arc::new)
                    .map_err(|error| error.to_string())
            })
            .transpose()
        };
        let result_preview = render(mask.as_ref())?;
        let inactive_preview = render(inactive_mask.as_ref())?;
        let new_bytes = preview.png.len()
            + background.bytes()
            + result_preview.as_ref().map_or(0, |bytes| bytes.len())
            + inactive_preview.as_ref().map_or(0, |bytes| bytes.len());
        if new_bytes > PREVIEW_BUDGET {
            return Err("恢复预览超过内存预算".to_owned());
        }
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if asset.snapshot.generation != generation
                || asset.snapshot.mask_revision != mask_revision
                || asset.snapshot.background_revision != background_revision
            {
                return Ok(false);
            }
            if asset.preview.is_some() {
                return Ok(true);
            }
            inner.preview_bytes += new_bytes;
            inner.access_tick += 1;
            let tick = inner.access_tick;
            if inner.selected_asset_id == Some(asset_id) {
                inner.decoded_source = preview.source.map(|source| (asset_id, source));
            }
            let asset = inner.assets.get_mut(&asset_id).unwrap();
            asset.snapshot.width = Some(preview.source_size.0);
            asset.snapshot.height = Some(preview.source_size.1);
            asset.snapshot.state = AssetState::Ready;
            asset.snapshot.error = None;
            asset.snapshot.preview_available = true;
            asset.snapshot.preview_revision += 1;
            asset.snapshot.result_available = result_preview.is_some();
            asset.snapshot.result_revision += 1;
            asset.snapshot.background = background.view();
            asset.preview = Some(Arc::new(preview.png));
            asset.result_preview = result_preview;
            asset.inactive.result_preview = inactive_preview;
            asset.background = background;
            asset.preview_access = tick;
            inner.evict_previews(asset_id);
            inner.changed()
        };
        (self.shared.listener)(revision);
        Ok(true)
    }
}

pub(super) fn save_edits(
    store: &SessionStore,
    (mask, undo, redo): (Option<GrayImage>, Vec<GrayImage>, Vec<GrayImage>),
) -> Result<SavedEdits, String> {
    let save_mask = |mask: &GrayImage| -> Result<BlobRef, String> {
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(mask.clone())
            .write_to(&mut encoded, image::ImageFormat::Png)
            .map_err(|error| error.to_string())?;
        store.put_bytes(encoded.get_ref())
    };
    Ok(SavedEdits {
        mask: mask.as_ref().map(&save_mask).transpose()?,
        undo: undo.iter().map(&save_mask).collect::<Result<_, _>>()?,
        redo: redo.iter().map(&save_mask).collect::<Result<_, _>>()?,
    })
}

pub(super) fn load_edits(store: &SessionStore, saved: SavedEdits) -> Result<EditState, String> {
    let load_mask = |reference: &BlobRef| -> Result<GrayImage, String> {
        let path = store.verified_blob_path(reference)?;
        image_io::decode_gray_png(&path).map_err(|error| error.to_string())
    };
    Ok(EditState {
        mask: saved.mask.as_ref().map(&load_mask).transpose()?,
        result_preview: None,
        undo: saved
            .undo
            .iter()
            .map(&load_mask)
            .collect::<Result<_, _>>()?,
        redo: saved
            .redo
            .iter()
            .map(&load_mask)
            .collect::<Result<_, _>>()?,
    })
}

pub(super) fn archive_batch_inputs(
    archive: &Path,
    images: Vec<crate::batch_zip::ImportedImage>,
) -> Vec<BatchInput> {
    images
        .into_iter()
        .map(|image| BatchInput {
            source: image.path,
            display_name: image.display_name,
            origin: SavedBatchSource::Archive {
                path: archive.to_path_buf(),
                entry_index: image.entry_index,
                sha256: image.sha256,
            },
        })
        .collect()
}

pub(super) fn restore_batch_metadata(
    saved_batches: Vec<SavedBatch>,
) -> (
    BTreeMap<u64, Batch>,
    BTreeMap<u64, BatchItem>,
    VecDeque<u64>,
) {
    let mut batches = BTreeMap::new();
    let mut items = BTreeMap::new();
    let mut queue = VecDeque::new();
    for saved in saved_batches {
        let output_valid = saved
            .items
            .iter()
            .filter(|item| matches!(item.state, SavedBatchState::Succeeded))
            .map(|item| {
                (
                    item.item_id,
                    committed_batch_output_exists(
                        &saved.output_directory.join(&item.output_name),
                        item.output.as_ref(),
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut archives: BTreeMap<PathBuf, Result<crate::batch_zip::ImportedArchive, String>> =
            BTreeMap::new();
        for item in &saved.items {
            if let SavedBatchSource::Archive { path, .. } = &item.source {
                if matches!(item.state, SavedBatchState::Succeeded)
                    && output_valid.get(&item.item_id) == Some(&true)
                {
                    continue;
                }
                archives.entry(path.clone()).or_insert_with(|| {
                    if !std::fs::canonicalize(path).is_ok_and(|current| current == *path) {
                        return Err("ZIP 路径已变化".to_owned());
                    }
                    crate::batch_zip::import_images(path)
                });
            }
        }
        let output_ok = saved.output_directory.is_dir()
            && std::fs::canonicalize(&saved.output_directory)
                .is_ok_and(|path| path == saved.output_directory);
        let mut item_ids = Vec::with_capacity(saved.items.len());
        for item in saved.items {
            let (source, input_error) = match &item.source {
                SavedBatchSource::File { path, identity } => (
                    path.clone(),
                    verify_batch_file_source(path, identity.as_ref()).err(),
                ),
                SavedBatchSource::Archive {
                    path,
                    entry_index,
                    sha256,
                } => {
                    let found = archives
                        .get(path)
                        .and_then(|result| result.as_ref().ok())
                        .and_then(|archive| {
                            archive
                                .sources
                                .iter()
                                .find(|image| image.entry_index == *entry_index)
                        });
                    match found {
                        Some(image)
                            if image.sha256 == *sha256
                                && image.display_name == item.display_name =>
                        {
                            (image.path.clone(), None)
                        }
                        _ => (
                            path.clone(),
                            Some("ZIP 已移动或内容变化，请重新导入".to_owned()),
                        ),
                    }
                }
            };
            let destination = saved.output_directory.join(&item.output_name);
            let (state, error) = match item.state {
                SavedBatchState::Succeeded if output_valid.get(&item.item_id) != Some(&true) => (
                    TaskState::Failed,
                    Some("已保存的 PNG 丢失或内容变化，请核对输出目录".to_owned()),
                ),
                SavedBatchState::Succeeded => (TaskState::Succeeded, None),
                SavedBatchState::Cancelled => (TaskState::Cancelled, None),
                SavedBatchState::Failed => (TaskState::Failed, item.error),
                SavedBatchState::Running | SavedBatchState::CancelRequested => (
                    TaskState::Failed,
                    Some("上次退出时处理中断，请重试".to_owned()),
                ),
                SavedBatchState::Queued => (TaskState::Queued, None),
            };
            let (state, error) = if state != TaskState::Succeeded
                && state != TaskState::Cancelled
                && std::fs::symlink_metadata(&destination).is_ok()
            {
                (
                    TaskState::Failed,
                    if matches!(item.state, SavedBatchState::Succeeded) {
                        error
                    } else {
                        Some("输出目录已有未记录的同名文件，请核对后重新创建批次".to_owned())
                    },
                )
            } else {
                (state, error)
            };
            let (state, error) = if !output_ok {
                (
                    TaskState::Failed,
                    Some("输出目录不可用，请重新创建批次".to_owned()),
                )
            } else if state != TaskState::Succeeded && state != TaskState::Cancelled {
                if let Some(reason) = input_error {
                    (TaskState::Failed, Some(reason))
                } else {
                    (state, error)
                }
            } else {
                (state, error)
            };
            if state == TaskState::Queued {
                queue.push_back(item.item_id);
            }
            item_ids.push(item.item_id);
            items.insert(
                item.item_id,
                BatchItem {
                    source,
                    origin: item.source,
                    destination,
                    output: if state == TaskState::Succeeded {
                        item.output
                    } else {
                        None
                    },
                    reserved_bytes: 0,
                    timings: BatchItemTimings {
                        item_id: item.item_id,
                        ..BatchItemTimings::default()
                    },
                    snapshot: BatchItemSnapshot {
                        item_id: item.item_id,
                        batch_id: saved.batch_id,
                        display_name: item.display_name,
                        output_name: item.output_name,
                        state,
                        stage: if state == TaskState::Queued {
                            "queued"
                        } else {
                            "finished"
                        },
                        error,
                    },
                },
            );
        }
        let imported_archives = archives
            .into_values()
            .filter_map(Result::ok)
            .map(|archive| archive.directory)
            .collect();
        batches.insert(
            saved.batch_id,
            Batch {
                output_dir: saved.output_directory,
                paused: true,
                items: item_ids,
                imported_archives,
            },
        );
    }
    (batches, items, queue)
}

pub(super) fn committed_batch_output_exists(
    path: &Path,
    expected: Option<&SavedBatchOutput>,
) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file()
        || expected.is_some_and(|expected| expected.bytes != metadata.len())
    {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut signature = [0_u8; 8];
    if file.read_exact(&mut signature).is_err() || signature != *b"\x89PNG\r\n\x1a\n" {
        return false;
    }
    expected.is_none_or(|expected| {
        image_io::file_digest(path)
            .is_ok_and(|(sha256, bytes)| sha256 == expected.sha256 && bytes == expected.bytes)
    })
}
