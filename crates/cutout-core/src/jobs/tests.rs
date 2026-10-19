use super::*;
use image::Luma;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

fn wait_for(
    workspace: &Workspace,
    predicate: impl Fn(&WorkspaceSnapshot) -> bool,
) -> WorkspaceSnapshot {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = workspace.snapshot();
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(
            Instant::now() < until,
            "workspace worker timed out: {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn runtime_failure_pauses_prefetched_jobs_and_retry_preserves_saved_results() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let output = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let worker_calls = Arc::clone(&calls);
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(move |image| {
        if worker_calls.fetch_add(1, Ordering::SeqCst) == 1 {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            return Err("[runtime_unavailable] test-only load failure".to_owned());
        }
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace
        .create_batch(output.path(), &vec![fixture.clone(); 4])
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !workspace
        .batch_items(batch_id, 0, 4)
        .unwrap()
        .items
        .iter()
        .any(|item| item.stage == "ready_for_inference")
    {
        assert!(Instant::now() < until, "decoder did not prefetch");
        thread::sleep(Duration::from_millis(10));
    }
    release_tx.send(()).unwrap();
    let paused = wait_for(&workspace, |snapshot| {
        let batch = &snapshot.batches[0];
        batch.paused && batch.failed == 1 && batch.succeeded == 1
    });
    assert_eq!(paused.batches[0].queued + paused.batches[0].running, 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let other_output = tempfile::tempdir().unwrap();
    let other_batch = workspace
        .create_batch(other_output.path(), &[fixture])
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .batches
            .iter()
            .any(|batch| batch.batch_id == other_batch && batch.succeeded == 1)
    });
    let saved = workspace
        .batch_items(batch_id, 0, 4)
        .unwrap()
        .items
        .into_iter()
        .find(|item| item.state == TaskState::Succeeded)
        .unwrap();
    let saved_path = output.path().join(&saved.output_name);
    let saved_modified = std::fs::metadata(&saved_path).unwrap().modified().unwrap();
    assert_eq!(workspace.retry_failed_batch(batch_id).unwrap(), 1);
    assert_eq!(workspace.retry_failed_batch(batch_id).unwrap(), 0);
    wait_for(&workspace, |snapshot| snapshot.batches[0].succeeded == 4);
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    assert_eq!(
        std::fs::metadata(saved_path).unwrap().modified().unwrap(),
        saved_modified
    );
}

#[test]
fn invalid_output_folder_is_rejected_before_accepting_batch() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let workspace = Workspace::new(|_| {}).unwrap();
    let error = workspace
        .create_batch(&directory.path().join("missing"), &[fixture])
        .unwrap_err();
    assert!(error.starts_with("[output_unavailable]"));
    assert!(workspace.snapshot().batches.is_empty());
}

#[test]
fn folder_batch_processes_nested_images_isolates_corruption_and_appends_safely() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("nested");
    let output = root.path().join("results");
    std::fs::create_dir(&nested).unwrap();
    std::fs::create_dir(&output).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    std::fs::copy(&fixture, root.path().join("same.PNG")).unwrap();
    std::fs::copy(&fixture, nested.join("same.png")).unwrap();
    std::fs::write(nested.join("broken.jpg"), b"not an image").unwrap();
    std::fs::write(root.path().join("notes.txt"), b"not an input").unwrap();
    std::fs::write(output.join("saved.png"), b"existing result").unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(|image| {
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let imported = workspace
        .create_batch_from_folder(&output, root.path())
        .unwrap();
    assert_eq!((imported.added, imported.skipped), (3, 1));
    wait_for(&workspace, |snapshot| {
        snapshot.batches[0].succeeded == 2 && snapshot.batches[0].failed == 1
    });
    assert!(!workspace.snapshot().batches[0].paused);
    let items = workspace
        .batch_items(imported.batch_id, 0, 10)
        .unwrap()
        .items;
    let names: std::collections::HashSet<_> = items.iter().map(|item| &item.output_name).collect();
    assert_eq!(names.len(), 3);
    assert_eq!(
        std::fs::read(output.join("saved.png")).unwrap(),
        b"existing result"
    );
    let saved = items
        .iter()
        .find(|item| item.state == TaskState::Succeeded)
        .unwrap();
    let saved_path = output.join(&saved.output_name);
    let saved_modified = std::fs::metadata(&saved_path).unwrap().modified().unwrap();
    let added = workspace
        .append_batch_folder(imported.batch_id, &nested)
        .unwrap();
    assert_eq!(added.added, 2);
    wait_for(&workspace, |snapshot| {
        snapshot.batches[0].succeeded == 3 && snapshot.batches[0].failed == 2
    });
    assert_eq!(
        std::fs::metadata(saved_path).unwrap().modified().unwrap(),
        saved_modified
    );
    let empty = tempfile::tempdir().unwrap();
    assert!(workspace
        .create_batch_from_folder(&output, empty.path())
        .unwrap_err()
        .starts_with("[folder_empty]"));
    assert_eq!(workspace.snapshot().batches.len(), 1);
}

#[test]
fn batch_admission_pause_cancel_retry_and_paging_share_the_workspace_worker() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.png");
    let output = directory.path().join("output");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/images/hard-edge-opaque.png"),
        &input,
    )
    .unwrap();
    std::fs::create_dir(&output).unwrap();
    std::fs::write(output.join("input-cutout.png"), b"existing output").unwrap();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let workspace = Workspace::with_processor(
        |_| {},
        move |_| {
            started_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            Ok(Preview {
                source_size: (2, 2),
                png: vec![1],
                source: None,
            })
        },
    )
    .unwrap();
    workspace.import(&input).unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let batch_id = workspace.create_batch(&output, &[input.clone()]).unwrap();
    assert_eq!(
        workspace
            .append_batch(batch_id, &vec![input.clone(); 99])
            .unwrap(),
        100
    );
    workspace.set_batch_paused(batch_id, true).unwrap();
    let summary = &workspace.snapshot().batches[0];
    assert_eq!(summary.total, 100);
    assert_eq!(summary.queued + summary.running, 100);
    assert!(summary.running <= 1);
    let first_page = workspace.batch_items(batch_id, 0, 2).unwrap();
    assert_eq!(first_page.total, 100);
    assert_eq!(first_page.items[0].output_name, "input-cutout-2.png");
    assert_eq!(first_page.items[1].output_name, "input-cutout-3.png");
    assert_eq!(
        workspace.batch_items(batch_id, 99, 2).unwrap().items.len(),
        1
    );
    assert!(workspace.batch_items(batch_id, 0, 201).is_err());
    assert_eq!(
        workspace
            .filtered_batch_items(batch_id, 99, 2, BatchFilter::Active)
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        workspace
            .filtered_batch_items(batch_id, 0, 2, BatchFilter::Issues)
            .unwrap()
            .total,
        0
    );
    assert!(workspace.remove_batch(batch_id).is_err());
    workspace.cancel_batch(batch_id).unwrap();
    release_tx.send(()).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready && snapshot.batches[0].cancelled == 100
    });
    let first_id = first_page.items[0].item_id;
    let issues = workspace
        .filtered_batch_items(batch_id, 99, 2, BatchFilter::Issues)
        .unwrap();
    assert_eq!(issues.total, 100);
    assert_eq!(issues.items.len(), 1);
    workspace.retry_batch_item(first_id).unwrap();
    assert_eq!(workspace.snapshot().batches[0].queued, 1);
    let active = workspace
        .filtered_batch_items(batch_id, 0, 2, BatchFilter::Active)
        .unwrap();
    assert_eq!(active.total, 1);
    assert_eq!(active.items[0].item_id, first_id);
    assert_eq!(
        workspace
            .filtered_batch_items(batch_id, 0, 2, BatchFilter::Issues)
            .unwrap()
            .total,
        99
    );
    assert_eq!(
        workspace
            .filtered_batch_items(batch_id, 0, 2, BatchFilter::Completed)
            .unwrap()
            .total,
        0
    );
    let retry_timing = &workspace.batch_timings(batch_id).unwrap()[0];
    assert!(retry_timing.prepare_us.is_none());
    assert!(retry_timing.inference_us.is_none());
    assert!(retry_timing.write_us.is_none());
    workspace.set_batch_paused(batch_id, false).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].failed == 1);
    assert!(!output.join("input-cutout-2.png").exists());
    assert_eq!(
        std::fs::read(output.join("input-cutout.png")).unwrap(),
        b"existing output"
    );
    workspace.remove_batch(batch_id).unwrap();
    assert!(workspace.snapshot().batches.is_empty());
    assert!(workspace.batch_items(batch_id, 0, 1).is_err());
}

#[test]
fn batch_original_review_keeps_queue_and_output_files() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let output = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    let batch_id = workspace.create_batch(output.path(), &[fixture]).unwrap();
    workspace.cancel_batch(batch_id).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].cancelled == 1);
    let item = workspace
        .batch_items(batch_id, 0, 1)
        .unwrap()
        .items
        .remove(0);
    let exported = output.path().join(&item.output_name);
    std::fs::write(&exported, b"user output").unwrap();
    let imported = workspace.review_batch_item(item.item_id).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .assets
            .iter()
            .any(|asset| asset.asset_id == imported.asset_id && asset.state == AssetState::Ready)
    });
    let snapshot = workspace.snapshot();
    assert_eq!(snapshot.selected_asset_id, Some(imported.asset_id));
    assert_eq!(snapshot.batches[0].total, 1);
    assert_eq!(snapshot.batches[0].cancelled, 1);
    assert_eq!(std::fs::read(exported).unwrap(), b"user output");
    assert!(workspace.review_batch_item(u64::MAX).is_err());
}

#[test]
fn batch_result_preview_uses_committed_alpha_and_handles_missing_or_changed_files() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("original.png");
    let image = RgbaImage::from_pixel(1536, 768, image::Rgba([90, 120, 160, 255]));
    image.save(&source).unwrap();
    let output = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(|image| {
        Ok(GrayImage::from_fn(image.width(), image.height(), |x, _| {
            Luma([if x < image.width() / 2 { 0 } else { 255 }])
        }))
    }));
    let batch_id = workspace
        .create_paused_batch(output.path(), &[source.clone()])
        .unwrap();
    let item = workspace
        .batch_items(batch_id, 0, 1)
        .unwrap()
        .items
        .remove(0);
    assert!(!workspace.batch_result_available(item.item_id));
    assert!(workspace.batch_result_preview_png(item.item_id).is_err());
    workspace.set_batch_paused(batch_id, false).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].succeeded == 1);
    let png = workspace.batch_result_preview_png(item.item_id).unwrap();
    let preview = image::load_from_memory(&png).unwrap().into_rgba8();
    assert_eq!(preview.dimensions(), (1024, 512));
    assert_eq!(preview.get_pixel(100, 100)[3], 0);
    assert_eq!(preview.get_pixel(900, 100)[3], 255);
    assert!(workspace.snapshot().assets.is_empty());
    assert_eq!(image_io::decode_image(&source).unwrap(), image);
    let path = output.path().join(item.output_name);
    let saved = std::fs::read(&path).unwrap();
    image.save(&path).unwrap();
    assert!(workspace.batch_result_preview_png(item.item_id).is_err());
    std::fs::write(&path, &saved).unwrap();
    assert!(workspace.batch_result_preview_png(item.item_id).is_ok());
    std::fs::remove_file(&path).unwrap();
    assert!(workspace.batch_result_preview_png(item.item_id).is_err());
    workspace.remove_batch(batch_id).unwrap();
    assert!(!workspace.batch_result_available(item.item_id));
    assert!(workspace.batch_result_preview_png(item.item_id).is_err());
}

#[test]
fn batch_pipeline_prefetches_one_image_and_writes_each_result_without_retaining_pixels() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let source = image_io::decode_image(&fixture).unwrap();
    let output = tempfile::tempdir().unwrap();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let calls = AtomicUsize::new(0);
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(move |image| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace
        .create_batch(
            output.path(),
            &[fixture.clone(), fixture.clone(), fixture.clone()],
        )
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let inner = workspace.shared.inner.lock().unwrap();
        if inner.batch_prepared.len() == 1 {
            assert!(inner.batch_inflight_bytes > 0);
            assert!(inner.batch_inflight_bytes <= BATCH_INFLIGHT_BUDGET);
            break;
        }
        drop(inner);
        assert!(Instant::now() < until, "解码未与推理重叠");
        thread::sleep(Duration::from_millis(10));
    }
    release_tx.send(()).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].succeeded == 3);
    for timing in workspace.batch_timings(batch_id).unwrap() {
        assert!(timing.prepare_us.is_some());
        assert_eq!(timing.model_load_us, Some(0));
        assert!(timing.inference_us.is_some());
        assert!(timing.compose_queue_us.is_some());
        assert!(timing.write_us.is_some());
    }
    let page = workspace.batch_items(batch_id, 0, 3).unwrap();
    for item in page.items {
        let result = image_io::decode_image(&output.path().join(item.output_name)).unwrap();
        assert_eq!(result, source);
    }
    let inner = workspace.shared.inner.lock().unwrap();
    assert_eq!(inner.batch_inflight_bytes, 0);
    assert!(inner.batch_prepared.is_empty());
    assert!(inner.batch_writes.is_empty());
}

#[test]
fn cancelling_prefetched_batch_items_never_publishes_a_late_png() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let output = tempfile::tempdir().unwrap();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(move |image| {
        started_tx.send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace
        .create_batch(output.path(), &[fixture.clone(), fixture.clone(), fixture])
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    workspace.cancel_batch(batch_id).unwrap();
    release_tx.send(()).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].cancelled == 3);
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 0);
    assert_eq!(
        workspace.shared.inner.lock().unwrap().batch_inflight_bytes,
        0
    );
}

#[test]
fn batch_writer_rejects_an_external_output_collision_and_releases_memory() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let output = tempfile::tempdir().unwrap();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(move |image| {
        started_tx.send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace.create_batch(output.path(), &[fixture]).unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let item = &workspace.batch_items(batch_id, 0, 1).unwrap().items[0];
    let destination = output.path().join(&item.output_name);
    std::fs::write(&destination, b"external result").unwrap();
    release_tx.send(()).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].failed == 1);
    assert_eq!(std::fs::read(destination).unwrap(), b"external result");
    assert_eq!(std::fs::read_dir(output.path()).unwrap().count(), 1);
    assert_eq!(
        workspace.shared.inner.lock().unwrap().batch_inflight_bytes,
        0
    );
}

#[test]
fn queued_file_replacement_with_identical_bytes_is_not_processed() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.png");
    let second = root.path().join("second.png");
    std::fs::copy(&fixture, &first).unwrap();
    std::fs::copy(&fixture, &second).unwrap();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(move |image| {
        started_tx.send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace.create_batch(root.path(), &[first]).unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    workspace.set_batch_paused(batch_id, true).unwrap();
    workspace.append_batch(batch_id, &[second.clone()]).unwrap();
    let replacement = root.path().join("replacement.png");
    std::fs::copy(&second, &replacement).unwrap();
    assert_eq!(
        std::fs::read(&second).unwrap(),
        std::fs::read(&replacement).unwrap()
    );
    std::fs::remove_file(&second).unwrap();
    std::fs::rename(replacement, &second).unwrap();
    workspace.set_batch_paused(batch_id, false).unwrap();
    release_tx.send(()).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.batches[0].succeeded == 1 && snapshot.batches[0].failed == 1
    });
    let items = workspace.batch_items(batch_id, 0, 2).unwrap().items;
    assert!(items[1].error.as_ref().unwrap().contains("替换"));
    assert!(!root.path().join(&items[1].output_name).exists());
}

#[test]
fn checkpoint_rejects_a_replaced_queued_file_on_restore() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.png");
    std::fs::copy(fixture, &source).unwrap();
    let source = std::fs::canonicalize(source).unwrap();
    let store = SessionStore::new(root.path().join("session"));
    let mut checkpoint = SessionCheckpoint::empty();
    checkpoint.batches.push(SavedBatch {
        batch_id: 1,
        output_directory: std::fs::canonicalize(root.path()).unwrap(),
        paused: false,
        items: vec![SavedBatchItem {
            item_id: 2,
            source: SavedBatchSource::File {
                path: source.clone(),
                identity: Some(batch_file_identity(&source).unwrap()),
            },
            display_name: "source.png".to_owned(),
            output_name: "source-cutout.png".to_owned(),
            state: SavedBatchState::Queued,
            error: None,
            output: None,
        }],
    });
    store.commit(&checkpoint).unwrap();
    let replacement = root.path().join("replacement.png");
    std::fs::copy(&source, &replacement).unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::rename(replacement, &source).unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    assert!(workspace.restore(&store).unwrap());
    let item = &workspace.batch_items(1, 0, 1).unwrap().items[0];
    assert_eq!(item.state, TaskState::Failed);
    assert!(item.error.as_ref().unwrap().contains("替换"));
    assert!(!root.path().join("source-cutout.png").exists());
}

#[test]
fn zip_batch_keeps_imported_images_until_terminal_export_then_cleans_them_on_remove() {
    use std::io::Read;
    use zip::write::SimpleFileOptions;

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let root = tempfile::tempdir().unwrap();
    let archive_path = root.path().join("input.zip");
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
    for name in ["first/photo.png", "second/photo.png"] {
        archive
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        std::io::copy(&mut std::fs::File::open(&fixture).unwrap(), &mut archive).unwrap();
    }
    archive.finish().unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(|image| {
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace
        .create_batch_from_zip(root.path(), &archive_path)
        .unwrap();
    assert_eq!(
        workspace.append_batch_zip(batch_id, &archive_path).unwrap(),
        4
    );
    let import_dirs = {
        let inner = workspace.shared.inner.lock().unwrap();
        inner.batches[&batch_id]
            .imported_archives
            .iter()
            .map(|dir| dir.path().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(import_dirs.len(), 2);
    wait_for(&workspace, |snapshot| snapshot.batches[0].succeeded == 4);
    let output = root.path().join("results.zip");
    assert_eq!(workspace.export_batch_zip(batch_id, &output).unwrap(), 4);
    assert!(workspace.export_batch_zip(batch_id, &output).is_err());
    let mut packed = zip::ZipArchive::new(std::fs::File::open(output).unwrap()).unwrap();
    let names = [
        "photo-cutout.png",
        "photo-cutout-2.png",
        "photo-cutout-3.png",
        "photo-cutout-4.png",
    ];
    for (index, name) in names.iter().enumerate() {
        let mut entry = packed.by_index(index).unwrap();
        assert_eq!(entry.name(), *name);
        let mut actual = Vec::new();
        entry.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, std::fs::read(root.path().join(name)).unwrap());
    }
    workspace.remove_batch(batch_id).unwrap();
    assert!(import_dirs.iter().all(|path| !path.exists()));
}

#[test]
fn running_zip_streams_committed_png_and_skips_cancelled_items() {
    use std::io::Read;

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("output");
    let zip_dir = root.path().join("zip");
    std::fs::create_dir(&output).unwrap();
    std::fs::create_dir(&zip_dir).unwrap();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let calls = AtomicUsize::new(0);
    let workspace = Arc::new(Workspace::new(|_| {}).unwrap());
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(move |image| {
        if calls.fetch_add(1, Ordering::SeqCst) == 1 {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace
        .create_batch(&output, &[fixture.clone(), fixture.clone()])
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].succeeded == 1);
    let first_name = workspace.batch_items(batch_id, 0, 1).unwrap().items[0]
        .output_name
        .clone();
    let destination = zip_dir.join("results.zip");
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let exporting = Arc::clone(&workspace);
    let archive_path = destination.clone();
    let handle = thread::spawn(move || {
        done_tx
            .send(exporting.export_batch_zip(batch_id, &archive_path))
            .unwrap();
    });
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let streamed = std::fs::read_dir(&zip_dir)
            .unwrap()
            .any(|entry| entry.unwrap().metadata().unwrap().len() > 0);
        if streamed {
            break;
        }
        assert!(Instant::now() < until, "ZIP did not stream the first PNG");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(done_rx.try_recv().is_err());
    assert!(workspace
        .export_batch_zip(batch_id, &zip_dir.join("second.zip"))
        .is_err());
    workspace.append_batch(batch_id, &[fixture]).unwrap();
    workspace.cancel_batch(batch_id).unwrap();
    release_tx.send(()).unwrap();
    assert_eq!(
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        1
    );
    handle.join().unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&destination).unwrap()).unwrap();
    assert_eq!(archive.len(), 1);
    let mut entry = archive.by_index(0).unwrap();
    assert_eq!(entry.name(), first_name);
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, std::fs::read(output.join(first_name)).unwrap());
    assert_eq!(std::fs::read_dir(zip_dir).unwrap().count(), 1);
}

#[test]
fn pausing_a_running_zip_discards_the_temporary_archive() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let root = tempfile::tempdir().unwrap();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let calls = AtomicUsize::new(0);
    let workspace = Arc::new(Workspace::new(|_| {}).unwrap());
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(move |image| {
        if calls.fetch_add(1, Ordering::SeqCst) == 1 {
            started_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace
        .create_batch(root.path(), &[fixture.clone(), fixture])
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].succeeded == 1);
    let destination = root.path().join("results.zip");
    let exporting = Arc::clone(&workspace);
    let archive_path = destination.clone();
    let handle = thread::spawn(move || exporting.export_batch_zip(batch_id, &archive_path));
    let until = Instant::now() + Duration::from_secs(5);
    while !workspace
        .shared
        .inner
        .lock()
        .unwrap()
        .batch_zip_exports
        .contains(&batch_id)
    {
        assert!(Instant::now() < until, "ZIP export did not start");
        thread::sleep(Duration::from_millis(10));
    }
    workspace.set_batch_paused(batch_id, true).unwrap();
    assert!(handle.join().unwrap().is_err());
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    release_tx.send(()).unwrap();
}

#[test]
fn batch_checkpoint_restores_committed_png_without_reprocessing() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().join("session"));
    let output = root.path().join("output");
    std::fs::create_dir(&output).unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(|image| {
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace.create_batch(&output, &[fixture]).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].succeeded == 1);
    let name = workspace.batch_items(batch_id, 0, 1).unwrap().items[0]
        .output_name
        .clone();
    let original = std::fs::read(output.join(&name)).unwrap();
    workspace.checkpoint(&store).unwrap();

    let reopened = Workspace::new(|_| {}).unwrap();
    assert!(reopened.restore(&store).unwrap());
    let summary = &reopened.snapshot().batches[0];
    assert_eq!(
        (summary.succeeded, summary.queued, summary.running),
        (1, 0, 0)
    );
    assert!(summary.paused);
    assert_eq!(std::fs::read(output.join(name)).unwrap(), original);
    assert_eq!(
        reopened
            .export_batch_zip(batch_id, &root.path().join("restored.zip"))
            .unwrap(),
        1
    );
}

#[test]
fn changed_successful_png_is_rejected_by_zip_and_recovery() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().join("session"));
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(|image| {
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace.create_batch(root.path(), &[fixture]).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].succeeded == 1);
    let output_name = workspace.batch_items(batch_id, 0, 1).unwrap().items[0]
        .output_name
        .clone();
    let output = root.path().join(output_name);
    workspace.checkpoint(&store).unwrap();
    let recorded = store.load().unwrap().unwrap().batches[0].items[0]
        .output
        .clone()
        .unwrap();
    assert_eq!(
        (recorded.sha256, recorded.bytes),
        image_io::file_digest(&output).unwrap()
    );
    let mut replacement = image_io::decode_png(&output).unwrap();
    replacement.get_pixel_mut(0, 0).0 = [7, 19, 31, 255];
    let mut bytes = Vec::new();
    image_io::encode_png(&replacement, &mut bytes).unwrap();
    std::fs::write(&output, bytes).unwrap();
    let archive = root.path().join("changed.zip");
    assert!(workspace.export_batch_zip(batch_id, &archive).is_err());
    assert!(!archive.exists());
    let reopened = Workspace::new(|_| {}).unwrap();
    assert!(reopened.restore(&store).unwrap());
    let restored = reopened.batch_items(batch_id, 0, 1).unwrap();
    assert_eq!(restored.items[0].state, TaskState::Failed);
    assert!(restored.items[0]
        .error
        .as_ref()
        .unwrap()
        .contains("内容变化"));
    assert!(reopened.snapshot().batches[0].paused);
}

#[test]
fn restored_queued_batch_stays_paused_until_user_resumes() {
    let fixture = std::fs::canonicalize(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/images/hard-edge-opaque.png"),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().join("session"));
    let mut checkpoint = SessionCheckpoint::empty();
    checkpoint.batches.push(SavedBatch {
        batch_id: 1,
        output_directory: std::fs::canonicalize(root.path()).unwrap(),
        paused: false,
        items: vec![SavedBatchItem {
            item_id: 2,
            source: SavedBatchSource::File {
                path: fixture,
                identity: None,
            },
            display_name: "photo.png".to_owned(),
            output_name: "photo-cutout.png".to_owned(),
            state: SavedBatchState::Queued,
            error: None,
            output: None,
        }],
    });
    store.commit(&checkpoint).unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(|image| {
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    assert!(workspace.restore(&store).unwrap());
    assert_eq!(workspace.snapshot().batches[0].queued, 1);
    assert!(workspace.snapshot().batches[0].paused);
    thread::sleep(Duration::from_millis(30));
    assert!(!root.path().join("photo-cutout.png").exists());
    workspace.set_batch_paused(1, false).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].succeeded == 1);
}

#[test]
fn interrupted_zip_batch_restores_as_retryable_without_publishing_old_result() {
    use zip::write::SimpleFileOptions;

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let root = tempfile::tempdir().unwrap();
    let archive_path = root.path().join("input.zip");
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
    archive
        .start_file("photo.png", SimpleFileOptions::default())
        .unwrap();
    std::io::copy(&mut std::fs::File::open(fixture).unwrap(), &mut archive).unwrap();
    archive.finish().unwrap();
    let output = root.path().join("output");
    std::fs::create_dir(&output).unwrap();
    let store = SessionStore::new(root.path().join("session"));
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let workspace = Workspace::new(|_| {}).unwrap();
    *workspace.shared.batch_infer.lock().unwrap() = Some(Box::new(move |image| {
        started_tx.send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    let batch_id = workspace
        .create_batch_from_zip(&output, &archive_path)
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    workspace.checkpoint(&store).unwrap();
    release_tx.send(()).unwrap();
    wait_for(&workspace, |snapshot| snapshot.batches[0].succeeded == 1);
    let late_output = output.join("photo-cutout.png");
    let late_bytes = std::fs::read(&late_output).unwrap();
    let collision_workspace = Workspace::new(|_| {}).unwrap();
    assert!(collision_workspace.restore(&store).unwrap());
    let collision_item = &collision_workspace
        .batch_items(batch_id, 0, 1)
        .unwrap()
        .items[0];
    assert_eq!(collision_item.state, TaskState::Failed);
    assert!(collision_item.error.as_deref().unwrap().contains("未记录"));
    assert_eq!(std::fs::read(&late_output).unwrap(), late_bytes);
    std::fs::remove_file(output.join("photo-cutout.png")).unwrap();

    let reopened = Workspace::new(|_| {}).unwrap();
    assert!(reopened.restore(&store).unwrap());
    let summary = &reopened.snapshot().batches[0];
    assert_eq!((summary.failed, summary.running), (1, 0));
    assert!(summary.paused);
    let item = &reopened.batch_items(batch_id, 0, 1).unwrap().items[0];
    assert!(item.error.as_deref().unwrap().contains("中断"));
    *reopened.shared.batch_infer.lock().unwrap() = Some(Box::new(|image| {
        Ok(GrayImage::from_pixel(
            image.width(),
            image.height(),
            Luma([255]),
        ))
    }));
    reopened.retry_batch_item(item.item_id).unwrap();
    reopened.set_batch_paused(batch_id, false).unwrap();
    wait_for(&reopened, |snapshot| snapshot.batches[0].succeeded == 1);
    assert!(output.join("photo-cutout.png").is_file());

    let replacement =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/images/low-contrast.png");
    let mut changed = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
    changed
        .start_file("photo.png", SimpleFileOptions::default())
        .unwrap();
    std::io::copy(&mut std::fs::File::open(replacement).unwrap(), &mut changed).unwrap();
    changed.finish().unwrap();
    let changed_workspace = Workspace::new(|_| {}).unwrap();
    assert!(changed_workspace.restore(&store).unwrap());
    let item = &changed_workspace.batch_items(batch_id, 0, 1).unwrap().items[0];
    assert_eq!(item.state, TaskState::Failed);
    assert!(item.error.as_deref().unwrap().contains("ZIP"));
}

#[test]
fn native_batch_writes_source_sized_png_when_model_is_available() {
    let (Some(runtime), Some(model_dir)) = (
        std::env::var_os("S08_NATIVE_RUNTIME"),
        std::env::var_os("S08_MODEL_DIR"),
    ) else {
        return;
    };
    let store = Arc::new(ModelStore::new(PathBuf::from(model_dir), |_| {}).unwrap());
    let workspace =
        Workspace::new_with_inference(|_| {}, PathBuf::from(runtime), Arc::clone(&store)).unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while !store.snapshot().features.automatic.files_verified {
        assert!(Instant::now() < until, "本地 IS-Net 模型验证超时");
        thread::sleep(Duration::from_millis(20));
    }
    let input = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let output = tempfile::tempdir().unwrap();
    let batch_id = workspace
        .create_batch(output.path(), &[input.clone()])
        .unwrap();
    let until = Instant::now() + Duration::from_secs(120);
    loop {
        let summary = &workspace.snapshot().batches[0];
        if summary.succeeded + summary.failed == 1 {
            assert_eq!(
                summary.succeeded,
                1,
                "批量项失败: {:?}",
                workspace.batch_items(batch_id, 0, 1).unwrap()
            );
            break;
        }
        assert!(Instant::now() < until, "批量推理超时");
        thread::sleep(Duration::from_millis(20));
    }
    let name = workspace.batch_items(batch_id, 0, 1).unwrap().items[0]
        .output_name
        .clone();
    let result = image_io::decode_image(&output.path().join(name)).unwrap();
    let source = image_io::decode_image(&input).unwrap();
    assert_eq!(result.dimensions(), source.dimensions());
}

#[test]
fn brush_history_tracks_exported_alpha_and_stale_revisions() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let directory = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&fixture).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            0,
            GrayImage::from_pixel(96, 64, Luma([0])),
        )
        .unwrap();
    let stroke = Stroke {
        points: vec![edit::Point { x: 0.5, y: 0.5 }],
        radius: 2,
        value: 255,
    };
    assert!(workspace.paint_mask(imported.asset_id, 1, stroke).unwrap());
    assert!(!workspace
        .paint_mask(
            imported.asset_id,
            1,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 2,
                value: 0
            }
        )
        .unwrap());
    assert!(workspace.snapshot().assets[0].can_undo);
    assert!(workspace.step_history(imported.asset_id, 2, false).unwrap());
    assert!(workspace.snapshot().assets[0].can_redo);
    assert!(workspace.step_history(imported.asset_id, 3, true).unwrap());
    let destination = directory.path().join("edited.png");
    let task_id = workspace
        .export_png(imported.asset_id, destination.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == task_id && task.state == TaskState::Succeeded)
    });
    let exported = image_io::decode_png(&destination).unwrap();
    assert_eq!(exported.get_pixel(48, 32)[3], 255);
    assert_eq!(exported.get_pixel(0, 0)[3], 0);
    workspace.release_preview(imported.asset_id).unwrap();
    assert!(!workspace.snapshot().assets[0].can_undo);
}

#[test]
fn switching_workflows_keeps_masks_and_undo_histories_separate() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let directory = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&fixture).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    assert!(workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            0,
            GrayImage::from_pixel(96, 64, Luma([0])),
        )
        .unwrap());
    assert!(workspace
        .paint_mask(
            imported.asset_id,
            1,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 2,
                value: 255,
            }
        )
        .unwrap());
    let auto_revision = workspace.snapshot().assets[0].mask_revision;
    workspace
        .switch_workflow(imported.asset_id, Workflow::Selection)
        .unwrap();
    assert!(!workspace.snapshot().assets[0].result_available);
    assert!(!workspace.snapshot().assets[0].can_undo);
    assert!(!workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            auto_revision,
            GrayImage::from_pixel(96, 64, Luma([255])),
        )
        .unwrap());
    let selection_revision = workspace.snapshot().assets[0].mask_revision;
    assert!(workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            selection_revision,
            GrayImage::from_pixel(96, 64, Luma([255])),
        )
        .unwrap());
    assert!(workspace
        .paint_mask(
            imported.asset_id,
            selection_revision + 1,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 2,
                value: 0,
            }
        )
        .unwrap());
    assert!(workspace.snapshot().assets[0].can_undo);
    workspace
        .switch_workflow(imported.asset_id, Workflow::Automatic)
        .unwrap();
    let auto = workspace.snapshot().assets[0].clone();
    assert!(auto.result_available && auto.can_undo);
    let auto_path = directory.path().join("automatic.png");
    let auto_task = workspace
        .export_png(imported.asset_id, auto_path.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == auto_task && task.state == TaskState::Succeeded)
    });
    let auto_png = image_io::decode_png(&auto_path).unwrap();
    assert_eq!(auto_png.get_pixel(48, 32)[3], 255);
    assert_eq!(auto_png.get_pixel(0, 0)[3], 0);
    workspace
        .switch_workflow(imported.asset_id, Workflow::Selection)
        .unwrap();
    let selected = workspace.snapshot().assets[0].clone();
    assert!(selected.result_available && selected.can_undo);
    let selected_path = directory.path().join("selection.png");
    let selected_task = workspace
        .export_png(imported.asset_id, selected_path.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == selected_task && task.state == TaskState::Succeeded)
    });
    let selected_png = image_io::decode_png(&selected_path).unwrap();
    assert_eq!(selected_png.get_pixel(48, 32)[3], 0);
    assert_eq!(selected_png.get_pixel(0, 0)[3], 255);
    let selected_revision = selected.mask_revision;
    assert!(workspace
        .set_background(
            imported.asset_id,
            selected_revision,
            0,
            BackgroundChoice::Solid { rgb: [12, 34, 56] }
        )
        .unwrap());
    workspace
        .switch_workflow(imported.asset_id, Workflow::Automatic)
        .unwrap();
    let auto = workspace.snapshot().assets[0].clone();
    let preview = workspace
        .result_preview_png(imported.asset_id, auto.result_revision)
        .unwrap();
    let preview = image::load_from_memory_with_format(&preview, image::ImageFormat::Png)
        .unwrap()
        .into_rgba8();
    assert_eq!(preview.get_pixel(0, 0)[3], 0);
}

#[test]
fn switching_images_and_workflows_preserves_independent_results_across_sizes() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/images");
    let directory = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    let wide = workspace.import(&fixtures.join("wide-alpha.png")).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .assets
            .iter()
            .any(|asset| asset.asset_id == wide.asset_id && asset.state == AssetState::Ready)
    });
    assert!(workspace
        .publish_mask(
            wide.asset_id,
            1,
            wide.selection_generation,
            0,
            GrayImage::from_pixel(160, 48, Luma([255])),
        )
        .unwrap());
    assert!(workspace
        .paint_mask(
            wide.asset_id,
            1,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 1,
                value: 255,
            },
        )
        .unwrap());
    let wide_original_revision = workspace.snapshot().assets[0].result_revision;
    assert!(workspace
        .set_background(
            wide.asset_id,
            2,
            0,
            BackgroundChoice::Solid { rgb: [7, 11, 13] },
        )
        .unwrap());
    assert!(workspace
        .result_preview_png(wide.asset_id, wide_original_revision)
        .is_none());

    let portrait = workspace
        .import(&fixtures.join("portrait-strands.png"))
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .assets
            .iter()
            .any(|asset| asset.asset_id == portrait.asset_id && asset.state == AssetState::Ready)
    });
    assert!(!workspace
        .publish_mask(
            wide.asset_id,
            1,
            wide.selection_generation,
            2,
            GrayImage::from_pixel(160, 48, Luma([0])),
        )
        .unwrap());
    assert!(workspace
        .publish_mask(
            portrait.asset_id,
            1,
            portrait.selection_generation,
            0,
            GrayImage::from_pixel(48, 160, Luma([255])),
        )
        .unwrap());
    assert!(workspace
        .paint_mask(
            portrait.asset_id,
            1,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 1,
                value: 255,
            },
        )
        .unwrap());
    workspace
        .switch_workflow(portrait.asset_id, Workflow::Selection)
        .unwrap();
    let selection_revision = workspace
        .snapshot()
        .assets
        .iter()
        .find(|asset| asset.asset_id == portrait.asset_id)
        .unwrap()
        .mask_revision;
    assert!(workspace
        .publish_mask(
            portrait.asset_id,
            1,
            portrait.selection_generation,
            selection_revision,
            GrayImage::from_pixel(48, 160, Luma([0])),
        )
        .unwrap());
    assert!(workspace
        .paint_mask(
            portrait.asset_id,
            selection_revision + 1,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 1,
                value: 0,
            },
        )
        .unwrap());
    assert!(workspace
        .set_background(
            portrait.asset_id,
            selection_revision + 2,
            0,
            BackgroundChoice::Solid { rgb: [19, 23, 29] },
        )
        .unwrap());
    let portrait_selection = directory.path().join("portrait-selection.png");
    let task = workspace
        .export_png(portrait.asset_id, portrait_selection.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|item| item.task_id == task && item.state == TaskState::Succeeded)
    });
    let selected = image_io::decode_png(&portrait_selection).unwrap();
    assert_eq!(selected.dimensions(), (48, 160));
    assert!(selected.pixels().all(|pixel| pixel.0 == [19, 23, 29, 255]));

    workspace
        .switch_workflow(portrait.asset_id, Workflow::Automatic)
        .unwrap();
    let portrait_auto = directory.path().join("portrait-automatic.png");
    let task = workspace
        .export_png(portrait.asset_id, portrait_auto.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|item| item.task_id == task && item.state == TaskState::Succeeded)
    });
    let automatic = image_io::decode_png(&portrait_auto).unwrap();
    let source = image_io::decode_image(&fixtures.join("portrait-strands.png")).unwrap();
    assert_eq!(automatic.dimensions(), source.dimensions());
    for (actual, original) in automatic.pixels().zip(source.pixels()) {
        assert_eq!(actual[3], 255);
        if original[3] == 255 {
            assert_eq!(actual, original);
        } else if original[3] == 0 {
            assert_eq!(actual.0, [19, 23, 29, 255]);
        }
    }

    let current_selection = workspace.select(wide.asset_id).unwrap();
    assert!(current_selection > portrait.selection_generation);
    let wide_snapshot = workspace
        .snapshot()
        .assets
        .into_iter()
        .find(|asset| asset.asset_id == wide.asset_id)
        .unwrap();
    assert!(wide_snapshot.result_available);
    assert!(matches!(
        wide_snapshot.background,
        BackgroundView::Solid { rgb: [7, 11, 13] }
    ));
    let wide_export = directory.path().join("wide-background.png");
    let task = workspace
        .export_png(wide.asset_id, wide_export.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|item| item.task_id == task && item.state == TaskState::Succeeded)
    });
    let exported = image_io::decode_png(&wide_export).unwrap();
    assert_eq!(exported.dimensions(), (160, 48));
    let source = image_io::decode_image(&fixtures.join("wide-alpha.png")).unwrap();
    for (actual, original) in exported.pixels().zip(source.pixels()) {
        assert_eq!(actual[3], 255);
        if original[3] == 255 {
            assert_eq!(actual, original);
        } else if original[3] == 0 {
            assert_eq!(actual.0, [7, 11, 13, 255]);
        }
    }
}

#[test]
fn backgrounds_change_preview_and_export_without_editing_the_mask() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let directory = tempfile::tempdir().unwrap();
    let background_path = directory.path().join("background.png");
    image_io::save_png_noclobber(
        &RgbaImage::from_pixel(4, 2, image::Rgba([9, 8, 7, 255])),
        &background_path,
    )
    .unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&fixture).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    assert!(workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            0,
            GrayImage::from_pixel(96, 64, Luma([0]))
        )
        .unwrap());
    assert!(workspace
        .paint_mask(
            imported.asset_id,
            1,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 2,
                value: 255,
            }
        )
        .unwrap());
    let original = workspace.snapshot().assets[0].clone();
    assert_eq!(original.background_revision, 0);
    assert_eq!(original.mask_revision, 2);
    let transparent_preview = workspace
        .result_preview_png(imported.asset_id, original.result_revision)
        .unwrap();
    assert!(workspace
        .set_background(
            imported.asset_id,
            2,
            0,
            BackgroundChoice::Solid { rgb: [1, 2, 3] }
        )
        .unwrap());
    let solid = workspace.snapshot().assets[0].clone();
    assert_eq!(solid.mask_revision, original.mask_revision);
    assert_eq!(solid.can_undo, original.can_undo);
    assert!(solid.result_revision > original.result_revision);
    assert!(workspace
        .result_preview_png(imported.asset_id, original.result_revision)
        .is_none());
    let preview = workspace
        .result_preview_png(imported.asset_id, solid.result_revision)
        .unwrap();
    assert!(Arc::ptr_eq(&transparent_preview, &preview));
    let preview = image::load_from_memory_with_format(&preview, image::ImageFormat::Png)
        .unwrap()
        .into_rgba8();
    assert_eq!(preview.get_pixel(0, 0)[3], 0);
    let solid_path = directory.path().join("solid.png");
    let export = workspace
        .export_png(imported.asset_id, solid_path.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == export && task.state == TaskState::Succeeded)
    });
    assert_eq!(
        image_io::decode_png(&solid_path).unwrap().get_pixel(0, 0).0,
        [1, 2, 3, 255]
    );
    assert!(workspace
        .set_background(
            imported.asset_id,
            2,
            1,
            BackgroundChoice::Image {
                path: background_path.clone()
            }
        )
        .unwrap());
    let image = workspace.snapshot().assets[0].clone();
    assert_eq!(image.mask_revision, 2);
    assert_eq!(image.background_revision, 2);
    assert!(matches!(image.background, BackgroundView::Image { .. }));
    let json = serde_json::to_value(workspace.snapshot()).unwrap();
    assert_eq!(
        json["assets"][0]["background"]["displayName"],
        "background.png"
    );
    let preview = workspace
        .result_preview_png(imported.asset_id, image.result_revision)
        .unwrap();
    let preview = image::load_from_memory_with_format(&preview, image::ImageFormat::Png)
        .unwrap()
        .into_rgba8();
    assert_eq!(preview.get_pixel(0, 0).0, [9, 8, 7, 255]);
    let image_path = directory.path().join("image.png");
    let export = workspace
        .export_png(imported.asset_id, image_path.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == export && task.state == TaskState::Succeeded)
    });
    assert_eq!(
        image_io::decode_png(&image_path).unwrap().get_pixel(0, 0).0,
        [9, 8, 7, 255]
    );
    assert!(workspace
        .set_background(imported.asset_id, 2, 2, BackgroundChoice::Transparent)
        .unwrap());
    let transparent_path = directory.path().join("transparent.png");
    let export = workspace
        .export_png(imported.asset_id, transparent_path.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == export && task.state == TaskState::Succeeded)
    });
    assert_eq!(
        image_io::decode_png(&transparent_path)
            .unwrap()
            .get_pixel(0, 0)[3],
        0
    );
    assert!(!workspace
        .set_background(
            imported.asset_id,
            1,
            3,
            BackgroundChoice::Solid { rgb: [0, 0, 0] }
        )
        .unwrap());
    assert!(workspace
        .set_background(
            imported.asset_id,
            2,
            3,
            BackgroundChoice::Image {
                path: background_path
            }
        )
        .unwrap());
    workspace.release_preview(imported.asset_id).unwrap();
    assert!(matches!(
        workspace.snapshot().assets[0].background,
        BackgroundView::Transparent
    ));
}

#[test]
fn native_automatic_result_can_be_brushed_undone_and_exported_when_models_are_available() {
    let (Some(runtime), Some(model_dir)) = (
        std::env::var_os("S08_NATIVE_RUNTIME"),
        std::env::var_os("S08_MODEL_DIR"),
    ) else {
        return;
    };
    let store = Arc::new(ModelStore::new(PathBuf::from(model_dir), |_| {}).unwrap());
    let workspace =
        Workspace::new_with_inference(|_| {}, PathBuf::from(runtime), Arc::clone(&store)).unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while !store.snapshot().features.automatic.files_verified {
        assert!(
            Instant::now() < until,
            "本地 IS-Net 模型验证超时或失败: {:?}",
            store.snapshot().models
        );
        thread::sleep(Duration::from_millis(20));
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let directory = tempfile::tempdir().unwrap();
    let imported = workspace.import(&fixture).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    let task_id = workspace.auto_cutout(imported.asset_id).unwrap();
    let inference_until = Instant::now() + Duration::from_secs(120);
    let completed = loop {
        let snapshot = workspace.snapshot();
        if snapshot.tasks.iter().any(|task| {
            task.task_id == task_id
                && matches!(task.state, TaskState::Succeeded | TaskState::Failed)
        }) {
            break snapshot;
        }
        assert!(
            Instant::now() < inference_until,
            "原生自动抠图超时: {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        completed
            .tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .unwrap()
            .state,
        TaskState::Succeeded,
        "{completed:?}"
    );
    let revision = completed.assets[0].mask_revision;
    workspace
        .paint_mask(
            imported.asset_id,
            revision,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 4,
                value: 0,
            },
        )
        .unwrap();
    workspace
        .step_history(imported.asset_id, revision + 1, false)
        .unwrap();
    workspace
        .step_history(imported.asset_id, revision + 2, true)
        .unwrap();
    let destination = directory.path().join("native-edited.png");
    let export = workspace
        .export_png(imported.asset_id, destination.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == export && task.state == TaskState::Succeeded)
    });
    let output = image_io::decode_png(&destination).unwrap();
    assert_eq!(output.dimensions(), (96, 64));
    assert_eq!(output.get_pixel(48, 32)[3], 0);
}

#[test]
fn automatic_preparation_requires_idle_selected_ready_automatic_source() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&fixture).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    {
        let mut inner = workspace.shared.inner.lock().unwrap();
        let key = Some((
            imported.asset_id,
            inner.assets[&imported.asset_id].snapshot.generation,
        ));
        assert!(automatic_preparation_candidate(&inner, None).is_some());
        assert!(
            automatic_preparation_candidate(&inner, key).is_none(),
            "one attempt per generation"
        );
        inner.queue.push_back(u64::MAX);
        assert!(
            automatic_preparation_candidate(&inner, None).is_none(),
            "queued work wins"
        );
        inner.queue.clear();
        let width = inner.assets[&imported.asset_id].snapshot.width;
        inner
            .assets
            .get_mut(&imported.asset_id)
            .unwrap()
            .snapshot
            .width = Some(1_000_000);
        assert!(
            automatic_preparation_candidate(&inner, None).is_none(),
            "do not prepare a source above the decoded-image budget"
        );
        inner
            .assets
            .get_mut(&imported.asset_id)
            .unwrap()
            .snapshot
            .width = width;
    }
    workspace
        .switch_workflow(imported.asset_id, Workflow::Selection)
        .unwrap();
    assert!(
        automatic_preparation_candidate(&workspace.shared.inner.lock().unwrap(), None).is_none()
    );
}

#[test]
#[ignore = "requires S08_NATIVE_RUNTIME, S08_MODEL_DIR and S08_PERF_IMAGE"]
fn native_idle_preparation_can_be_interrupted_then_retried() {
    let runtime = PathBuf::from(std::env::var_os("S08_NATIVE_RUNTIME").unwrap());
    let model_dir = PathBuf::from(std::env::var_os("S08_MODEL_DIR").unwrap());
    let image = PathBuf::from(std::env::var_os("S08_PERF_IMAGE").unwrap());
    let store = Arc::new(ModelStore::new(model_dir, |_| {}).unwrap());
    let workspace = Workspace::new_with_inference(|_| {}, runtime, Arc::clone(&store)).unwrap();
    let imported = workspace.import(&image).unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while workspace
        .shared
        .automatic_preparation
        .lock()
        .unwrap()
        .is_none()
    {
        assert!(Instant::now() < until, "空闲准备未开始");
        thread::sleep(Duration::from_millis(2));
    }
    workspace
        .switch_workflow(imported.asset_id, Workflow::Selection)
        .unwrap();
    while workspace
        .shared
        .automatic_preparation
        .lock()
        .unwrap()
        .is_some()
    {
        assert!(Instant::now() < until, "空闲准备未停止");
        thread::sleep(Duration::from_millis(2));
    }
    let before = workspace.snapshot();
    assert!(!before.assets[0].result_available);
    assert!(before.tasks.iter().all(|task| task.kind == "preview"));
    // Explicit retry must load a fresh, uncancelled session and publish normally.
    let task_id = workspace.auto_cutout(imported.asset_id).unwrap();
    loop {
        let snapshot = workspace.snapshot();
        let task = snapshot
            .tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .unwrap();
        if matches!(task.state, TaskState::Succeeded | TaskState::Failed) {
            assert_eq!(task.state, TaskState::Succeeded, "{task:?}");
            assert!(snapshot.assets[0].result_available);
            break;
        }
        assert!(Instant::now() < until, "重试自动抠图超时");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires S08_NATIVE_RUNTIME, S08_MODEL_DIR and S08_PERF_IMAGE"]
fn native_idle_preparation_preserves_edits_and_reuses_result() {
    let runtime = PathBuf::from(std::env::var_os("S08_NATIVE_RUNTIME").unwrap());
    let model_dir = PathBuf::from(std::env::var_os("S08_MODEL_DIR").unwrap());
    let image = PathBuf::from(std::env::var_os("S08_PERF_IMAGE").unwrap());
    let store = Arc::new(ModelStore::new(model_dir, |_| {}).unwrap());
    let workspace = Workspace::new_with_inference(|_| {}, runtime, Arc::clone(&store)).unwrap();
    let imported = workspace.import(&image).unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while !store.snapshot().features.automatic.session_ready {
        assert!(Instant::now() < until, "空闲准备超时");
        thread::sleep(Duration::from_millis(5));
    }
    let before = workspace.snapshot();
    assert_eq!(before.assets[0].mask_revision, 0);
    assert!(!before.assets[0].result_available);
    assert!(before.tasks.iter().all(|task| task.kind == "preview"));
    for label in ["prepared first click", "repeat click"] {
        let started = Instant::now();
        let task_id = workspace.auto_cutout(imported.asset_id).unwrap();
        let result = wait_for(&workspace, |snapshot| {
            snapshot.tasks.iter().any(|task| {
                task.task_id == task_id
                    && matches!(task.state, TaskState::Succeeded | TaskState::Failed)
            })
        });
        assert_eq!(
            result
                .tasks
                .iter()
                .find(|task| task.task_id == task_id)
                .unwrap()
                .state,
            TaskState::Succeeded
        );
        assert!(workspace
            .result_preview_png(imported.asset_id, result.assets[0].result_revision)
            .is_some());
        println!(
            "{label}: {} ms to result preview",
            started.elapsed().as_millis()
        );
    }
}

#[test]
#[ignore = "requires S08_NATIVE_RUNTIME, S08_MODEL_DIR and S08_PERF_IMAGE"]
fn native_automatic_workspace_latency_probe() {
    let runtime = PathBuf::from(std::env::var_os("S08_NATIVE_RUNTIME").unwrap());
    let model_dir = PathBuf::from(std::env::var_os("S08_MODEL_DIR").unwrap());
    let image = PathBuf::from(std::env::var_os("S08_PERF_IMAGE").unwrap());
    let store = Arc::new(ModelStore::new(model_dir, |_| {}).unwrap());
    let workspace = Workspace::new_with_inference(|_| {}, runtime, Arc::clone(&store)).unwrap();
    let ready_until = Instant::now() + Duration::from_secs(30);
    while !store.snapshot().features.automatic.files_verified {
        assert!(Instant::now() < ready_until, "模型校验超时");
        thread::sleep(Duration::from_millis(5));
    }
    let imported = workspace.import(&image).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    for label in ["first", "warm"] {
        let started = Instant::now();
        let task_id = workspace.auto_cutout(imported.asset_id).unwrap();
        let until = started + Duration::from_secs(30);
        loop {
            let snapshot = workspace.snapshot();
            if let Some(task) = snapshot.tasks.iter().find(|task| task.task_id == task_id) {
                if matches!(task.state, TaskState::Succeeded | TaskState::Failed) {
                    assert_eq!(task.state, TaskState::Succeeded, "{task:?}");
                    let preview = workspace
                        .result_preview_png(imported.asset_id, snapshot.assets[0].result_revision)
                        .unwrap();
                    println!(
                        "{label}: {} ms, preview {} bytes",
                        started.elapsed().as_millis(),
                        preview.len()
                    );
                    break;
                }
            }
            assert!(Instant::now() < until, "自动抠图超时");
            thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn selected_large_source_is_reused_and_released_with_its_preview() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("wide.png");
    image_io::save_png_noclobber(
        &RgbaImage::from_pixel(1025, 2, image::Rgba([40, 80, 120, 255])),
        &input,
    )
    .unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&input).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    let cached = {
        let inner = workspace.shared.inner.lock().unwrap();
        let (id, source) = inner.decoded_source.as_ref().unwrap();
        assert_eq!(*id, imported.asset_id);
        Arc::clone(source)
    };
    let reused = source_for_asset(&workspace.shared, imported.asset_id, &input).unwrap();
    assert!(Arc::ptr_eq(&cached, &reused));
    workspace.release_preview(imported.asset_id).unwrap();
    assert!(workspace
        .shared
        .inner
        .lock()
        .unwrap()
        .decoded_source
        .is_none());
}

#[test]
#[ignore = "requires S08_NATIVE_RUNTIME, S08_MODEL_DIR and S08_PERF_IMAGE"]
fn native_automatic_stage_latency_probe() {
    use crate::isnet;
    let runtime = PathBuf::from(std::env::var_os("S08_NATIVE_RUNTIME").unwrap());
    let model_dir = PathBuf::from(std::env::var_os("S08_MODEL_DIR").unwrap());
    let image = PathBuf::from(std::env::var_os("S08_PERF_IMAGE").unwrap());
    let store = ModelStore::new(model_dir, |_| {}).unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while !store.snapshot().features.automatic.files_verified {
        assert!(Instant::now() < until, "模型校验超时");
        thread::sleep(Duration::from_millis(5));
    }
    let lease = store.acquire("isnet-auto-high").unwrap();
    let mut session = IsnetSession::load_verified(&runtime, &lease).unwrap();
    let started = Instant::now();
    let preview = asset::build_preview(&image).unwrap();
    println!("import preview: {} ms", started.elapsed().as_millis());
    let started = Instant::now();
    let source = image_io::decode_image(&image).unwrap();
    println!("second decode: {} ms", started.elapsed().as_millis());
    let started = Instant::now();
    let prepared = isnet::prepare(&source).unwrap();
    println!("prepare: {} ms", started.elapsed().as_millis());
    for label in ["first", "warm"] {
        let started = Instant::now();
        let values = session.run_prepared(&prepared).unwrap();
        println!("{label} ORT: {} ms", started.elapsed().as_millis());
        let started = Instant::now();
        let normalized = mask::normalize(&values).unwrap();
        let restored = isnet::restore_contain_mask(
            &normalized,
            isnet::INPUT_SIDE,
            isnet::INPUT_SIDE,
            prepared.fitted_width,
            prepared.fitted_height,
        )
        .unwrap();
        let full_mask = mask::resize_model_mask(
            &restored,
            isnet::INPUT_SIDE,
            isnet::INPUT_SIDE,
            source.width(),
            source.height(),
        )
        .unwrap();
        println!("{label} restore: {} ms", started.elapsed().as_millis());
        let started = Instant::now();
        let result = asset::build_result_preview(&preview.png, &full_mask, None).unwrap();
        println!(
            "{label} result preview: {} ms, {} bytes",
            started.elapsed().as_millis(),
            result.len()
        );
    }
}

#[test]
fn native_selection_reuses_embedding_and_exports_multiple_image_sizes_when_models_are_available() {
    let (Some(runtime), Some(model_dir)) = (
        std::env::var_os("S08_NATIVE_RUNTIME"),
        std::env::var_os("S08_MODEL_DIR"),
    ) else {
        return;
    };
    let store = Arc::new(ModelStore::new(PathBuf::from(model_dir), |_| {}).unwrap());
    let workspace =
        Workspace::new_with_inference(|_| {}, PathBuf::from(runtime), Arc::clone(&store)).unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while !store.snapshot().features.selection.files_verified {
        assert!(
            Instant::now() < until,
            "本地 MobileSAM 模型验证超时或失败: {:?}",
            store.snapshot().models
        );
        thread::sleep(Duration::from_millis(20));
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let directory = tempfile::tempdir().unwrap();
    let imported = workspace.import(&fixture).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    for points in [
        vec![sam::Point {
            x: 0.5,
            y: 0.5,
            label: 1,
        }],
        vec![
            sam::Point {
                x: 0.5,
                y: 0.5,
                label: 1,
            },
            sam::Point {
                x: 0.25,
                y: 0.5,
                label: 0,
            },
        ],
    ] {
        let task_id = workspace
            .select_points(imported.asset_id, points.clone())
            .unwrap();
        let inference_until = Instant::now() + Duration::from_secs(120);
        let completed = loop {
            let snapshot = workspace.snapshot();
            if snapshot.tasks.iter().any(|task| {
                task.task_id == task_id
                    && matches!(task.state, TaskState::Succeeded | TaskState::Failed)
            }) {
                break snapshot;
            }
            assert!(
                Instant::now() < inference_until,
                "原生点选抠图超时: {snapshot:?}"
            );
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            completed
                .tasks
                .iter()
                .find(|task| task.task_id == task_id)
                .unwrap()
                .state,
            TaskState::Succeeded,
            "{completed:?}"
        );
        assert_eq!(completed.assets[0].selection_points.len(), points.len());
        assert!(completed.assets[0].result_available);
    }
    let destination = directory.path().join("selected.png");
    let task_id = workspace
        .export_png(imported.asset_id, destination.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == task_id && task.state == TaskState::Succeeded)
    });
    assert_eq!(
        image_io::decode_png(&destination).unwrap().dimensions(),
        (96, 64)
    );
    for (name, size) in [
        ("wide-alpha.png", (160, 48)),
        ("portrait-strands.png", (48, 160)),
    ] {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/images")
            .join(name);
        let imported = workspace.import(&fixture).unwrap();
        wait_for(&workspace, |snapshot| {
            snapshot.assets.iter().any(|asset| {
                asset.asset_id == imported.asset_id && asset.state == AssetState::Ready
            })
        });
        let points = vec![sam::Point {
            x: 0.5,
            y: 0.5,
            label: 1,
        }];
        let task_id = workspace.select_points(imported.asset_id, points).unwrap();
        let inference_until = Instant::now() + Duration::from_secs(120);
        let completed = loop {
            let snapshot = workspace.snapshot();
            if snapshot.tasks.iter().any(|task| {
                task.task_id == task_id
                    && matches!(task.state, TaskState::Succeeded | TaskState::Failed)
            }) {
                break snapshot;
            }
            assert!(
                Instant::now() < inference_until,
                "跨尺寸点选抠图超时: {snapshot:?}"
            );
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            completed
                .tasks
                .iter()
                .find(|task| task.task_id == task_id)
                .unwrap()
                .state,
            TaskState::Succeeded,
            "{completed:?}"
        );
        let destination = directory.path().join(format!("selected-{name}"));
        let export = workspace
            .export_png(imported.asset_id, destination.clone())
            .unwrap();
        wait_for(&workspace, |snapshot| {
            snapshot
                .tasks
                .iter()
                .any(|task| task.task_id == export && task.state == TaskState::Succeeded)
        });
        let exported = image_io::decode_png(&destination).unwrap();
        let source = image_io::decode_image(&fixture).unwrap();
        assert_eq!(exported.dimensions(), size);
        assert!(exported
            .pixels()
            .zip(source.pixels())
            .all(|(actual, original)| actual[3] <= original[3]));
    }
}

#[test]
fn native_refinement_uses_current_mask_and_can_be_undone_when_model_is_available() {
    let (Some(runtime), Some(model_dir)) = (
        std::env::var_os("S08_NATIVE_RUNTIME"),
        std::env::var_os("S08_MODEL_DIR"),
    ) else {
        return;
    };
    let store = Arc::new(ModelStore::new(PathBuf::from(model_dir), |_| {}).unwrap());
    let workspace =
        Workspace::new_with_inference(|_| {}, PathBuf::from(runtime), Arc::clone(&store)).unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while !store.snapshot().features.matting.files_verified {
        assert!(
            Instant::now() < until,
            "本地 ViTMatte 模型验证超时或失败: {:?}",
            store.snapshot().models
        );
        thread::sleep(Duration::from_millis(20));
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let directory = tempfile::tempdir().unwrap();
    let imported = workspace.import(&fixture).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    let coarse = GrayImage::from_fn(96, 64, |x, y| {
        Luma([if (16..80).contains(&x) && (8..56).contains(&y) {
            255
        } else {
            0
        }])
    });
    assert!(workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            0,
            coarse
        )
        .unwrap());
    assert!(workspace.refine_mask(imported.asset_id, 0, 12).is_err());
    let task_id = workspace.refine_mask(imported.asset_id, 1, 12).unwrap();
    let inference_until = Instant::now() + Duration::from_secs(120);
    let completed = loop {
        let snapshot = workspace.snapshot();
        if snapshot.tasks.iter().any(|task| {
            task.task_id == task_id
                && matches!(task.state, TaskState::Succeeded | TaskState::Failed)
        }) {
            break snapshot;
        }
        assert!(
            Instant::now() < inference_until,
            "原生精修超时: {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        completed
            .tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .unwrap()
            .state,
        TaskState::Succeeded,
        "{completed:?}"
    );
    assert_eq!(completed.assets[0].mask_revision, 2);
    assert!(completed.assets[0].can_undo);
    let refined_path = directory.path().join("refined.png");
    let export = workspace
        .export_png(imported.asset_id, refined_path.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == export && task.state == TaskState::Succeeded)
    });
    assert_eq!(
        image_io::decode_png(&refined_path).unwrap().dimensions(),
        (96, 64)
    );
    assert!(workspace.step_history(imported.asset_id, 2, false).unwrap());
    let coarse_path = directory.path().join("coarse.png");
    let export = workspace
        .export_png(imported.asset_id, coarse_path.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == export && task.state == TaskState::Succeeded)
    });
    let coarse_png = image_io::decode_png(&coarse_path).unwrap();
    assert_eq!(coarse_png.get_pixel(48, 32)[3], 255);
    assert_eq!(coarse_png.get_pixel(0, 0)[3], 0);

    let oriented_fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/images/exif-orient6.jpg");
    let oriented = workspace.import(&oriented_fixture).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .assets
            .iter()
            .any(|asset| asset.asset_id == oriented.asset_id && asset.state == AssetState::Ready)
    });
    let oriented_coarse = GrayImage::from_fn(64, 96, |x, y| {
        Luma([if (8..56).contains(&x) && (16..80).contains(&y) {
            255
        } else {
            0
        }])
    });
    assert!(workspace
        .publish_mask(
            oriented.asset_id,
            1,
            oriented.selection_generation,
            0,
            oriented_coarse,
        )
        .unwrap());
    let task_id = workspace.refine_mask(oriented.asset_id, 1, 12).unwrap();
    let inference_until = Instant::now() + Duration::from_secs(120);
    let completed = loop {
        let snapshot = workspace.snapshot();
        if snapshot.tasks.iter().any(|task| {
            task.task_id == task_id
                && matches!(task.state, TaskState::Succeeded | TaskState::Failed)
        }) {
            break snapshot;
        }
        assert!(
            Instant::now() < inference_until,
            "EXIF 图片原生精修超时: {snapshot:?}"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        completed
            .tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .unwrap()
            .state,
        TaskState::Succeeded,
        "{completed:?}"
    );
    let destination = directory.path().join("oriented-refined.png");
    let export = workspace
        .export_png(oriented.asset_id, destination.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == export && task.state == TaskState::Succeeded)
    });
    assert_eq!(
        image_io::decode_png(&destination).unwrap().dimensions(),
        (64, 96)
    );
}

#[test]
fn late_refinement_cannot_replace_a_new_brush_stroke() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&fixture).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    assert!(workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            0,
            GrayImage::from_pixel(96, 64, Luma([0])),
        )
        .unwrap());
    let task_id = {
        let mut inner = workspace.shared.inner.lock().unwrap();
        let task_id = inner.next_id();
        inner.tasks.insert(
            task_id,
            TaskSnapshot {
                task_id,
                workflow_id: imported.workflow_id,
                asset_id: imported.asset_id,
                generation: 1,
                selection_generation: imported.selection_generation,
                mask_revision: 1,
                kind: "refinement",
                state: TaskState::Running,
                stage: "inference",
                error: None,
            },
        );
        task_id
    };
    assert!(workspace
        .paint_mask(
            imported.asset_id,
            1,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 2,
                value: 255,
            }
        )
        .unwrap());
    assert!(commit_generated_mask(
        &workspace.shared,
        task_id,
        GrayImage::from_pixel(96, 64, Luma([255])),
        None,
    )
    .is_err());
    let inner = workspace.shared.inner.lock().unwrap();
    assert_eq!(inner.assets[&imported.asset_id].snapshot.mask_revision, 2);
    assert_eq!(
        inner.assets[&imported.asset_id]
            .mask
            .as_ref()
            .unwrap()
            .get_pixel(48, 32)[0],
        255
    );
    assert_eq!(
        inner.assets[&imported.asset_id]
            .mask
            .as_ref()
            .unwrap()
            .get_pixel(0, 0)[0],
        0
    );
}

#[test]
fn late_generated_masks_cannot_commit_after_workflow_or_image_switch() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/images");
    let workspace = Workspace::new(|_| {}).unwrap();
    let first = workspace
        .import(&fixtures.join("hard-edge-opaque.png"))
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    let insert_running_task = |workspace: &Workspace| {
        let mut inner = workspace.shared.inner.lock().unwrap();
        let selected_id = inner.selected_asset_id.unwrap();
        let asset = &inner.assets[&selected_id];
        let (workflow_id, generation, mask_revision) = (
            asset.snapshot.workflow_id,
            asset.snapshot.generation,
            asset.snapshot.mask_revision,
        );
        let selection_generation = inner.selection_generation;
        let task_id = inner.next_id();
        inner.tasks.insert(
            task_id,
            TaskSnapshot {
                task_id,
                workflow_id,
                asset_id: selected_id,
                generation,
                selection_generation,
                mask_revision,
                kind: "automatic",
                state: TaskState::Running,
                stage: "inference",
                error: None,
            },
        );
        task_id
    };
    let first_task = insert_running_task(&workspace);
    workspace
        .switch_workflow(first.asset_id, Workflow::Selection)
        .unwrap();
    assert!(commit_generated_mask(
        &workspace.shared,
        first_task,
        GrayImage::from_pixel(96, 64, Luma([255])),
        None,
    )
    .is_err());
    let second_task = insert_running_task(&workspace);
    let second = workspace
        .import(&fixtures.join("portrait-strands.png"))
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .assets
            .iter()
            .any(|asset| asset.asset_id == second.asset_id && asset.state == AssetState::Ready)
    });
    assert!(commit_generated_mask(
        &workspace.shared,
        second_task,
        GrayImage::from_pixel(96, 64, Luma([255])),
        None,
    )
    .is_err());
    let snapshot = workspace.snapshot();
    assert!(snapshot.assets.iter().all(|asset| !asset.result_available));
    assert_eq!(snapshot.selected_asset_id, Some(second.asset_id));
}

#[test]
fn import_preview_release_and_mask_revision_stay_in_rust() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&fixture).unwrap();
    let ready = wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    assert_eq!(ready.selected_asset_id, Some(imported.asset_id));
    assert_eq!(ready.assets[0].width, Some(96));
    assert_eq!(ready.assets[0].height, Some(64));
    assert_eq!(ready.tasks[0].state, TaskState::Succeeded);
    let revision = ready.assets[0].preview_revision;
    assert!(workspace
        .preview_png(imported.asset_id, revision)
        .unwrap()
        .starts_with(b"\x89PNG\r\n\x1a\n"));
    let mask = GrayImage::from_pixel(96, 64, Luma([128]));
    assert!(workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            0,
            mask.clone()
        )
        .unwrap());
    assert!(!workspace
        .publish_mask(imported.asset_id, 1, imported.selection_generation, 0, mask)
        .unwrap());
    assert_eq!(workspace.snapshot().assets[0].mask_revision, 1);
    workspace.release_preview(imported.asset_id).unwrap();
    assert!(workspace.preview_png(imported.asset_id, revision).is_none());
    assert_eq!(workspace.snapshot().assets[0].mask_revision, 1);
}

#[test]
fn checkpoint_keeps_verified_source_after_original_moves() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let source = directory.path().join("source.png");
    std::fs::copy(fixture, &source).unwrap();
    let store = SessionStore::new(directory.path().join("session"));
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&source).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            0,
            GrayImage::from_pixel(96, 64, Luma([128])),
        )
        .unwrap();
    let saved_revision = workspace.checkpoint(&store).unwrap();
    assert_eq!(saved_revision, workspace.snapshot().revision);
    let first = store.load().unwrap().unwrap();
    assert_eq!(first.selected_asset_id, Some(imported.asset_id));
    assert!(first.assets[0].automatic.mask.is_some());
    assert!(first.assets[0].selection.mask.is_none());
    std::fs::remove_file(source).unwrap();
    workspace.checkpoint(&store).unwrap();
    let after = store.load().unwrap().unwrap();
    assert_eq!(after.assets[0].source, first.assets[0].source);
    assert_eq!(
        after.assets[0].automatic.mask,
        first.assets[0].automatic.mask
    );
}

#[test]
fn restored_workspace_rebuilds_preview_and_keeps_both_edit_histories() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let source = directory.path().join("original.png");
    std::fs::copy(fixture, &source).unwrap();
    let store = SessionStore::new(directory.path().join("session"));
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&source).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            0,
            GrayImage::from_pixel(96, 64, Luma([128])),
        )
        .unwrap();
    let revision = workspace.snapshot().assets[0].mask_revision;
    workspace
        .paint_mask(
            imported.asset_id,
            revision,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 3,
                value: 255,
            },
        )
        .unwrap();
    workspace
        .switch_workflow(imported.asset_id, Workflow::Selection)
        .unwrap();
    let revision = workspace.snapshot().assets[0].mask_revision;
    workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            revision,
            GrayImage::from_pixel(96, 64, Luma([64])),
        )
        .unwrap();
    let revision = workspace.snapshot().assets[0].mask_revision;
    workspace
        .paint_mask(
            imported.asset_id,
            revision,
            Stroke {
                points: vec![edit::Point { x: 0.25, y: 0.25 }],
                radius: 3,
                value: 0,
            },
        )
        .unwrap();
    workspace
        .switch_workflow(imported.asset_id, Workflow::Automatic)
        .unwrap();
    let before = workspace.snapshot();
    workspace
        .set_background(
            imported.asset_id,
            before.assets[0].mask_revision,
            before.assets[0].background_revision,
            BackgroundChoice::Solid { rgb: [20, 30, 40] },
        )
        .unwrap();
    workspace.checkpoint(&store).unwrap();
    drop(workspace);
    std::fs::remove_file(source).unwrap();

    let reopened = Workspace::new(|_| {}).unwrap();
    assert!(reopened.restore(&store).unwrap());
    let cold = reopened.snapshot();
    assert_eq!(cold.selected_asset_id, Some(imported.asset_id));
    assert!(!cold.assets[0].preview_available);
    assert!(reopened.hydrate_preview(imported.asset_id).unwrap());
    let ready = reopened.snapshot();
    assert_eq!(ready.assets[0].state, AssetState::Ready);
    assert!(ready.assets[0].result_available);
    assert!(ready.assets[0].can_undo);
    assert!(matches!(
        ready.assets[0].background,
        BackgroundView::Solid { rgb: [20, 30, 40] }
    ));
    assert!(reopened
        .result_preview_png(imported.asset_id, ready.assets[0].result_revision)
        .is_some());
    reopened
        .switch_workflow(imported.asset_id, Workflow::Selection)
        .unwrap();
    let selected = reopened.snapshot();
    assert!(selected.assets[0].result_available);
    assert!(selected.assets[0].can_undo);
    assert!(reopened
        .step_history(imported.asset_id, selected.assets[0].mask_revision, false)
        .unwrap());
    assert!(reopened.snapshot().assets[0].can_redo);
}

#[test]
fn restored_image_background_matches_before_sources_are_removed() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let source = directory.path().join("source.png");
    let background = directory.path().join("background.png");
    std::fs::copy(&fixture, &source).unwrap();
    std::fs::copy(&fixture, &background).unwrap();
    let store = SessionStore::new(directory.path().join("session"));
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&source).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            0,
            GrayImage::from_pixel(96, 64, Luma([128])),
        )
        .unwrap();
    let revision = workspace.snapshot().assets[0].mask_revision;
    workspace
        .paint_mask(
            imported.asset_id,
            revision,
            Stroke {
                points: vec![edit::Point { x: 0.5, y: 0.5 }],
                radius: 2,
                value: 255,
            },
        )
        .unwrap();
    let before = workspace.snapshot();
    workspace
        .set_background(
            imported.asset_id,
            before.assets[0].mask_revision,
            before.assets[0].background_revision,
            BackgroundChoice::Image {
                path: background.clone(),
            },
        )
        .unwrap();
    let before = workspace.snapshot();
    let original_result = workspace
        .result_preview_png(imported.asset_id, before.assets[0].result_revision)
        .unwrap();
    workspace.checkpoint(&store).unwrap();
    drop(workspace);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(background).unwrap();

    let reopened = Workspace::new(|_| {}).unwrap();
    assert!(reopened.restore(&store).unwrap());
    store.clear().unwrap();
    assert!(reopened.hydrate_preview(imported.asset_id).unwrap());
    let ready = reopened.snapshot();
    assert!(matches!(
        ready.assets[0].background,
        BackgroundView::Image { .. }
    ));
    let restored_result = reopened
        .result_preview_png(imported.asset_id, ready.assets[0].result_revision)
        .unwrap();
    assert_eq!(*restored_result, *original_result);
}

#[test]
fn restore_rejects_wrong_mask_dimensions_before_showing_a_result() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let store = SessionStore::new(directory.path().join("session"));
    let source = store.put_file(&fixture).unwrap();
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(GrayImage::from_pixel(2, 2, Luma([255])))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let wrong_mask = store.put_bytes(encoded.get_ref()).unwrap();
    let mut checkpoint = SessionCheckpoint::empty();
    checkpoint.selected_asset_id = Some(1);
    checkpoint.assets.push(SavedAsset {
        asset_id: 1,
        display_name: "source.png".to_owned(),
        source,
        workflow: SavedWorkflow::Automatic,
        automatic: SavedEdits {
            mask: Some(wrong_mask),
            undo: vec![],
            redo: vec![],
        },
        selection: SavedEdits {
            mask: None,
            undo: vec![],
            redo: vec![],
        },
        selection_points: vec![],
        background: SavedBackground::Transparent,
    });
    store.commit(&checkpoint).unwrap();
    let workspace = Workspace::new(|_| {}).unwrap();
    assert!(workspace.restore(&store).unwrap());
    assert!(workspace.hydrate_preview(1).unwrap_err().contains("尺寸"));
    assert!(!workspace.snapshot().assets[0].result_available);
}

#[test]
fn restored_images_can_be_hydrated_when_selected_later() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let first = directory.path().join("first.png");
    let second = directory.path().join("second.png");
    std::fs::copy(&fixture, &first).unwrap();
    std::fs::copy(&fixture, &second).unwrap();
    let store = SessionStore::new(directory.path().join("session"));
    let workspace = Workspace::new(|_| {}).unwrap();
    let first_id = workspace.import(&first).unwrap().asset_id;
    let second_id = workspace.import(&second).unwrap().asset_id;
    wait_for(&workspace, |snapshot| {
        snapshot
            .assets
            .iter()
            .all(|asset| asset.state == AssetState::Ready)
    });
    workspace.checkpoint(&store).unwrap();
    drop(workspace);
    let restored = Workspace::new(|_| {}).unwrap();
    assert!(restored.restore(&store).unwrap());
    assert_eq!(restored.snapshot().selected_asset_id, Some(second_id));
    assert!(restored.hydrate_preview(second_id).unwrap());
    restored.select(first_id).unwrap();
    assert!(restored.hydrate_preview(first_id).unwrap());
    let snapshot = restored.snapshot();
    assert_eq!(snapshot.selected_asset_id, Some(first_id));
    assert!(snapshot.assets.iter().all(|asset| asset.preview_available));
}

#[test]
fn export_uses_rust_mask_and_never_overwrites_a_result() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace.import(&fixture).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.assets[0].state == AssetState::Ready
    });
    workspace
        .publish_mask(
            imported.asset_id,
            1,
            imported.selection_generation,
            0,
            GrayImage::from_pixel(96, 64, Luma([128])),
        )
        .unwrap();
    let destination = directory.path().join("cutout.png");
    let task_id = workspace
        .export_png(imported.asset_id, destination.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == task_id && task.state == TaskState::Succeeded)
    });
    let output = image_io::decode_png(&destination).unwrap();
    assert_eq!(output.dimensions(), (96, 64));
    assert_eq!(output.get_pixel(0, 0)[3], 128);
    let first = std::fs::read(&destination).unwrap();
    let retry_id = workspace
        .export_png(imported.asset_id, destination.clone())
        .unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == retry_id && task.state == TaskState::Failed)
    });
    assert_eq!(std::fs::read(destination).unwrap(), first);
}

#[test]
fn clipboard_pixels_enter_the_same_preview_queue() {
    let workspace = Workspace::new(|_| {}).unwrap();
    let imported = workspace
        .import_clipboard_image(2, 1, vec![200, 30, 40, 255, 9, 8, 7, 128])
        .unwrap();
    let snapshot = wait_for(&workspace, |value| {
        value.assets[0].state == AssetState::Ready
    });
    assert_eq!(snapshot.assets[0].asset_id, imported.asset_id);
    assert_eq!(
        (snapshot.assets[0].width, snapshot.assets[0].height),
        (Some(2), Some(1))
    );
    assert!(workspace
        .preview_png(imported.asset_id, snapshot.assets[0].preview_revision)
        .is_some());
    assert!(workspace.import_clipboard_image(0, 1, Vec::new()).is_err());
}

#[test]
fn cancel_retry_and_switch_cannot_publish_a_stale_preview() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("first.png");
    let second_path = directory.path().join("second.png");
    std::fs::write(&first_path, b"mock").unwrap();
    std::fs::write(&second_path, b"mock").unwrap();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let calls = AtomicUsize::new(0);
    let workspace = Workspace::with_processor(
        |_| {},
        move |_| {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
            Ok(Preview {
                source_size: (2, 2),
                png: vec![call as u8],
                source: None,
            })
        },
    )
    .unwrap();
    let first = workspace.import(&first_path).unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let second = workspace.import(&second_path).unwrap();
    assert_eq!(
        workspace.snapshot().selected_asset_id,
        Some(second.asset_id)
    );
    workspace.cancel(first.task_id).unwrap();
    assert_eq!(
        workspace.snapshot().tasks[0].state,
        TaskState::CancelRequested,
        "running task stays active until its worker returns"
    );
    let retry = workspace.retry_preview(first.asset_id).unwrap();
    release_tx.send(()).unwrap();
    let finished = wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == retry && task.state == TaskState::Succeeded)
    });
    assert_eq!(finished.selected_asset_id, Some(second.asset_id));
    assert_eq!(finished.tasks[0].state, TaskState::Stale);
    let first_asset = finished
        .assets
        .iter()
        .find(|asset| asset.asset_id == first.asset_id)
        .unwrap();
    assert_eq!(first_asset.generation, 2);
    assert_eq!(
        workspace
            .preview_png(first.asset_id, first_asset.preview_revision)
            .unwrap()
            .as_slice(),
        &[2]
    );
    assert!(!workspace
        .publish_mask(
            first.asset_id,
            first_asset.generation,
            first.selection_generation,
            0,
            GrayImage::from_pixel(2, 2, Luma([255])),
        )
        .unwrap());
}

#[test]
fn bounded_queue_rejects_extra_work_without_dropping_existing_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mock.png");
    std::fs::write(&path, b"mock").unwrap();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Mutex::new(release_rx);
    let calls = AtomicUsize::new(0);
    let workspace = Workspace::with_processor(
        |_| {},
        move |_| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                started_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
            Ok(Preview {
                source_size: (1, 1),
                png: vec![1],
                source: None,
            })
        },
    )
    .unwrap();
    workspace.import(&path).unwrap();
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let queued: Vec<_> = (0..QUEUE_LIMIT)
        .map(|_| workspace.import(&path).unwrap())
        .collect();
    assert!(workspace.import(&path).is_err());
    assert_eq!(workspace.snapshot().tasks.len(), QUEUE_LIMIT + 1);
    workspace.cancel(queued[0].task_id).unwrap();
    assert!(workspace.import(&path).is_ok());
    release_tx.send(()).unwrap();
}

#[test]
fn preview_cache_evicts_old_entries_without_losing_assets() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mock.png");
    std::fs::write(&path, b"mock").unwrap();
    let workspace = Workspace::with_processor(
        |_| {},
        |_| {
            Ok(Preview {
                source_size: (1, 1),
                png: vec![7; 2 * 1024 * 1024],
                source: None,
            })
        },
    )
    .unwrap();
    let mut imports = Vec::new();
    for _ in 0..18 {
        let imported = workspace.import(&path).unwrap();
        wait_for(&workspace, |snapshot| {
            snapshot
                .tasks
                .iter()
                .any(|task| task.task_id == imported.task_id && task.state == TaskState::Succeeded)
        });
        imports.push(imported);
    }
    let snapshot = workspace.snapshot();
    assert_eq!(snapshot.assets.len(), 18);
    assert_eq!(
        snapshot
            .assets
            .iter()
            .filter(|asset| asset.preview_available)
            .count(),
        16
    );
    assert!(workspace.preview_revision(imports[0].asset_id).is_none());
    assert!(workspace.preview_revision(imports[17].asset_id).is_some());
}

#[test]
fn worker_continues_after_a_preview_processor_panics() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mock.png");
    std::fs::write(&path, b"mock").unwrap();
    let calls = AtomicUsize::new(0);
    let workspace = Workspace::with_processor(
        |_| {},
        move |_| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("decoder panic");
            }
            Ok(Preview {
                source_size: (1, 1),
                png: vec![1],
                source: None,
            })
        },
    )
    .unwrap();
    let failed = workspace.import(&path).unwrap();
    wait_for(&workspace, |snapshot| {
        snapshot.tasks[0].state == TaskState::Failed
    });
    let next = workspace.import(&path).unwrap();
    let finished = wait_for(&workspace, |snapshot| {
        snapshot
            .tasks
            .iter()
            .any(|task| task.task_id == next.task_id && task.state == TaskState::Succeeded)
    });
    assert_eq!(finished.tasks[0].task_id, failed.task_id);
    assert_eq!(finished.assets[0].error.as_deref(), Some("预览处理异常"));
}
