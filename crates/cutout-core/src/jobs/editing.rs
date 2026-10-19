use super::*;

impl Workspace {
    /// Internal result commit API. No mask pixels are accepted over JSON IPC.
    /// A late worker can only publish against its captured selection, asset
    /// generation and mask revision. Masks have a separate fixed byte budget.
    pub fn publish_mask(
        &self,
        asset_id: u64,
        generation: u64,
        selection_generation: u64,
        expected_mask_revision: u64,
        mask: GrayImage,
    ) -> Result<bool, String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if inner.selected_asset_id != Some(asset_id)
                || inner.selection_generation != selection_generation
                || asset.snapshot.generation != generation
                || asset.snapshot.mask_revision != expected_mask_revision
            {
                return Ok(false);
            }
            if Some(mask.width()) != asset.snapshot.width
                || Some(mask.height()) != asset.snapshot.height
            {
                return Err("蒙版尺寸与资产不匹配".to_owned());
            }
            let redo_bytes: usize = asset.redo.iter().map(|value| value.as_raw().len()).sum();
            let dropped_bytes = asset
                .undo
                .first()
                .filter(|_| asset.undo.len() >= EDIT_HISTORY_LIMIT)
                .map_or(0, |value| value.as_raw().len());
            if inner.mask_bytes - redo_bytes - dropped_bytes + mask.as_raw().len() > MASK_BUDGET {
                return Err("蒙版内存预算不足".to_owned());
            }
            inner.mask_bytes = inner.mask_bytes - redo_bytes - dropped_bytes + mask.as_raw().len();
            let asset = inner.assets.get_mut(&asset_id).unwrap();
            if asset.undo.len() >= EDIT_HISTORY_LIMIT {
                asset.undo.remove(0);
            }
            if let Some(old) = asset.mask.take() {
                asset.undo.push(old);
            }
            asset.redo.clear();
            asset.mask = Some(mask);
            asset.snapshot.mask_revision += 1;
            asset.snapshot.can_undo = !asset.undo.is_empty();
            asset.snapshot.can_redo = false;
            inner.clear_result(asset_id);
            inner.changed()
        };
        (self.shared.listener)(revision);
        Ok(true)
    }

    /// Only an owned, currently cached preview revision can be served.
    pub fn preview_png(&self, asset_id: u64, revision: u64) -> Option<Arc<Vec<u8>>> {
        let mut inner = self.shared.inner.lock().unwrap();
        let asset = inner.assets.get(&asset_id)?;
        if asset.snapshot.preview_revision != revision {
            return None;
        }
        let bytes = Arc::clone(asset.preview.as_ref()?);
        inner.access_tick += 1;
        let tick = inner.access_tick;
        inner.assets.get_mut(&asset_id).unwrap().preview_access = tick;
        Some(bytes)
    }

    pub fn preview_revision(&self, asset_id: u64) -> Option<u64> {
        let inner = self.shared.inner.lock().unwrap();
        let asset = inner.assets.get(&asset_id)?;
        asset.preview.as_ref()?;
        Some(asset.snapshot.preview_revision)
    }

    pub fn result_preview_png(&self, asset_id: u64, result_revision: u64) -> Option<Arc<Vec<u8>>> {
        let mut inner = self.shared.inner.lock().unwrap();
        let asset = inner.assets.get(&asset_id)?;
        if asset.snapshot.result_revision != result_revision {
            return None;
        }
        let bytes = Arc::clone(asset.result_preview.as_ref()?);
        inner.access_tick += 1;
        let tick = inner.access_tick;
        inner.assets.get_mut(&asset_id).unwrap().preview_access = tick;
        Some(bytes)
    }

    pub fn result_revision(&self, asset_id: u64) -> Option<u64> {
        let inner = self.shared.inner.lock().unwrap();
        let asset = inner.assets.get(&asset_id)?;
        asset.result_preview.as_ref()?;
        Some(asset.snapshot.result_revision)
    }

    pub fn set_background(
        &self,
        asset_id: u64,
        expected_mask_revision: u64,
        expected_background_revision: u64,
        choice: BackgroundChoice,
    ) -> Result<bool, String> {
        let (
            original_png,
            active_preview,
            inactive_preview,
            old_image_background,
            generation,
            selection_generation,
        ) = {
            let inner = self.shared.inner.lock().unwrap();
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if inner.selected_asset_id != Some(asset_id)
                || asset.snapshot.mask_revision != expected_mask_revision
                || asset.snapshot.background_revision != expected_background_revision
            {
                return Ok(false);
            }
            if asset.snapshot.state != AssetState::Ready {
                return Err("图片预览尚未就绪".to_owned());
            }
            (
                Arc::clone(asset.preview.as_ref().ok_or("预览已释放")?),
                asset.result_preview.clone(),
                asset.inactive.result_preview.clone(),
                matches!(
                    asset.background,
                    BackgroundState::Image { .. } | BackgroundState::ImagePending { .. }
                ),
                asset.snapshot.generation,
                inner.selection_generation,
            )
        };
        let background = match choice {
            BackgroundChoice::Transparent => BackgroundState::Transparent,
            BackgroundChoice::Solid { rgb } => BackgroundState::Solid(rgb),
            BackgroundChoice::Image { path } => {
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .ok_or("背景图片路径无效")?
                    .to_owned();
                let original =
                    image::load_from_memory_with_format(&original_png, image::ImageFormat::Png)
                        .map_err(|error| error.to_string())?;
                let image = image_io::decode_image(&path).map_err(|error| error.to_string())?;
                let preview = image::DynamicImage::ImageRgba8(image)
                    .resize_to_fill(
                        original.width(),
                        original.height(),
                        image::imageops::FilterType::CatmullRom,
                    )
                    .to_rgba8();
                BackgroundState::Image {
                    path,
                    display_name: name,
                    preview: Arc::new(preview),
                }
            }
        };
        let render = |mask: Option<&GrayImage>| -> Result<Option<Vec<u8>>, String> {
            mask.map(|value| {
                asset::build_result_preview(&original_png, value, background.preview())
                    .map_err(|error| error.to_string())
            })
            .transpose()
        };
        let reuse_cutout = !old_image_background
            && matches!(
                background,
                BackgroundState::Transparent | BackgroundState::Solid(_)
            );
        let (active_png, inactive_png) = if reuse_cutout {
            (active_preview, inactive_preview)
        } else {
            let (active_mask, inactive_mask) = {
                let inner = self.shared.inner.lock().unwrap();
                let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
                (
                    asset
                        .result_preview
                        .as_ref()
                        .and_then(|_| asset.mask.clone()),
                    asset
                        .inactive
                        .result_preview
                        .as_ref()
                        .and_then(|_| asset.inactive.mask.clone()),
                )
            };
            (
                render(active_mask.as_ref())?.map(Arc::new),
                render(inactive_mask.as_ref())?.map(Arc::new),
            )
        };
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if inner.selected_asset_id != Some(asset_id)
                || inner.selection_generation != selection_generation
                || asset.snapshot.generation != generation
                || asset.snapshot.mask_revision != expected_mask_revision
                || asset.snapshot.background_revision != expected_background_revision
            {
                return Ok(false);
            }
            let old_bytes = asset.background.bytes()
                + asset.result_preview.as_ref().map_or(0, |value| value.len())
                + asset
                    .inactive
                    .result_preview
                    .as_ref()
                    .map_or(0, |value| value.len());
            let new_bytes = background.bytes()
                + active_png.as_ref().map_or(0, |value| value.len())
                + inactive_png.as_ref().map_or(0, |value| value.len());
            if inner.preview_bytes - old_bytes + new_bytes > PREVIEW_BUDGET {
                return Err("背景或结果预览超过内存预算".to_owned());
            }
            inner.preview_bytes = inner.preview_bytes - old_bytes + new_bytes;
            let asset = inner.assets.get_mut(&asset_id).unwrap();
            asset.snapshot.background = background.view();
            asset.background = background;
            asset.result_preview = active_png;
            asset.inactive.result_preview = inactive_png;
            asset.snapshot.background_revision += 1;
            asset.snapshot.result_revision += 1;
            inner.changed()
        };
        (self.shared.listener)(revision);
        Ok(true)
    }

    pub fn auto_cutout(&self, asset_id: u64) -> Result<u64, String> {
        if self.shared.inference.is_none() {
            return Err("原生推理环境尚未配置".to_owned());
        }
        self.switch_workflow(asset_id, Workflow::Automatic)?;
        self.enqueue(asset_id, TaskRequest::Automatic)
    }

    pub fn select_points(&self, asset_id: u64, points: Vec<sam::Point>) -> Result<u64, String> {
        if self.shared.inference.is_none() {
            return Err("原生推理环境尚未配置".to_owned());
        }
        {
            let inner = self.shared.inner.lock().unwrap();
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            sam::prompt(
                &points,
                asset.snapshot.width.ok_or("图片尚未就绪")?,
                asset.snapshot.height.ok_or("图片尚未就绪")?,
            )
            .map_err(|error| error.to_owned())?;
        }
        self.switch_workflow(asset_id, Workflow::Selection)?;
        self.enqueue(asset_id, TaskRequest::Selection(points))
    }

    pub fn refine_mask(
        &self,
        asset_id: u64,
        expected_revision: u64,
        edge: u32,
    ) -> Result<u64, String> {
        if self.shared.inference.is_none() {
            return Err("原生推理环境尚未配置".to_owned());
        }
        if edge > 1024 {
            return Err("精修边缘宽度无效".to_owned());
        }
        self.enqueue(
            asset_id,
            TaskRequest::Refinement {
                expected_revision,
                edge,
            },
        )
    }

    pub fn paint_mask(
        &self,
        asset_id: u64,
        expected_revision: u64,
        stroke: Stroke,
    ) -> Result<bool, String> {
        let (mut next, preview, background, background_revision, generation, selection_generation) = {
            let inner = self.shared.inner.lock().unwrap();
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if inner.selected_asset_id != Some(asset_id)
                || asset.snapshot.mask_revision != expected_revision
            {
                return Ok(false);
            }
            (
                asset.mask.clone().ok_or("请先完成抠图")?,
                Arc::clone(asset.preview.as_ref().ok_or("预览已释放")?),
                asset.background.clone(),
                asset.snapshot.background_revision,
                asset.snapshot.generation,
                inner.selection_generation,
            )
        };
        edit::paint(&mut next, &stroke).map_err(str::to_owned)?;
        let rendered = asset::build_result_preview(&preview, &next, background.preview())
            .map_err(|error| error.to_string())?;
        self.commit_edit(
            asset_id,
            expected_revision,
            background_revision,
            generation,
            selection_generation,
            next,
            rendered,
        )
    }

    pub fn step_history(
        &self,
        asset_id: u64,
        expected_revision: u64,
        redo: bool,
    ) -> Result<bool, String> {
        let (next, preview, background, background_revision, generation, selection_generation) = {
            let inner = self.shared.inner.lock().unwrap();
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if inner.selected_asset_id != Some(asset_id)
                || asset.snapshot.mask_revision != expected_revision
            {
                return Ok(false);
            }
            let stack = if redo { &asset.redo } else { &asset.undo };
            (
                stack.last().cloned().ok_or("没有可恢复的编辑")?,
                Arc::clone(asset.preview.as_ref().ok_or("预览已释放")?),
                asset.background.clone(),
                asset.snapshot.background_revision,
                asset.snapshot.generation,
                inner.selection_generation,
            )
        };
        let rendered = asset::build_result_preview(&preview, &next, background.preview())
            .map_err(|error| error.to_string())?;
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if inner.selected_asset_id != Some(asset_id)
                || inner.selection_generation != selection_generation
                || asset.snapshot.generation != generation
                || asset.snapshot.mask_revision != expected_revision
                || asset.snapshot.background_revision != background_revision
            {
                return Ok(false);
            }
            let old_preview = asset.result_preview.as_ref().map_or(0, |value| value.len());
            if inner.preview_bytes - old_preview + rendered.len() > PREVIEW_BUDGET {
                return Err("结果预览超过内存预算".to_owned());
            }
            inner.preview_bytes = inner.preview_bytes - old_preview + rendered.len();
            let asset = inner.assets.get_mut(&asset_id).unwrap();
            let next = if redo {
                asset.redo.pop()
            } else {
                asset.undo.pop()
            }
            .unwrap();
            let old = asset.mask.replace(next).ok_or("蒙版不存在")?;
            if redo {
                asset.undo.push(old);
            } else {
                asset.redo.push(old);
            }
            asset.result_preview = Some(Arc::new(rendered));
            asset.snapshot.result_available = true;
            asset.snapshot.mask_revision += 1;
            asset.snapshot.result_revision += 1;
            asset.snapshot.can_undo = !asset.undo.is_empty();
            asset.snapshot.can_redo = !asset.redo.is_empty();
            inner.changed()
        };
        (self.shared.listener)(revision);
        Ok(true)
    }

    pub(super) fn commit_edit(
        &self,
        asset_id: u64,
        expected_revision: u64,
        background_revision: u64,
        generation: u64,
        selection_generation: u64,
        next: GrayImage,
        rendered: Vec<u8>,
    ) -> Result<bool, String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let asset = inner.assets.get(&asset_id).ok_or("资产不存在")?;
            if inner.selected_asset_id != Some(asset_id)
                || inner.selection_generation != selection_generation
                || asset.snapshot.generation != generation
                || asset.snapshot.mask_revision != expected_revision
                || asset.snapshot.background_revision != background_revision
            {
                return Ok(false);
            }
            let discarded = asset
                .redo
                .iter()
                .map(|mask| mask.as_raw().len())
                .sum::<usize>()
                + asset
                    .undo
                    .first()
                    .filter(|_| asset.undo.len() >= EDIT_HISTORY_LIMIT)
                    .map_or(0, |mask| mask.as_raw().len());
            let old_mask_len = asset.mask.as_ref().ok_or("蒙版不存在")?.as_raw().len();
            if inner.mask_bytes - discarded + old_mask_len > MASK_BUDGET
                || inner.preview_bytes
                    - asset.result_preview.as_ref().map_or(0, |value| value.len())
                    + rendered.len()
                    > PREVIEW_BUDGET
            {
                return Err("编辑历史或预览超过内存预算".to_owned());
            }
            let old_preview = asset.result_preview.as_ref().map_or(0, |value| value.len());
            inner.mask_bytes = inner.mask_bytes - discarded + old_mask_len;
            inner.preview_bytes = inner.preview_bytes - old_preview + rendered.len();
            let asset = inner.assets.get_mut(&asset_id).unwrap();
            if asset.undo.len() >= EDIT_HISTORY_LIMIT {
                asset.undo.remove(0);
            }
            let previous = asset.mask.replace(next).ok_or("蒙版不存在")?;
            asset.undo.push(previous);
            asset.redo.clear();
            asset.result_preview = Some(Arc::new(rendered));
            asset.snapshot.mask_revision += 1;
            asset.snapshot.result_revision += 1;
            asset.snapshot.result_available = true;
            asset.snapshot.can_undo = true;
            asset.snapshot.can_redo = false;
            inner.changed()
        };
        (self.shared.listener)(revision);
        Ok(true)
    }
}
