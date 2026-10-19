//! App-private, revision-pinned model cache. Downloads run on a Rust I/O worker;
//! inference sessions and pixels never cross the webview IPC boundary.
use crate::model::{self, ModelSpec};
use reqwest::blocking::Client;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

type Listener = dyn Fn(u64) + Send + Sync;
type Fetcher =
    dyn Fn(&ModelSpec, &Path, &dyn Fn() -> bool, &dyn Fn(u64)) -> Result<(), String> + Send + Sync;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    Missing,
    Queued,
    Downloading,
    Verifying,
    CancelRequested,
    Cancelled,
    FileVerified,
    SessionReady,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSnapshot {
    pub spec: ModelSpec,
    pub state: ModelState,
    pub received_bytes: u64,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureSnapshot {
    pub files_verified: bool,
    pub session_ready: bool,
    pub missing_model_ids: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureReadiness {
    pub automatic: FeatureSnapshot,
    pub selection: FeatureSnapshot,
    pub matting: FeatureSnapshot,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStoreSnapshot {
    pub revision: u64,
    pub models: Vec<ModelSnapshot>,
    pub features: FeatureReadiness,
}

struct Entry {
    snapshot: ModelSnapshot,
    generation: u64,
    leases: usize,
    cancel_requested: bool,
}

#[derive(Clone, Copy)]
enum WorkKind {
    Scan,
    Download,
}

#[derive(Clone, Copy)]
struct WorkItem {
    id: &'static str,
    generation: u64,
    kind: WorkKind,
}

struct Inner {
    revision: u64,
    entries: BTreeMap<&'static str, Entry>,
    queue: VecDeque<WorkItem>,
    shutdown: bool,
}

impl Inner {
    fn changed(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    fn feature(&self, ids: &[&'static str]) -> FeatureSnapshot {
        let missing_model_ids = ids
            .iter()
            .copied()
            .filter(|id| {
                !matches!(
                    self.entries.get(id).map(|entry| entry.snapshot.state),
                    Some(ModelState::FileVerified | ModelState::SessionReady)
                )
            })
            .collect();
        FeatureSnapshot {
            files_verified: ids.iter().all(|id| {
                matches!(
                    self.entries.get(id).map(|entry| entry.snapshot.state),
                    Some(ModelState::FileVerified | ModelState::SessionReady)
                )
            }),
            session_ready: ids.iter().all(|id| {
                self.entries
                    .get(id)
                    .is_some_and(|entry| entry.snapshot.state == ModelState::SessionReady)
            }),
            missing_model_ids,
        }
    }

    fn snapshot(&self) -> ModelStoreSnapshot {
        ModelStoreSnapshot {
            revision: self.revision,
            models: self
                .entries
                .values()
                .map(|entry| entry.snapshot.clone())
                .collect(),
            features: FeatureReadiness {
                automatic: self.feature(&["isnet-auto-high"]),
                selection: self.feature(&["mobilesam-image-encoder", "mobilesam-mask-decoder"]),
                matting: self.feature(&["vitmatte-high"]),
            },
        }
    }
}

struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
    cache_dir: PathBuf,
    listener: Box<Listener>,
    fetcher: Box<Fetcher>,
}

pub struct ModelStore {
    shared: Arc<Shared>,
}

impl ModelStore {
    pub fn new(
        cache_dir: PathBuf,
        listener: impl Fn(u64) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        Self::with_fetcher(cache_dir, &model::MODELS, listener, download_model)
    }

    fn with_fetcher(
        cache_dir: PathBuf,
        specs: &'static [ModelSpec],
        listener: impl Fn(u64) + Send + Sync + 'static,
        fetcher: impl Fn(&ModelSpec, &Path, &dyn Fn() -> bool, &dyn Fn(u64)) -> Result<(), String>
            + Send
            + Sync
            + 'static,
    ) -> Result<Self, String> {
        let mut entries = BTreeMap::new();
        let mut queue = VecDeque::new();
        for &spec in specs {
            entries.insert(
                spec.id,
                Entry {
                    snapshot: ModelSnapshot {
                        spec,
                        state: ModelState::Verifying,
                        received_bytes: 0,
                        error: None,
                    },
                    generation: 1,
                    leases: 0,
                    cancel_requested: false,
                },
            );
            queue.push_back(WorkItem {
                id: spec.id,
                generation: 1,
                kind: WorkKind::Scan,
            });
        }
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                revision: 0,
                entries,
                queue,
                shutdown: false,
            }),
            wake: Condvar::new(),
            cache_dir,
            listener: Box::new(listener),
            fetcher: Box::new(fetcher),
        });
        let worker = Arc::clone(&shared);
        thread::Builder::new()
            .name("clearframe-model-io".to_owned())
            .spawn(move || run_worker(worker))
            .map_err(|error| format!("启动模型工作线程失败: {error}"))?;
        Ok(Self { shared })
    }

    pub fn snapshot(&self) -> ModelStoreSnapshot {
        self.shared.inner.lock().unwrap().snapshot()
    }

    /// An explicit user action; a failed or cancelled download can be retried.
    pub fn download(&self, id: &str) -> Result<(), String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let entry = inner.entries.get(id).ok_or("未知模型")?;
            if matches!(
                entry.snapshot.state,
                ModelState::Queued | ModelState::Downloading | ModelState::CancelRequested
            ) || (entry.snapshot.state == ModelState::Verifying
                && inner
                    .queue
                    .iter()
                    .any(|item| item.id == id && matches!(item.kind, WorkKind::Download)))
            {
                return Ok(());
            }
            if matches!(
                entry.snapshot.state,
                ModelState::FileVerified | ModelState::SessionReady
            ) {
                return Ok(());
            }
            if inner.queue.len() > inner.entries.len() {
                return Err("模型任务队列已满".to_owned());
            }
            let entry = inner.entries.get_mut(id).unwrap();
            entry.generation += 1;
            entry.cancel_requested = false;
            entry.snapshot.state = ModelState::Queued;
            entry.snapshot.received_bytes = 0;
            entry.snapshot.error = None;
            let generation = entry.generation;
            let model_id = entry.snapshot.spec.id;
            inner.queue.retain(|item| item.id != model_id);
            inner.queue.push_back(WorkItem {
                id: model_id,
                generation,
                kind: WorkKind::Download,
            });
            inner.changed()
        };
        self.shared.wake.notify_one();
        (self.shared.listener)(revision);
        Ok(())
    }

    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let entry = inner.entries.get_mut(id).ok_or("未知模型")?;
            match entry.snapshot.state {
                ModelState::Queued => {
                    entry.generation += 1;
                    entry.snapshot.state = ModelState::Cancelled;
                    inner.queue.retain(|item| item.id != id);
                }
                ModelState::Downloading | ModelState::Verifying => {
                    entry.cancel_requested = true;
                    entry.snapshot.state = ModelState::CancelRequested;
                }
                ModelState::CancelRequested => return Ok(()),
                _ => return Err("模型任务未运行".to_owned()),
            }
            inner.changed()
        };
        (self.shared.listener)(revision);
        Ok(())
    }

    pub fn clear(&self, id: &str) -> Result<(), String> {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let entry = inner.entries.get_mut(id).ok_or("未知模型")?;
            if entry.leases > 0 || entry.snapshot.state == ModelState::SessionReady {
                return Err("模型正在被推理会话使用".to_owned());
            }
            if matches!(
                entry.snapshot.state,
                ModelState::Queued
                    | ModelState::Downloading
                    | ModelState::Verifying
                    | ModelState::CancelRequested
            ) {
                return Err("请先取消并等待模型任务结束".to_owned());
            }
            let path = model_path(&self.shared.cache_dir, &entry.snapshot.spec);
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("清理模型失败: {error}")),
            }
            entry.snapshot.state = ModelState::Missing;
            entry.snapshot.received_bytes = 0;
            entry.snapshot.error = None;
            inner.changed()
        };
        (self.shared.listener)(revision);
        Ok(())
    }

    /// The S07 session owner must hold this lease while loading and using ORT.
    /// Clearing a leased model fails instead of deleting bytes in use.
    pub fn acquire(&self, id: &str) -> Result<ModelLease, String> {
        let mut inner = self.shared.inner.lock().unwrap();
        let entry = inner.entries.get_mut(id).ok_or("未知模型")?;
        if !matches!(
            entry.snapshot.state,
            ModelState::FileVerified | ModelState::SessionReady
        ) {
            return Err("模型文件尚未验证".to_owned());
        }
        entry.leases += 1;
        Ok(ModelLease {
            shared: Arc::clone(&self.shared),
            id: entry.snapshot.spec.id,
            path: model_path(&self.shared.cache_dir, &entry.snapshot.spec),
        })
    }
}

impl Drop for ModelStore {
    fn drop(&mut self) {
        let mut inner = self.shared.inner.lock().unwrap();
        inner.shutdown = true;
        self.shared.wake.notify_one();
    }
}

pub struct ModelLease {
    shared: Arc<Shared>,
    id: &'static str,
    path: PathBuf,
}

impl ModelLease {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Call only after the actual native session has loaded successfully.
    pub fn mark_session_ready(&self) {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let entry = inner.entries.get_mut(self.id).unwrap();
            entry.snapshot.state = ModelState::SessionReady;
            inner.changed()
        };
        (self.shared.listener)(revision);
    }
}

impl Drop for ModelLease {
    fn drop(&mut self) {
        let revision = {
            let mut inner = self.shared.inner.lock().unwrap();
            let entry = inner.entries.get_mut(self.id).unwrap();
            entry.leases -= 1;
            if entry.leases == 0 && entry.snapshot.state == ModelState::SessionReady {
                entry.snapshot.state = ModelState::FileVerified;
                Some(inner.changed())
            } else {
                None
            }
        };
        if let Some(revision) = revision {
            (self.shared.listener)(revision);
        }
    }
}

fn model_path(root: &Path, spec: &ModelSpec) -> PathBuf {
    root.join(format!("{}-{}.onnx", spec.id, spec.revision))
}

fn run_worker(shared: Arc<Shared>) {
    loop {
        let work = {
            let mut inner = shared.inner.lock().unwrap();
            loop {
                if inner.shutdown {
                    return;
                }
                if let Some(work) = inner.queue.pop_front() {
                    if inner.entries[work.id].generation == work.generation {
                        break work;
                    }
                } else {
                    inner = shared.wake.wait(inner).unwrap();
                }
            }
        };
        let spec = {
            let inner = shared.inner.lock().unwrap();
            inner.entries[work.id].snapshot.spec
        };
        let path = model_path(&shared.cache_dir, &spec);
        let cancelled = || {
            let inner = shared.inner.lock().unwrap();
            inner.shutdown
                || inner.entries[work.id].generation != work.generation
                || inner.entries[work.id].cancel_requested
        };
        let report_gate = Mutex::new((0_u64, Instant::now()));
        let progress = |bytes| {
            {
                let mut gate = report_gate.lock().unwrap();
                if bytes < spec.bytes
                    && bytes - gate.0 < 1024 * 1024
                    && gate.1.elapsed() < Duration::from_millis(250)
                {
                    return;
                }
                *gate = (bytes, Instant::now());
            }
            let revision = {
                let mut inner = shared.inner.lock().unwrap();
                let entry = inner.entries.get_mut(work.id).unwrap();
                if entry.generation != work.generation {
                    return;
                }
                entry.snapshot.received_bytes = bytes;
                if bytes == spec.bytes {
                    entry.snapshot.state = ModelState::Verifying;
                }
                inner.changed()
            };
            (shared.listener)(revision);
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match work.kind {
            WorkKind::Scan => {
                if !path.exists() {
                    Ok(false)
                } else {
                    model::verify_file(&path, &spec)
                        .map(|()| true)
                        .map_err(|error| format!("缓存校验失败: {error}"))
                }
            }
            WorkKind::Download => {
                let revision = {
                    let mut inner = shared.inner.lock().unwrap();
                    let entry = inner.entries.get_mut(work.id).unwrap();
                    entry.snapshot.state = ModelState::Downloading;
                    inner.changed()
                };
                (shared.listener)(revision);
                if path.exists() && model::verify_file(&path, &spec).is_ok() {
                    Ok(true)
                } else {
                    let remove = if path.exists() {
                        fs::remove_file(&path).map_err(|error| format!("清理损坏缓存失败: {error}"))
                    } else {
                        Ok(())
                    };
                    remove.and_then(|()| {
                        (shared.fetcher)(&spec, &shared.cache_dir, &cancelled, &progress)
                            .and_then(|()| {
                                model::verify_file(&path, &spec)
                                    .map_err(|error| format!("下载文件校验失败: {error}"))
                            })
                            .map(|()| true)
                    })
                }
            }
        }))
        .unwrap_or_else(|_| Err("模型工作线程处理异常".to_owned()));
        let revision = {
            let mut inner = shared.inner.lock().unwrap();
            let shutdown = inner.shutdown;
            let entry = inner.entries.get_mut(work.id).unwrap();
            if entry.generation != work.generation {
                continue;
            }
            if entry.cancel_requested || shutdown {
                entry.snapshot.state = ModelState::Cancelled;
                entry.snapshot.error = None;
            } else {
                match result {
                    Ok(true) => {
                        entry.snapshot.state = ModelState::FileVerified;
                        entry.snapshot.received_bytes = spec.bytes;
                        entry.snapshot.error = None;
                    }
                    Ok(false) => {
                        entry.snapshot.state = ModelState::Missing;
                        entry.snapshot.received_bytes = 0;
                    }
                    Err(error) => {
                        entry.snapshot.state = ModelState::Failed;
                        entry.snapshot.error = Some(error);
                    }
                }
            }
            inner.changed()
        };
        (shared.listener)(revision);
    }
}

fn download_model(
    spec: &ModelSpec,
    root: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(u64),
) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| format!("创建模型缓存失败: {error}"))?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(30 * 60))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|error| format!("建立下载连接失败: {error}"))?;
    let response = client
        .get(spec.download_url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .map_err(|error| format!("模型下载失败: {error}"))?;
    if response.status() != reqwest::StatusCode::OK || response.url().scheme() != "https" {
        return Err(format!("模型下载响应无效: {}", response.status()));
    }
    if let Some(length) = response.content_length() {
        if length != spec.bytes {
            return Err(format!(
                "下载字节数不匹配: 预期 {}, 实际 {length}",
                spec.bytes
            ));
        }
    }
    install_reader(spec, root, response, cancelled, progress)
}

fn install_reader(
    spec: &ModelSpec,
    root: &Path,
    mut reader: impl Read,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(u64),
) -> Result<(), String> {
    let mut temp = tempfile::NamedTempFile::new_in(root)
        .map_err(|error| format!("创建临时模型文件失败: {error}"))?;
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        if cancelled() {
            return Err("下载已取消".to_owned());
        }
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("读取下载内容失败: {error}"))?;
        if count == 0 {
            break;
        }
        received += count as u64;
        if received > spec.bytes {
            return Err("下载内容超过清单字节数".to_owned());
        }
        temp.write_all(&buffer[..count])
            .map_err(|error| format!("写入模型缓存失败: {error}"))?;
        hasher.update(&buffer[..count]);
        progress(received);
    }
    if cancelled() {
        return Err("下载已取消".to_owned());
    }
    if received != spec.bytes {
        return Err(format!(
            "下载字节数不匹配: 预期 {}, 实际 {received}",
            spec.bytes
        ));
    }
    if format!("{:x}", hasher.finalize()) != spec.sha256 {
        return Err("下载模型 SHA-256 不匹配".to_owned());
    }
    temp.as_file_mut()
        .sync_all()
        .map_err(|error| format!("同步模型缓存失败: {error}"))?;
    let final_path = model_path(root, spec);
    if final_path.exists() {
        model::verify_file(&final_path, spec)
            .map_err(|error| format!("已有缓存无效，请先清理: {error}"))?;
        return Ok(());
    }
    temp.persist_noclobber(&final_path)
        .map_err(|error| format!("提交模型缓存失败: {}", error.error))?;
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .ok(); // Some Windows filesystems cannot sync directory handles.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Cursor,
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
    };

    const TEST_SPEC: ModelSpec = ModelSpec {
        id: "fixture",
        revision: "v1",
        bytes: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        download_url: "https://example.invalid/test.onnx",
        license: "test",
        license_url: "https://example.invalid/license",
        tensor_contract: "test",
        verified_backend: "test",
    };

    fn wait_for(store: &ModelStore, state: ModelState) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if store.snapshot().models[0].state == state {
                return;
            }
            assert!(Instant::now() < until, "model worker timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn install_is_atomic_and_rejects_partial_corrupt_or_cancelled_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = model_path(directory.path(), &TEST_SPEC);
        for bytes in [b"ab".as_slice(), b"xyz", b"abcd"] {
            assert!(install_reader(
                &TEST_SPEC,
                directory.path(),
                Cursor::new(bytes),
                &|| false,
                &|_| {}
            )
            .is_err());
            assert!(!path.exists());
        }
        assert!(install_reader(
            &TEST_SPEC,
            directory.path(),
            Cursor::new(b"abc"),
            &|| true,
            &|_| {}
        )
        .is_err());
        assert!(!path.exists());
        install_reader(
            &TEST_SPEC,
            directory.path(),
            Cursor::new(b"abc"),
            &|| false,
            &|_| {},
        )
        .unwrap();
        model::verify_file(&path, &TEST_SPEC).unwrap();
    }

    #[test]
    fn startup_does_not_need_to_create_a_model_directory() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("not-created-yet");
        let store = ModelStore::with_fetcher(
            cache.clone(),
            &[TEST_SPEC],
            |_| {},
            |_, _, _, _| panic!("startup must not download"),
        )
        .unwrap();
        wait_for(&store, ModelState::Missing);
        assert!(!cache.exists());
    }

    #[test]
    fn cancel_retry_lease_and_offline_cold_start() {
        let directory = tempfile::tempdir().unwrap();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempt_counter = Arc::clone(&attempts);
        let store = ModelStore::with_fetcher(
            directory.path().to_path_buf(),
            &[TEST_SPEC],
            |_| {},
            move |spec, root, cancelled, progress| {
                if attempt_counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    started_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                }
                install_reader(spec, root, Cursor::new(b"abc"), cancelled, progress)
            },
        )
        .unwrap();
        wait_for(&store, ModelState::Missing);
        store.download(TEST_SPEC.id).unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        store.cancel(TEST_SPEC.id).unwrap();
        assert_eq!(
            store.snapshot().models[0].state,
            ModelState::CancelRequested
        );
        release_tx.send(()).unwrap();
        wait_for(&store, ModelState::Cancelled);
        assert!(!model_path(directory.path(), &TEST_SPEC).exists());

        store.download(TEST_SPEC.id).unwrap();
        wait_for(&store, ModelState::FileVerified);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        let lease = store.acquire(TEST_SPEC.id).unwrap();
        lease.mark_session_ready();
        assert!(store.clear(TEST_SPEC.id).is_err());
        drop(lease);
        wait_for(&store, ModelState::FileVerified);
        drop(store);

        let offline = ModelStore::with_fetcher(
            directory.path().to_path_buf(),
            &[TEST_SPEC],
            |_| {},
            |_, _, _, _| panic!("cold start must not download"),
        )
        .unwrap();
        wait_for(&offline, ModelState::FileVerified);
        offline.clear(TEST_SPEC.id).unwrap();
        wait_for(&offline, ModelState::Missing);
        assert!(!model_path(directory.path(), &TEST_SPEC).exists());
    }
}
