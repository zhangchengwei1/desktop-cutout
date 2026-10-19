use super::*;

pub(super) fn finish_batch_item(
    shared: &Shared,
    item_id: u64,
    result: Result<SavedBatchOutput, String>,
) {
    let revision = {
        let mut inner = shared.inner.lock().unwrap();
        let Some(item) = inner.batch_items.get_mut(&item_id) else {
            return;
        };
        let reserved = std::mem::take(&mut item.reserved_bytes);
        let pause_batch = result.as_ref().err().is_some_and(|error| {
            item.snapshot.state != TaskState::CancelRequested
                && crate::runtime::is_unavailable(error)
        });
        let batch_id = item.snapshot.batch_id;
        match result {
            Ok(output) => {
                item.snapshot.state = TaskState::Succeeded;
                item.snapshot.error = None;
                item.output = Some(output);
            }
            Err(_) if item.snapshot.state == TaskState::CancelRequested => {
                item.snapshot.state = TaskState::Cancelled;
            }
            Err(error) => {
                item.snapshot.state = TaskState::Failed;
                item.snapshot.error = Some(error);
            }
        }
        item.snapshot.stage = "finished";
        if pause_batch {
            if let Some(batch) = inner.batches.get_mut(&batch_id) {
                batch.paused = true;
            }
            // Release this batch's prefetched pixels so a paused failure cannot
            // occupy the global decoder slot or memory budget indefinitely.
            let mut pending = VecDeque::new();
            for (id, image) in std::mem::take(&mut inner.batch_prepared) {
                if inner.batch_items[&id].snapshot.batch_id == batch_id {
                    requeue_decoded_batch_item(&mut inner, id);
                    drop(image);
                } else {
                    pending.push_back((id, image));
                }
            }
            inner.batch_prepared = pending;
        }
        inner.batch_inflight_bytes -= reserved;
        inner.changed()
    };
    shared.wake.notify_all();
    (shared.listener)(revision);
}

pub(super) fn requeue_decoded_batch_item(inner: &mut Inner, item_id: u64) {
    let item = inner.batch_items.get_mut(&item_id).unwrap();
    inner.batch_inflight_bytes -= std::mem::take(&mut item.reserved_bytes);
    item.snapshot.state = TaskState::Queued;
    item.snapshot.stage = "queued";
    item.timings.prepare_us = None;
    inner.batch_queue.push_back(item_id);
}

pub(super) fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

pub(super) fn run_batch_decoder(shared: Arc<Shared>) {
    loop {
        let (item_id, source_path, revision) = {
            let mut inner = shared.inner.lock().unwrap();
            loop {
                if inner.shutdown {
                    return;
                }
                if inner.batch_prepared.len() >= BATCH_PREPARED_LIMIT {
                    inner = shared.wake.wait(inner).unwrap();
                    continue;
                }
                let next = inner.batch_queue.iter().position(|id| {
                    let Some(item) = inner.batch_items.get(id) else {
                        return false;
                    };
                    item.snapshot.state == TaskState::Queued
                        && inner
                            .batches
                            .get(&item.snapshot.batch_id)
                            .is_some_and(|batch| !batch.paused)
                });
                let Some(index) = next else {
                    inner = shared.wake.wait(inner).unwrap();
                    continue;
                };
                let item_id = inner.batch_queue.remove(index).unwrap();
                let item = inner.batch_items.get_mut(&item_id).unwrap();
                item.snapshot.state = TaskState::Running;
                item.snapshot.stage = "probing";
                let source_path = item.source.clone();
                break (item_id, source_path, inner.changed());
            }
        };
        (shared.listener)(revision);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prepare_batch_item(&shared, item_id, &source_path)
        }))
        .unwrap_or_else(|_| Err("批量解码处理异常".to_owned()));
        if let Err(error) = outcome {
            finish_batch_item(&shared, item_id, Err(error));
        }
    }
}

pub(super) fn prepare_batch_item(shared: &Shared, item_id: u64, path: &Path) -> Result<(), String> {
    let started = Instant::now();
    let origin = shared.inner.lock().unwrap().batch_items[&item_id]
        .origin
        .clone();
    if let SavedBatchSource::File {
        path: original,
        identity,
    } = &origin
    {
        verify_batch_file_source(original, identity.as_ref())?;
    }
    let reserve = image_io::batch_inflight_bytes(path).map_err(|error| error.to_string())?;
    if reserve > BATCH_INFLIGHT_BUDGET {
        return Err("图片超过批量在途内存预算".to_owned());
    }
    let revision = {
        let mut inner = shared.inner.lock().unwrap();
        loop {
            if inner.shutdown || inner.batch_items[&item_id].snapshot.state != TaskState::Running {
                return Err("批量项已取消".to_owned());
            }
            if inner.batch_inflight_bytes <= BATCH_INFLIGHT_BUDGET - reserve {
                inner.batch_inflight_bytes += reserve;
                let item = inner.batch_items.get_mut(&item_id).unwrap();
                item.reserved_bytes = reserve;
                item.snapshot.stage = "decoding";
                break inner.changed();
            }
            inner = shared.wake.wait(inner).unwrap();
        }
    };
    (shared.listener)(revision);
    let source = image_io::decode_image(path).map_err(|error| error.to_string())?;
    if let SavedBatchSource::File {
        path: original,
        identity,
    } = &origin
    {
        verify_batch_file_source(original, identity.as_ref())?;
    }
    let revision = {
        let mut inner = shared.inner.lock().unwrap();
        if inner.shutdown || inner.batch_items[&item_id].snapshot.state != TaskState::Running {
            return Err("批量项已取消".to_owned());
        }
        let batch_id = inner.batch_items[&item_id].snapshot.batch_id;
        if inner.batches[&batch_id].paused {
            requeue_decoded_batch_item(&mut inner, item_id);
        } else {
            let item = inner.batch_items.get_mut(&item_id).unwrap();
            item.snapshot.stage = "ready_for_inference";
            item.timings.prepare_us = Some(elapsed_us(started));
            inner.batch_prepared.push_back((item_id, source));
        }
        inner.changed()
    };
    shared.wake.notify_all();
    (shared.listener)(revision);
    Ok(())
}

pub(super) fn run_batch_inference(
    shared: &Shared,
    session: &mut Option<(IsnetSession, ModelLease)>,
    item_id: u64,
    source: RgbaImage,
) {
    #[cfg(test)]
    if shared.batch_infer.lock().unwrap().is_some() {
        let result = (|| -> Result<(), String> {
            let revision = {
                let mut inner = shared.inner.lock().unwrap();
                let item = &mut inner
                    .batch_items
                    .get_mut(&item_id)
                    .ok_or("批量项不存在")?
                    .snapshot;
                if item.state != TaskState::Running {
                    return Err("批量项已取消".to_owned());
                }
                item.stage = "inference";
                inner.changed()
            };
            (shared.listener)(revision);
            let started = Instant::now();
            let mask = shared.batch_infer.lock().unwrap().as_ref().unwrap()(&source)?;
            let mut inner = shared.inner.lock().unwrap();
            let timings = &mut inner.batch_items.get_mut(&item_id).unwrap().timings;
            timings.inference_us = Some(elapsed_us(started));
            timings.model_load_us = Some(0);
            drop(inner);
            compose_and_queue_batch_output(shared, item_id, &source, mask)
        })();
        if let Err(error) = result {
            finish_batch_item(shared, item_id, Err(error));
        }
        return;
    }
    let result = (|| -> Result<(), String> {
        {
            let inner = shared.inner.lock().unwrap();
            let item = inner.batch_items.get(&item_id).ok_or("批量项不存在")?;
            if item.snapshot.state != TaskState::Running {
                return Err("批量项已取消".to_owned());
            }
        }
        let (runtime, store) = shared.inference.as_ref().ok_or("原生推理环境尚未配置")?;
        let model_load_us = if session.is_none() {
            let started = Instant::now();
            let lease = store.acquire("isnet-auto-high")?;
            let loaded =
                IsnetSession::load_verified(runtime, &lease).map_err(|error| error.to_string())?;
            lease.mark_session_ready();
            *session = Some((loaded, lease));
            elapsed_us(started)
        } else {
            0
        };
        shared
            .inner
            .lock()
            .unwrap()
            .batch_items
            .get_mut(&item_id)
            .unwrap()
            .timings
            .model_load_us = Some(model_load_us);
        let revision = {
            let mut inner = shared.inner.lock().unwrap();
            let item = &mut inner
                .batch_items
                .get_mut(&item_id)
                .ok_or("批量项不存在")?
                .snapshot;
            if item.state != TaskState::Running {
                return Err("批量项已取消".to_owned());
            }
            item.stage = "inference";
            inner.changed()
        };
        (shared.listener)(revision);
        let handle = session.as_ref().unwrap().0.cancel_handle();
        *shared.active_inference.lock().unwrap() = Some((item_id, handle.clone()));
        let inference_started = Instant::now();
        let mask_result = session.as_mut().unwrap().0.infer_mask(&source);
        shared
            .inner
            .lock()
            .unwrap()
            .batch_items
            .get_mut(&item_id)
            .unwrap()
            .timings
            .inference_us = Some(elapsed_us(inference_started));
        *shared.active_inference.lock().unwrap() = None;
        if handle.is_requested() {
            *session = None;
        }
        let mask = mask_result.map_err(|error| error.to_string())?;
        compose_and_queue_batch_output(shared, item_id, &source, mask)
    })();
    if let Err(error) = result {
        finish_batch_item(shared, item_id, Err(error));
    }
}

pub(super) fn compose_and_queue_batch_output(
    shared: &Shared,
    item_id: u64,
    source: &RgbaImage,
    mask: GrayImage,
) -> Result<(), String> {
    let revision = {
        let mut inner = shared.inner.lock().unwrap();
        let item = &mut inner
            .batch_items
            .get_mut(&item_id)
            .ok_or("批量项不存在")?
            .snapshot;
        if item.state != TaskState::Running {
            return Err("批量项已取消".to_owned());
        }
        item.stage = "composing";
        inner.changed()
    };
    (shared.listener)(revision);
    let started = Instant::now();
    let cutout = mask::apply(&source, &mask).map_err(|error| error.to_string())?;
    let revision = {
        let mut inner = shared.inner.lock().unwrap();
        loop {
            if inner.shutdown || inner.batch_items[&item_id].snapshot.state != TaskState::Running {
                return Err("批量项已取消".to_owned());
            }
            if inner.batch_writes.len() < BATCH_WRITE_LIMIT {
                let item = inner.batch_items.get_mut(&item_id).unwrap();
                item.snapshot.stage = "waiting_to_write";
                item.timings.compose_queue_us = Some(elapsed_us(started));
                inner.batch_writes.push_back((item_id, cutout));
                break inner.changed();
            }
            inner = shared.wake.wait(inner).unwrap();
        }
    };
    shared.wake.notify_all();
    (shared.listener)(revision);
    Ok(())
}

pub(super) fn run_batch_writer(shared: Arc<Shared>) {
    loop {
        let (item_id, image, destination, revision) = {
            let mut inner = shared.inner.lock().unwrap();
            loop {
                if inner.shutdown {
                    return;
                }
                let Some((item_id, image)) = inner.batch_writes.pop_front() else {
                    inner = shared.wake.wait(inner).unwrap();
                    continue;
                };
                let item = inner.batch_items.get_mut(&item_id).unwrap();
                item.snapshot.stage = "encoding";
                let destination = item.destination.clone();
                shared.wake.notify_all();
                break (item_id, image, destination, inner.changed());
            }
        };
        (shared.listener)(revision);
        let started = Instant::now();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            image_io::save_png_noclobber_checked_hashed(&image, &destination, || {
                let inner = shared.inner.lock().unwrap();
                if inner
                    .batch_items
                    .get(&item_id)
                    .is_some_and(|item| item.snapshot.state == TaskState::Running)
                {
                    Ok(inner)
                } else {
                    Err(std::io::Error::other("批量项已取消").into())
                }
            })
            .map(|(sha256, bytes)| SavedBatchOutput { sha256, bytes })
            .map_err(|error| error.to_string())
        }))
        .unwrap_or_else(|_| Err("批量写盘处理异常".to_owned()));
        if let Some(item) = shared.inner.lock().unwrap().batch_items.get_mut(&item_id) {
            item.timings.write_us = Some(elapsed_us(started));
        }
        finish_batch_item(&shared, item_id, outcome);
    }
}
