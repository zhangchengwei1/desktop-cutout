use super::*;

enum WorkerJob {
    PrepareAutomatic {
        asset_id: u64,
        generation: u64,
        path: PathBuf,
        epoch: u64,
    },
    Asset {
        task_id: u64,
        asset_id: u64,
        generation: u64,
        selection_generation: u64,
        mask_revision: u64,
        kind: &'static str,
        path: PathBuf,
        points: Option<Vec<sam::Point>>,
        edge: Option<u32>,
        revision: u64,
    },
    Batch {
        item_id: u64,
        source: RgbaImage,
        revision: u64,
    },
}

pub(super) fn run_worker(shared: Arc<Shared>) {
    let mut automatic_session: Option<(IsnetSession, ModelLease)> = None;
    let mut selection_session: Option<CachedSelection> = None;
    let mut refinement_session: Option<(RefinementSession, ModelLease)> = None;
    let mut preparation_attempt: Option<(u64, u64)> = None;
    loop {
        let idle_since = Instant::now();
        let job = {
            let mut inner = shared.inner.lock().unwrap();
            loop {
                if inner.shutdown {
                    return;
                }
                if selection_session.as_ref().is_some_and(|cached| {
                    inner.selected_asset_id != Some(cached.asset_id)
                        || inner.assets.get(&cached.asset_id).is_none_or(|asset| {
                            asset.snapshot.generation != cached.generation
                                || asset.snapshot.workflow != Workflow::Selection
                        })
                }) {
                    selection_session = None;
                }
                let Some(task_id) = inner.queue.pop_front() else {
                    // Paused batches may already have decoded images. Preserve
                    // them without executing inference or blocking other batches.
                    let runnable = inner.batch_prepared.iter().position(|(id, _)| {
                        inner
                            .batches
                            .get(&inner.batch_items[id].snapshot.batch_id)
                            .is_some_and(|batch| !batch.paused)
                    });
                    if let Some((item_id, source)) =
                        runnable.and_then(|index| inner.batch_prepared.remove(index))
                    {
                        let item = &mut inner.batch_items.get_mut(&item_id).unwrap().snapshot;
                        item.stage = "loading_model";
                        shared.wake.notify_all();
                        break WorkerJob::Batch {
                            item_id,
                            source,
                            revision: inner.changed(),
                        };
                    }
                    // Actual queued work always wins. Prepare at most one selected
                    // image, after a short idle interval, without publishing edits.
                    if refinement_session.is_none() && selection_session.is_none() {
                        if let Some((asset_id, generation, path)) =
                            automatic_preparation_candidate(&inner, preparation_attempt)
                        {
                            if let Some((_, store)) = &shared.inference {
                                let models = store.snapshot();
                                if models.features.automatic.files_verified {
                                    let delay = std::time::Duration::from_millis(200)
                                        .saturating_sub(idle_since.elapsed());
                                    if !delay.is_zero() {
                                        inner = shared.wake.wait_timeout(inner, delay).unwrap().0;
                                        continue;
                                    }
                                    preparation_attempt = Some((asset_id, generation));
                                    break WorkerJob::PrepareAutomatic {
                                        asset_id,
                                        generation,
                                        path,
                                        epoch: shared
                                            .automatic_preparation_epoch
                                            .load(Ordering::Acquire),
                                    };
                                }
                                // Verification/download events belong to ModelStore.
                                // Recheck only while a model operation is in progress.
                                if models.models.iter().any(|model| {
                                    model.spec.id == "isnet-auto-high"
                                        && matches!(
                                            model.state,
                                            crate::model_store::ModelState::Verifying
                                                | crate::model_store::ModelState::Queued
                                                | crate::model_store::ModelState::Downloading
                                        )
                                }) {
                                    inner = shared
                                        .wake
                                        .wait_timeout(inner, std::time::Duration::from_secs(1))
                                        .unwrap()
                                        .0;
                                    continue;
                                }
                            }
                        }
                    }
                    inner = shared.wake.wait(inner).unwrap();
                    continue;
                };
                let Some(task) = inner.tasks.get(&task_id) else {
                    continue;
                };
                if task.state != TaskState::Queued {
                    continue;
                }
                let asset_id = task.asset_id;
                let generation = task.generation;
                let selection_generation = task.selection_generation;
                let mask_revision = task.mask_revision;
                let kind = task.kind;
                let Some(path) = inner
                    .assets
                    .get(&asset_id)
                    .map(|asset| asset.source.clone())
                else {
                    inner.tasks.get_mut(&task_id).unwrap().state = TaskState::Stale;
                    continue;
                };
                let task = inner.tasks.get_mut(&task_id).unwrap();
                task.state = TaskState::Running;
                task.stage = match kind {
                    "automatic" => "loading_model",
                    "selection" => "loading_model",
                    "refinement" => "loading_model",
                    "export" => "exporting",
                    _ => "preparing_preview",
                };
                let points = inner.selections.remove(&task_id);
                let edge = inner.refinements.remove(&task_id);
                let revision = inner.changed();
                break WorkerJob::Asset {
                    task_id,
                    asset_id,
                    generation,
                    selection_generation,
                    mask_revision,
                    kind,
                    path,
                    points,
                    edge,
                    revision,
                };
            }
        };
        let (
            task_id,
            asset_id,
            generation,
            selection_generation,
            mask_revision,
            kind,
            path,
            points,
            edge,
            revision,
        ) = match job {
            WorkerJob::PrepareAutomatic {
                asset_id,
                generation,
                path,
                epoch,
            } => {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    prepare_automatic(
                        &shared,
                        &mut automatic_session,
                        asset_id,
                        generation,
                        &path,
                        epoch,
                    )
                }));
                *shared.automatic_preparation.lock().unwrap() = None;
                if !matches!(outcome, Ok(Ok(())))
                    || automatic_session
                        .as_ref()
                        .is_some_and(|(session, _)| session.cancel_handle().is_requested())
                {
                    automatic_session = None;
                }
                continue;
            }
            WorkerJob::Batch {
                item_id,
                source,
                revision,
            } => {
                (shared.listener)(revision);
                refinement_session = None;
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_batch_inference(&shared, &mut automatic_session, item_id, source)
                }));
                if outcome.is_err() {
                    *shared.active_inference.lock().unwrap() = None;
                    automatic_session = None;
                    finish_batch_item(&shared, item_id, Err("批量处理异常".to_owned()));
                }
                continue;
            }
            WorkerJob::Asset {
                task_id,
                asset_id,
                generation,
                selection_generation,
                mask_revision,
                kind,
                path,
                points,
                edge,
                revision,
            } => (
                task_id,
                asset_id,
                generation,
                selection_generation,
                mask_revision,
                kind,
                path,
                points,
                edge,
                revision,
            ),
        };
        (shared.listener)(revision);
        if kind == "automatic" {
            refinement_session = None;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_automatic(&shared, &mut automatic_session, task_id, asset_id, &path)
            }));
            if outcome.is_err() {
                *shared.active_inference.lock().unwrap() = None;
                automatic_session = None;
                finish_task(&shared, task_id, Err("自动抠图处理异常".to_owned()));
            }
            continue;
        }
        if kind == "selection" {
            refinement_session = None;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_selection(
                    &shared,
                    &mut selection_session,
                    task_id,
                    asset_id,
                    generation,
                    &path,
                    points,
                )
            }));
            if outcome.is_err() {
                selection_session = None;
                finish_task(&shared, task_id, Err("点选抠图处理异常".to_owned()));
            }
            continue;
        }
        if kind == "refinement" {
            automatic_session = None;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_refinement(&shared, &mut refinement_session, task_id, &path, edge)
            }));
            if outcome.is_err() {
                refinement_session = None;
                finish_task(&shared, task_id, Err("精修处理异常".to_owned()));
            }
            continue;
        }
        if kind == "export" {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_export(
                    &shared,
                    task_id,
                    asset_id,
                    generation,
                    selection_generation,
                    mask_revision,
                    &path,
                )
            }));
            if outcome.is_err() {
                finish_task(&shared, task_id, Err("PNG 导出处理异常".to_owned()));
            }
            continue;
        }
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (shared.processor)(&path)))
                .unwrap_or_else(|_| Err("预览处理异常".to_owned()));
        let revision = {
            let mut inner = shared.inner.lock().unwrap();
            let stale = inner
                .assets
                .get(&asset_id)
                .is_none_or(|asset| asset.snapshot.generation != generation);
            let cancelled = inner
                .tasks
                .get(&task_id)
                .is_some_and(|task| task.state == TaskState::CancelRequested);
            if stale || cancelled || inner.shutdown {
                let task = inner.tasks.get_mut(&task_id).unwrap();
                task.state = if stale {
                    TaskState::Stale
                } else {
                    TaskState::Cancelled
                };
                task.stage = "finished";
            } else {
                match result {
                    Ok(preview) if preview.png.len() <= PREVIEW_BUDGET => {
                        inner.clear_preview(asset_id);
                        inner.preview_bytes += preview.png.len();
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
                        asset.preview = Some(Arc::new(preview.png));
                        asset.preview_access = tick;
                        inner.evict_previews(asset_id);
                        let task = inner.tasks.get_mut(&task_id).unwrap();
                        task.state = TaskState::Succeeded;
                        task.stage = "finished";
                    }
                    Ok(_) => {
                        let message = "预览超过内存预算".to_owned();
                        let asset = inner.assets.get_mut(&asset_id).unwrap();
                        asset.snapshot.state = AssetState::Failed;
                        asset.snapshot.error = Some(message.clone());
                        let task = inner.tasks.get_mut(&task_id).unwrap();
                        task.state = TaskState::Failed;
                        task.stage = "finished";
                        task.error = Some(message);
                    }
                    Err(message) => {
                        let asset = inner.assets.get_mut(&asset_id).unwrap();
                        asset.snapshot.state = AssetState::Failed;
                        asset.snapshot.error = Some(message.clone());
                        let task = inner.tasks.get_mut(&task_id).unwrap();
                        task.state = TaskState::Failed;
                        task.stage = "finished";
                        task.error = Some(message);
                    }
                }
            }
            inner.trim_history();
            inner.changed()
        };
        (shared.listener)(revision);
    }
}

pub(super) fn automatic_preparation_candidate(
    inner: &Inner,
    attempted: Option<(u64, u64)>,
) -> Option<(u64, u64, PathBuf)> {
    if inner.shutdown
        || !inner.queue.is_empty()
        || inner
            .batch_items
            .values()
            .any(|item| matches!(item.snapshot.state, TaskState::Queued | TaskState::Running))
    {
        return None;
    }
    let asset_id = inner.selected_asset_id?;
    let asset = inner.assets.get(&asset_id)?;
    let source_bytes = u64::from(asset.snapshot.width?)
        .checked_mul(u64::from(asset.snapshot.height?))?
        .checked_mul(4)?;
    (asset.snapshot.state == AssetState::Ready
        && asset.preview.is_some()
        && asset.snapshot.workflow == Workflow::Automatic
        && source_bytes <= asset::SOURCE_CACHE_BUDGET as u64
        && attempted != Some((asset_id, asset.snapshot.generation)))
    .then(|| (asset_id, asset.snapshot.generation, asset.source.clone()))
}

fn prepare_automatic(
    shared: &Shared,
    session: &mut Option<(IsnetSession, ModelLease)>,
    asset_id: u64,
    generation: u64,
    path: &Path,
    epoch: u64,
) -> Result<(), String> {
    let (runtime, store) = shared.inference.as_ref().ok_or("原生推理环境尚未配置")?;
    if session.is_none() {
        let lease = store.acquire("isnet-auto-high")?;
        let loaded =
            IsnetSession::load_verified(runtime, &lease).map_err(|error| error.to_string())?;
        *session = Some((loaded, lease));
    }
    let (session, lease) = session.as_mut().unwrap();
    let handle = session.cancel_handle();
    *shared.automatic_preparation.lock().unwrap() = Some((asset_id, handle));
    {
        let inner = shared.inner.lock().unwrap();
        if shared.automatic_preparation_epoch.load(Ordering::Acquire) != epoch
            || inner.shutdown
            || inner.selected_asset_id != Some(asset_id)
            || inner.assets.get(&asset_id).is_none_or(|asset| {
                asset.snapshot.generation != generation
                    || asset.snapshot.workflow != Workflow::Automatic
            })
            || inner.queue.iter().any(|id| {
                inner
                    .tasks
                    .get(id)
                    .is_some_and(|task| task.kind != "automatic")
            })
            || inner
                .batch_items
                .values()
                .any(|item| matches!(item.snapshot.state, TaskState::Queued | TaskState::Running))
        {
            return Err("空闲准备已失效".to_owned());
        }
    }
    let source = source_for_asset(shared, asset_id, path)?;
    session
        .prepare_mask(&source)
        .map_err(|error| error.to_string())?;
    lease.mark_session_ready();
    Ok(())
}

pub(super) fn task_is_current(inner: &Inner, task_id: u64) -> bool {
    let Some(task) = inner.tasks.get(&task_id) else {
        return false;
    };
    let Some(asset) = inner.assets.get(&task.asset_id) else {
        return false;
    };
    task.state == TaskState::Running
        && inner.selected_asset_id == Some(task.asset_id)
        && inner.selection_generation == task.selection_generation
        && asset.snapshot.generation == task.generation
        && asset.snapshot.mask_revision == task.mask_revision
}

pub(super) fn finish_task(shared: &Shared, task_id: u64, result: Result<(), String>) {
    let revision = {
        let mut inner = shared.inner.lock().unwrap();
        let current = task_is_current(&inner, task_id);
        let task = inner.tasks.get_mut(&task_id).unwrap();
        if result.is_ok() {
            task.state = TaskState::Succeeded;
        } else if task.state == TaskState::CancelRequested {
            task.state = TaskState::Cancelled;
        } else if !current {
            task.state = TaskState::Stale;
        } else if let Err(error) = result {
            task.state = TaskState::Failed;
            task.error = Some(error);
        }
        task.stage = "finished";
        inner.exports.remove(&task_id);
        inner.selections.remove(&task_id);
        inner.refinements.remove(&task_id);
        inner.trim_history();
        inner.changed()
    };
    (shared.listener)(revision);
}
