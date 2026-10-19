use super::*;

pub(super) fn commit_generated_mask(
    shared: &Shared,
    task_id: u64,
    mask: GrayImage,
    points: Option<Vec<sam::Point>>,
) -> Result<(), String> {
    let (original_png, background, background_revision) = {
        let inner = shared.inner.lock().unwrap();
        if !task_is_current(&inner, task_id) {
            return Err("任务已失效".to_owned());
        }
        let asset_id = inner.tasks[&task_id].asset_id;
        let asset = &inner.assets[&asset_id];
        (
            Arc::clone(asset.preview.as_ref().ok_or("预览已释放")?),
            asset.background.clone(),
            asset.snapshot.background_revision,
        )
    };
    let result_png = asset::build_result_preview(&original_png, &mask, background.preview())
        .map_err(|error| error.to_string())?;
    let revision = {
        let mut inner = shared.inner.lock().unwrap();
        if !task_is_current(&inner, task_id) {
            return Err("任务已失效".to_owned());
        }
        let asset_id = inner.tasks[&task_id].asset_id;
        let asset = &inner.assets[&asset_id];
        if asset.snapshot.background_revision != background_revision {
            return Err("背景已更新，请重试".to_owned());
        }
        if mask.dimensions()
            != (
                asset.snapshot.width.unwrap_or(0),
                asset.snapshot.height.unwrap_or(0),
            )
        {
            return Err("蒙版尺寸与资产不匹配".to_owned());
        }
        let redo_bytes: usize = asset.redo.iter().map(|value| value.as_raw().len()).sum();
        let dropped_bytes = asset
            .undo
            .first()
            .filter(|_| asset.undo.len() >= EDIT_HISTORY_LIMIT)
            .map_or(0, |value| value.as_raw().len());
        let old_preview = asset.result_preview.as_ref().map_or(0, |value| value.len());
        if inner.mask_bytes - redo_bytes - dropped_bytes + mask.as_raw().len() > MASK_BUDGET
            || inner.preview_bytes - old_preview + result_png.len() > PREVIEW_BUDGET
        {
            return Err("结果超过内存预算".to_owned());
        }
        inner.mask_bytes = inner.mask_bytes - redo_bytes - dropped_bytes + mask.as_raw().len();
        inner.preview_bytes = inner.preview_bytes - old_preview + result_png.len();
        let asset = inner.assets.get_mut(&asset_id).unwrap();
        if asset.undo.len() >= EDIT_HISTORY_LIMIT {
            asset.undo.remove(0);
        }
        if let Some(previous) = asset.mask.take() {
            asset.undo.push(previous);
        }
        asset.redo.clear();
        asset.mask = Some(mask);
        asset.result_preview = Some(Arc::new(result_png));
        asset.snapshot.mask_revision += 1;
        asset.snapshot.result_revision += 1;
        asset.snapshot.result_available = true;
        asset.snapshot.can_undo = !asset.undo.is_empty();
        asset.snapshot.can_redo = false;
        if let Some(points) = points {
            asset.snapshot.selection_points = points;
        }
        let task = inner.tasks.get_mut(&task_id).unwrap();
        task.state = TaskState::Succeeded;
        task.stage = "finished";
        inner.selections.remove(&task_id);
        inner.trim_history();
        inner.changed()
    };
    (shared.listener)(revision);
    Ok(())
}

pub(super) fn source_for_asset(
    shared: &Shared,
    asset_id: u64,
    path: &Path,
) -> Result<Arc<RgbaImage>, String> {
    {
        let inner = shared.inner.lock().unwrap();
        if let Some((cached_id, source)) = &inner.decoded_source {
            if *cached_id == asset_id {
                return Ok(Arc::clone(source));
            }
        }
    }
    let source = Arc::new(image_io::decode_image(path).map_err(|error| error.to_string())?);
    if source.as_raw().len() <= asset::SOURCE_CACHE_BUDGET {
        let mut inner = shared.inner.lock().unwrap();
        if inner.selected_asset_id == Some(asset_id)
            && inner.assets.get(&asset_id).is_some_and(|asset| {
                asset.source.as_path() == path
                    && asset.snapshot.state == AssetState::Ready
                    && asset.preview.is_some()
            })
        {
            inner.decoded_source = Some((asset_id, Arc::clone(&source)));
        }
    }
    Ok(source)
}

pub(super) fn run_automatic(
    shared: &Shared,
    session: &mut Option<(IsnetSession, ModelLease)>,
    task_id: u64,
    asset_id: u64,
    path: &Path,
) {
    let result = (|| -> Result<(), String> {
        let (runtime, store) = shared.inference.as_ref().ok_or("原生推理环境尚未配置")?;
        if session.is_none() {
            let lease = store.acquire("isnet-auto-high")?;
            let loaded =
                IsnetSession::load_verified(runtime, &lease).map_err(|error| error.to_string())?;
            lease.mark_session_ready();
            *session = Some((loaded, lease));
        }
        let revision = {
            let mut inner = shared.inner.lock().unwrap();
            if !task_is_current(&inner, task_id) {
                return Err("任务已失效".to_owned());
            }
            inner.tasks.get_mut(&task_id).unwrap().stage = "inference";
            inner.changed()
        };
        (shared.listener)(revision);
        let source = source_for_asset(shared, asset_id, path)?;
        let handle = session.as_ref().unwrap().0.cancel_handle();
        *shared.active_inference.lock().unwrap() = Some((task_id, handle.clone()));
        let mask_result = session.as_mut().unwrap().0.infer_mask(&source);
        *shared.active_inference.lock().unwrap() = None;
        if handle.is_requested() {
            *session = None;
        }
        let mask = mask_result.map_err(|error| error.to_string())?;
        commit_generated_mask(shared, task_id, mask, None)
    })();
    if let Err(error) = result {
        finish_task(shared, task_id, Err(error));
    }
}

pub(super) fn run_selection(
    shared: &Shared,
    cached: &mut Option<CachedSelection>,
    task_id: u64,
    asset_id: u64,
    generation: u64,
    path: &Path,
    points: Option<Vec<sam::Point>>,
) {
    let result = (|| -> Result<(), String> {
        let points = points.ok_or("缺少点选坐标")?;
        let (runtime, store) = shared.inference.as_ref().ok_or("原生推理环境尚未配置")?;
        if cached
            .as_ref()
            .is_none_or(|session| session.asset_id != asset_id || session.generation != generation)
        {
            *cached = None;
            let encoder = store.acquire("mobilesam-image-encoder")?;
            let decoder = store.acquire("mobilesam-mask-decoder")?;
            let source = image_io::decode_image(path).map_err(|error| error.to_string())?;
            let (session, _) =
                SelectionSession::load(runtime, encoder.path(), decoder.path(), &source)
                    .map_err(|error| error.to_string())?;
            encoder.mark_session_ready();
            decoder.mark_session_ready();
            *cached = Some(CachedSelection {
                asset_id,
                generation,
                session,
                _encoder: encoder,
                _decoder: decoder,
            });
        }
        let size = {
            let mut inner = shared.inner.lock().unwrap();
            if !task_is_current(&inner, task_id) {
                return Err("任务已失效".to_owned());
            }
            inner.tasks.get_mut(&task_id).unwrap().stage = "inference";
            let asset = &inner.assets[&asset_id];
            let size = (
                asset.snapshot.width.ok_or("图片尚未就绪")?,
                asset.snapshot.height.ok_or("图片尚未就绪")?,
            );
            let revision = inner.changed();
            drop(inner);
            (shared.listener)(revision);
            size
        };
        let (raw, _) = cached
            .as_mut()
            .unwrap()
            .session
            .select(&points, size)
            .map_err(|error| error.to_string())?;
        let mask = GrayImage::from_raw(size.0, size.1, raw).ok_or("点选蒙版尺寸无效")?;
        commit_generated_mask(shared, task_id, mask, Some(points))
    })();
    if let Err(error) = result {
        finish_task(shared, task_id, Err(error));
    }
}

pub(super) fn run_refinement(
    shared: &Shared,
    cached: &mut Option<(RefinementSession, ModelLease)>,
    task_id: u64,
    path: &Path,
    edge: Option<u32>,
) {
    let result = (|| -> Result<(), String> {
        let edge = edge.ok_or("缺少精修参数")?;
        let (runtime, store) = shared.inference.as_ref().ok_or("原生推理环境尚未配置")?;
        if cached.is_none() {
            let lease = store.acquire("vitmatte-high")?;
            let (session, _, _) = RefinementSession::load(runtime, lease.path())
                .map_err(|error| error.to_string())?;
            lease.mark_session_ready();
            *cached = Some((session, lease));
        }
        let coarse = {
            let mut inner = shared.inner.lock().unwrap();
            if !task_is_current(&inner, task_id) {
                return Err("任务已失效".to_owned());
            }
            let asset_id = inner.tasks[&task_id].asset_id;
            let coarse = inner.assets[&asset_id].mask.clone().ok_or("蒙版不存在")?;
            inner.tasks.get_mut(&task_id).unwrap().stage = "inference";
            let revision = inner.changed();
            drop(inner);
            (shared.listener)(revision);
            coarse
        };
        let source = image_io::decode_image(path).map_err(|error| error.to_string())?;
        let (refined, _) = cached
            .as_mut()
            .unwrap()
            .0
            .refine(&source, &coarse, edge)
            .map_err(|error| error.to_string())?;
        let mask = if refined.dimensions() == coarse.dimensions() {
            refined
        } else {
            let raw = matte::resize_mask(
                refined.as_raw(),
                refined.width(),
                refined.height(),
                coarse.dimensions(),
                false,
            )
            .map_err(str::to_owned)?;
            GrayImage::from_raw(coarse.width(), coarse.height(), raw).ok_or("精修蒙版尺寸无效")?
        };
        commit_generated_mask(shared, task_id, mask, None)
    })();
    if let Err(error) = result {
        finish_task(shared, task_id, Err(error));
    }
}

pub(super) fn run_export(
    shared: &Shared,
    task_id: u64,
    asset_id: u64,
    _generation: u64,
    _selection_generation: u64,
    _mask_revision: u64,
    source_path: &Path,
) {
    let result = (|| -> Result<(), String> {
        let (destination, mask, background, background_revision) = {
            let inner = shared.inner.lock().unwrap();
            if !task_is_current(&inner, task_id) {
                return Err("任务已失效".to_owned());
            }
            let destination = inner.exports.get(&task_id).ok_or("输出路径缺失")?.clone();
            if destination == source_path {
                return Err("不能覆盖原图".to_owned());
            }
            let mask = inner.assets[&asset_id]
                .mask
                .as_ref()
                .ok_or("蒙版不存在")?
                .clone();
            let asset = &inner.assets[&asset_id];
            (
                destination,
                mask,
                asset.background.clone(),
                asset.snapshot.background_revision,
            )
        };
        let source = source_for_asset(shared, asset_id, source_path)?;
        let cutout = mask::apply(&source, &mask).map_err(|error| error.to_string())?;
        let result = match &background {
            BackgroundState::Transparent => cutout,
            BackgroundState::Solid(rgb) => {
                mask::with_background(&cutout, mask::Background::Solid(*rgb))
                    .map_err(|error| error.to_string())?
            }
            BackgroundState::Image { path, .. } | BackgroundState::ImagePending { path, .. } => {
                let image = image_io::decode_image(path).map_err(|error| error.to_string())?;
                mask::with_background(&cutout, mask::Background::Image(&image))
                    .map_err(|error| error.to_string())?
            }
        };
        image_io::save_png_noclobber_checked(&result, &destination, || {
            let inner = shared.inner.lock().unwrap();
            if task_is_current(&inner, task_id)
                && inner
                    .assets
                    .get(&asset_id)
                    .is_some_and(|asset| asset.snapshot.background_revision == background_revision)
            {
                Ok(inner)
            } else {
                Err(std::io::Error::other("任务已失效").into())
            }
        })
        .map_err(|error| error.to_string())?;
        Ok(())
    })();
    finish_task(shared, task_id, result);
}
