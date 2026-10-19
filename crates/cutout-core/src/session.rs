//! Versioned, private recovery checkpoints. A manifest becomes visible only
//! after every referenced blob has been committed; exports live elsewhere.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime},
};

const SCHEMA_VERSION: u32 = 2;
const MANIFEST_LIMIT: u64 = 4 * 1024 * 1024;
const BLOB_LIMIT: u64 = 200 * 1024 * 1024;
const ASSET_LIMIT: usize = 512;
const HISTORY_LIMIT: usize = 20;
const BATCH_LIMIT: usize = 256;
const BATCH_ITEM_LIMIT: usize = 20_000;
const PENDING_PREFIX: &str = ".clearframe-pending-";
const PENDING_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobRef {
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedEdits {
    pub mask: Option<BlobRef>,
    pub undo: Vec<BlobRef>,
    pub redo: Vec<BlobRef>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedWorkflow {
    Automatic,
    Selection,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SavedBackground {
    Transparent,
    Solid { rgb: [u8; 3] },
    Image { display_name: String, file: BlobRef },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedAsset {
    pub asset_id: u64,
    pub display_name: String,
    pub source: BlobRef,
    pub workflow: SavedWorkflow,
    pub automatic: SavedEdits,
    pub selection: SavedEdits,
    pub selection_points: Vec<crate::sam::Point>,
    pub background: SavedBackground,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SavedBatchSource {
    File {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity: Option<SavedFileIdentity>,
    },
    Archive {
        path: PathBuf,
        entry_index: usize,
        sha256: String,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedFileIdentity {
    pub bytes: u64,
    pub modified_ns: i64,
    pub volume_id: Option<u64>,
    pub file_id: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedBatchState {
    Queued,
    Running,
    CancelRequested,
    Cancelled,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedBatchOutput {
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedBatchItem {
    pub item_id: u64,
    pub source: SavedBatchSource,
    pub display_name: String,
    pub output_name: String,
    pub state: SavedBatchState,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<SavedBatchOutput>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedBatch {
    pub batch_id: u64,
    pub output_directory: PathBuf,
    pub paused: bool,
    pub items: Vec<SavedBatchItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCheckpoint {
    pub schema_version: u32,
    pub workspace_revision: u64,
    pub selected_asset_id: Option<u64>,
    pub assets: Vec<SavedAsset>,
    #[serde(default)]
    pub batches: Vec<SavedBatch>,
}

impl SessionCheckpoint {
    pub fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            workspace_revision: 0,
            selected_asset_id: None,
            assets: Vec::new(),
            batches: Vec::new(),
        }
    }

    fn references(&self) -> Result<BTreeSet<BlobRefKey>, String> {
        if self.schema_version != 1 && self.schema_version != SCHEMA_VERSION {
            return Err("会话检查点版本不兼容".to_owned());
        }
        if self.schema_version == 1 && !self.batches.is_empty() {
            return Err("旧版会话检查点不能包含批次".to_owned());
        }
        if self.assets.len() > ASSET_LIMIT {
            return Err("会话资产数量超过上限".to_owned());
        }
        let ids: BTreeSet<_> = self.assets.iter().map(|asset| asset.asset_id).collect();
        if ids.contains(&0)
            || ids.len() != self.assets.len()
            || self.selected_asset_id.is_some_and(|id| !ids.contains(&id))
        {
            return Err("会话资产标识无效".to_owned());
        }
        let mut refs = BTreeSet::new();
        for asset in &self.assets {
            if asset.display_name.is_empty() || asset.display_name.len() > 1024 {
                return Err("会话文件名无效".to_owned());
            }
            refs.insert(BlobRefKey::validate(&asset.source)?);
            if let SavedBackground::Image { display_name, file } = &asset.background {
                if display_name.is_empty() || display_name.len() > 1024 {
                    return Err("会话背景文件名无效".to_owned());
                }
                refs.insert(BlobRefKey::validate(file)?);
            }
            if asset.selection_points.len() > 64
                || asset.selection_points.iter().any(|point| {
                    !point.x.is_finite()
                        || !point.y.is_finite()
                        || !(0.0..=1.0).contains(&point.x)
                        || !(0.0..=1.0).contains(&point.y)
                        || point.label > 1
                })
            {
                return Err("会话点选参数无效".to_owned());
            }
            for edits in [&asset.automatic, &asset.selection] {
                if edits.undo.len() > HISTORY_LIMIT
                    || edits.redo.len() > HISTORY_LIMIT
                    || (edits.mask.is_none() && (!edits.undo.is_empty() || !edits.redo.is_empty()))
                {
                    return Err("会话编辑历史超过上限".to_owned());
                }
                for reference in edits.mask.iter().chain(&edits.undo).chain(&edits.redo) {
                    refs.insert(BlobRefKey::validate(reference)?);
                }
            }
        }
        if self.batches.len() > BATCH_LIMIT {
            return Err("会话批次数量超过上限".to_owned());
        }
        let mut all_ids = ids;
        let mut item_count = 0_usize;
        for batch in &self.batches {
            if batch.batch_id == 0
                || !all_ids.insert(batch.batch_id)
                || !batch.output_directory.is_absolute()
            {
                return Err("会话批次标识或输出目录无效".to_owned());
            }
            item_count = item_count
                .checked_add(batch.items.len())
                .ok_or("会话批量项超过上限")?;
            if item_count > BATCH_ITEM_LIMIT {
                return Err("会话批量项超过上限".to_owned());
            }
            let mut output_names = BTreeSet::new();
            for item in &batch.items {
                if item.item_id == 0
                    || !all_ids.insert(item.item_id)
                    || item.display_name.is_empty()
                    || item.display_name.len() > 1024
                    || item.output_name.is_empty()
                    || item.output_name.len() > 1024
                    || item.output_name.contains(['/', '\\'])
                    || item.output_name.contains(':')
                    || !item.output_name.to_ascii_lowercase().ends_with(".png")
                    || !output_names.insert(item.output_name.to_lowercase())
                    || item.error.as_ref().is_some_and(|error| error.len() > 2048)
                {
                    return Err("会话批量项元数据无效".to_owned());
                }
                match &item.source {
                    SavedBatchSource::File { path, .. } if path.is_absolute() => {}
                    SavedBatchSource::Archive {
                        path,
                        entry_index,
                        sha256,
                    } if path.is_absolute() && *entry_index < 10_000 && is_sha256(sha256) => {}
                    _ => return Err("会话批量输入引用无效".to_owned()),
                }
                if item.output.as_ref().is_some_and(|output| {
                    !matches!(item.state, SavedBatchState::Succeeded)
                        || !is_sha256(&output.sha256)
                        || output.bytes == 0
                        || output.bytes > 4 * 1024 * 1024 * 1024
                }) {
                    return Err("会话批量输出校验值无效".to_owned());
                }
            }
        }
        Ok(refs)
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct BlobRefKey(String, u64);

impl BlobRefKey {
    fn validate(reference: &BlobRef) -> Result<Self, String> {
        if reference.sha256.len() != 64
            || !reference
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || reference.bytes == 0
            || reference.bytes > BLOB_LIMIT
        {
            return Err("会话数据引用无效".to_owned());
        }
        Ok(Self(reference.sha256.clone(), reference.bytes))
    }
}

pub struct SessionStore {
    root: PathBuf,
    gate: Mutex<()>,
    seen_files: Mutex<HashSet<PathBuf>>,
}

impl SessionStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            gate: Mutex::new(()),
            seen_files: Mutex::new(HashSet::new()),
        }
    }

    pub fn put_bytes(&self, bytes: &[u8]) -> Result<BlobRef, String> {
        if !bytes.is_empty() && bytes.len() as u64 <= BLOB_LIMIT {
            let reference = BlobRef {
                sha256: format!("{:x}", Sha256::digest(bytes)),
                bytes: bytes.len() as u64,
            };
            if self.blob_exists(&reference)? {
                // commit() verifies every referenced blob before publishing.
                return Ok(reference);
            }
        }
        self.put_reader(bytes)
    }

    pub fn put_file(&self, path: &Path) -> Result<BlobRef, String> {
        let file = fs::File::open(path).map_err(to_error)?;
        if file.metadata().map_err(to_error)?.len() > BLOB_LIMIT {
            return Err("会话数据超过单文件预算".to_owned());
        }
        let seen = self.seen_files.lock().unwrap().contains(path);
        let reference = if seen {
            let expected = hash_reader(file)?;
            if self.blob_exists(&expected)? {
                // The source is rehashed on every save, so a changed file is
                // never silently replaced with an earlier checkpoint.
                expected
            } else {
                let copied = self.put_reader(fs::File::open(path).map_err(to_error)?)?;
                if copied != expected {
                    return Err("原图在备份期间发生变化，请重试".to_owned());
                }
                copied
            }
        } else {
            self.put_reader(file)?
        };
        self.seen_files.lock().unwrap().insert(path.to_path_buf());
        Ok(reference)
    }

    fn blob_exists(&self, reference: &BlobRef) -> Result<bool, String> {
        let _guard = self.gate.lock().unwrap();
        let root = self.root.as_path();
        if root.exists() {
            reject_symlink(root)?;
        }
        let directory = self.blobs_dir();
        if directory.exists() {
            reject_symlink(&directory)?;
        }
        match fs::symlink_metadata(directory.join(&reference.sha256)) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }

    fn put_reader(&self, mut reader: impl Read) -> Result<BlobRef, String> {
        let _guard = self.gate.lock().unwrap();
        let directory = self.blobs_dir();
        ensure_private_directory(&self.root)?;
        ensure_private_directory(&directory)?;
        let mut pending = tempfile::Builder::new()
            .prefix(PENDING_PREFIX)
            .tempfile_in(&directory)
            .map_err(to_error)?;
        let mut hasher = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer).map_err(to_error)?;
            if read == 0 {
                break;
            }
            size = size
                .checked_add(read as u64)
                .ok_or("会话数据超过单文件预算")?;
            if size > BLOB_LIMIT {
                return Err("会话数据超过单文件预算".to_owned());
            }
            hasher.update(&buffer[..read]);
            pending.write_all(&buffer[..read]).map_err(to_error)?;
        }
        if size == 0 {
            return Err("会话数据为空".to_owned());
        }
        let reference = BlobRef {
            sha256: format!("{:x}", hasher.finalize()),
            bytes: size,
        };
        let path = directory.join(&reference.sha256);
        pending.as_file().sync_all().map_err(to_error)?;
        match pending.persist_noclobber(&path) {
            Ok(_) => Ok(reference),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                verify_blob(&path, &reference)?;
                Ok(reference)
            }
            Err(error) => Err(error.error.to_string()),
        }
    }

    pub fn commit(&self, checkpoint: &SessionCheckpoint) -> Result<(), String> {
        let _guard = self.gate.lock().unwrap();
        if checkpoint.schema_version != SCHEMA_VERSION {
            return Err("会话检查点版本不兼容".to_owned());
        }
        let refs = checkpoint.references()?;
        ensure_private_directory(&self.root)?;
        let path = self.manifest_path();
        if path.exists() {
            // Never replace a newer or malformed checkpoint implicitly.
            if let Some(previous) = self.read_manifest()? {
                if checkpoint.workspace_revision < previous.workspace_revision {
                    return Err("会话检查点已由较新的工作区状态更新".to_owned());
                }
            }
        }
        for BlobRefKey(hash, size) in refs {
            verify_blob(
                &self.blobs_dir().join(&hash),
                &BlobRef {
                    sha256: hash,
                    bytes: size,
                },
            )?;
        }
        let bytes = serde_json::to_vec_pretty(checkpoint).map_err(to_error)?;
        if bytes.len() as u64 > MANIFEST_LIMIT {
            return Err("会话清单超过大小预算".to_owned());
        }
        let mut pending = tempfile::Builder::new()
            .prefix(PENDING_PREFIX)
            .tempfile_in(&self.root)
            .map_err(to_error)?;
        pending.write_all(&bytes).map_err(to_error)?;
        pending.write_all(b"\n").map_err(to_error)?;
        pending.as_file().sync_all().map_err(to_error)?;
        pending.persist(path).map_err(to_error)?;
        Ok(())
    }

    pub fn load(&self) -> Result<Option<SessionCheckpoint>, String> {
        let _guard = self.gate.lock().unwrap();
        let Some(checkpoint) = self.read_manifest()? else {
            return Ok(None);
        };
        for BlobRefKey(hash, size) in checkpoint.references()? {
            verify_blob(
                &self.blobs_dir().join(&hash),
                &BlobRef {
                    sha256: hash,
                    bytes: size,
                },
            )?;
        }
        Ok(Some(checkpoint))
    }

    pub fn read_blob(&self, reference: &BlobRef) -> Result<Vec<u8>, String> {
        BlobRefKey::validate(reference)?;
        let _guard = self.gate.lock().unwrap();
        let path = self.blobs_dir().join(&reference.sha256);
        verify_blob(&path, reference)?;
        fs::read(path).map_err(to_error)
    }

    pub fn verified_blob_path(&self, reference: &BlobRef) -> Result<PathBuf, String> {
        BlobRefKey::validate(reference)?;
        let _guard = self.gate.lock().unwrap();
        let path = self.blobs_dir().join(&reference.sha256);
        verify_blob(&path, reference)?;
        Ok(path)
    }

    /// Removes content-addressed blobs no longer named by the committed
    /// manifest. A missing or unreadable manifest never authorizes deletion.
    /// Call this only after a successful commit or verified restore.
    pub fn prune_unreferenced(&self) -> Result<u64, String> {
        self.prune_unreferenced_at(SystemTime::now())
    }

    fn prune_unreferenced_at(&self, now: SystemTime) -> Result<u64, String> {
        let _guard = self.gate.lock().unwrap();
        let Some(checkpoint) = self.read_manifest()? else {
            return self.disk_usage_locked();
        };
        let referenced: BTreeSet<_> = checkpoint
            .references()?
            .into_iter()
            .map(|reference| reference.0)
            .collect();
        let directory = self.blobs_dir();
        if directory.exists() {
            reject_symlink(&directory)?;
            for entry in fs::read_dir(&directory).map_err(to_error)? {
                let entry = entry.map_err(to_error)?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.len() != 64
                    || !name
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                    || referenced.contains(name)
                {
                    continue;
                }
                if !entry.file_type().map_err(to_error)?.is_file() {
                    return Err("会话 blob 目录包含非文件项".to_owned());
                }
                fs::remove_file(entry.path()).map_err(to_error)?;
            }
        } else if !referenced.is_empty() {
            return Err("会话数据目录缺失".to_owned());
        }
        if directory.exists() {
            prune_stale_pending(&directory, now)?;
        }
        prune_stale_pending(&self.root, now)?;
        self.disk_usage_locked()
    }

    /// Logical bytes used by files in the private session directory,
    /// including incomplete or orphan files until pruned.
    pub fn disk_usage_bytes(&self) -> Result<u64, String> {
        let _guard = self.gate.lock().unwrap();
        self.disk_usage_locked()
    }

    fn disk_usage_locked(&self) -> Result<u64, String> {
        match fs::symlink_metadata(&self.root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.to_string()),
            Ok(_) => reject_symlink(&self.root)?,
        }
        let mut bytes = 0_u64;
        for entry in fs::read_dir(&self.root).map_err(to_error)? {
            let entry = entry.map_err(to_error)?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(to_error)?;
            if metadata.is_file() {
                bytes = bytes
                    .checked_add(metadata.len())
                    .ok_or("会话数据大小溢出")?;
            } else if entry.file_name() == "blobs" && metadata.is_dir() {
                for blob in fs::read_dir(entry.path()).map_err(to_error)? {
                    let blob = blob.map_err(to_error)?;
                    let metadata = fs::symlink_metadata(blob.path()).map_err(to_error)?;
                    if !metadata.is_file() {
                        return Err("会话 blob 目录包含非文件项".to_owned());
                    }
                    bytes = bytes
                        .checked_add(metadata.len())
                        .ok_or("会话数据大小溢出")?;
                }
            } else {
                return Err("会话目录包含非文件项".to_owned());
            }
        }
        Ok(bytes)
    }

    /// Clears only the application-provided session directory. User exports
    /// and model files must never be placed inside this directory.
    pub fn clear(&self) -> Result<(), String> {
        let _guard = self.gate.lock().unwrap();
        match fs::symlink_metadata(&self.root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
            Ok(_) => {}
        }
        reject_symlink(&self.root)?;
        fs::remove_dir_all(&self.root).map_err(to_error)?;
        self.seen_files.lock().unwrap().clear();
        Ok(())
    }

    fn read_manifest(&self) -> Result<Option<SessionCheckpoint>, String> {
        let path = self.manifest_path();
        if self.root.exists() {
            reject_symlink(&self.root)?;
        }
        if fs::symlink_metadata(&path).is_ok() {
            reject_symlink(&path)?;
        }
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(MANIFEST_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(to_error)?;
        if bytes.len() as u64 > MANIFEST_LIMIT {
            return Err("会话清单超过大小预算".to_owned());
        }
        let checkpoint: SessionCheckpoint = serde_json::from_slice(&bytes).map_err(to_error)?;
        checkpoint.references()?;
        Ok(Some(checkpoint))
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.json")
    }
    fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }
}

fn hash_reader(mut reader: impl Read) -> Result<BlobRef, String> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(to_error)?;
        if read == 0 {
            break;
        }
        size = size
            .checked_add(read as u64)
            .ok_or("会话数据超过单文件预算")?;
        if size > BLOB_LIMIT {
            return Err("会话数据超过单文件预算".to_owned());
        }
        hasher.update(&buffer[..read]);
    }
    if size == 0 {
        return Err("会话数据为空".to_owned());
    }
    Ok(BlobRef {
        sha256: format!("{:x}", hasher.finalize()),
        bytes: size,
    })
}

fn prune_stale_pending(directory: &Path, now: SystemTime) -> Result<(), String> {
    reject_symlink(directory)?;
    for entry in fs::read_dir(directory).map_err(to_error)? {
        let entry = entry.map_err(to_error)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(PENDING_PREFIX) && !name.starts_with(".tmp") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(to_error)?;
        if !metadata.is_file() {
            return Err("会话临时文件类型无效".to_owned());
        }
        if metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= PENDING_GRACE)
        {
            fs::remove_file(entry.path()).map_err(to_error)?;
        }
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(to_error)?;
    reject_symlink(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(to_error)?;
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path)
        .map_err(to_error)?
        .file_type()
        .is_symlink()
    {
        Err("会话目录包含符号链接".to_owned())
    } else {
        Ok(())
    }
}

fn verify_blob(path: &Path, reference: &BlobRef) -> Result<(), String> {
    reject_symlink(path)?;
    let mut file = fs::File::open(path).map_err(to_error)?;
    if file.metadata().map_err(to_error)?.len() != reference.bytes {
        return Err("会话数据大小不匹配".to_owned());
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(to_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if format!("{:x}", hasher.finalize()) != reference.sha256 {
        return Err("会话数据校验失败".to_owned());
    }
    Ok(())
}

fn to_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn asset(source: BlobRef) -> SavedAsset {
        SavedAsset {
            asset_id: 7,
            display_name: "portrait.png".to_owned(),
            source,
            workflow: SavedWorkflow::Automatic,
            automatic: SavedEdits {
                mask: None,
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
        }
    }

    #[test]
    fn committed_checkpoint_roundtrips_and_rejects_missing_blob_without_losing_old_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("session"));
        let source = store.put_bytes(b"source image bytes").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&store.root).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let mask = store.put_bytes(b"mask bytes").unwrap();
        let mut checkpoint = SessionCheckpoint {
            schema_version: SCHEMA_VERSION,
            workspace_revision: 1,
            selected_asset_id: Some(7),
            assets: vec![asset(source)],
            batches: Vec::new(),
        };
        checkpoint.assets[0].automatic.mask = Some(mask);
        store.commit(&checkpoint).unwrap();
        let manifest_before = fs::read(store.manifest_path()).unwrap();
        let reopened = store.load().unwrap().unwrap();
        assert_eq!(reopened.selected_asset_id, Some(7));
        assert_eq!(
            store.read_blob(&reopened.assets[0].source).unwrap(),
            b"source image bytes"
        );
        assert_eq!(
            reopened.assets[0].automatic.mask,
            checkpoint.assets[0].automatic.mask
        );

        checkpoint.assets[0].automatic.mask = Some(BlobRef {
            sha256: "0".repeat(64),
            bytes: 8,
        });
        assert!(store.commit(&checkpoint).is_err());
        assert_eq!(fs::read(store.manifest_path()).unwrap(), manifest_before);
        let mut newer = SessionCheckpoint::empty();
        newer.workspace_revision = 2;
        store.commit(&newer).unwrap();
        assert!(store
            .commit(&reopened)
            .unwrap_err()
            .contains("较新的工作区"));
    }

    #[test]
    fn corruption_and_newer_schema_are_reported_without_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("session"));
        let source = store.put_bytes(b"original").unwrap();
        let checkpoint = SessionCheckpoint {
            schema_version: SCHEMA_VERSION,
            workspace_revision: 1,
            selected_asset_id: Some(7),
            assets: vec![asset(source.clone())],
            batches: Vec::new(),
        };
        store.commit(&checkpoint).unwrap();
        fs::write(store.blobs_dir().join(&source.sha256), b"tampered").unwrap();
        assert!(store.load().unwrap_err().contains("校验失败"));

        let newer =
            br#"{"schemaVersion":3,"workspaceRevision":2,"selectedAssetId":null,"assets":[]}"#;
        fs::write(store.manifest_path(), newer).unwrap();
        assert!(store.load().unwrap_err().contains("版本不兼容"));
        assert!(store.commit(&SessionCheckpoint::empty()).is_err());
        assert_eq!(fs::read(store.manifest_path()).unwrap(), newer);
    }

    #[test]
    fn legacy_v1_manifest_loads_without_batches_and_upgrades_on_next_commit() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("session"));
        fs::create_dir(&store.root).unwrap();
        fs::write(
            store.manifest_path(),
            br#"{"schemaVersion":1,"workspaceRevision":7,"selectedAssetId":null,"assets":[]}"#,
        )
        .unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert!(loaded.batches.is_empty());
        assert_eq!(loaded.schema_version, 1);
        let mut upgraded = SessionCheckpoint::empty();
        upgraded.workspace_revision = 8;
        store.commit(&upgraded).unwrap();
        assert_eq!(store.load().unwrap().unwrap().schema_version, 2);
    }

    #[test]
    fn batch_manifest_rejects_unsafe_names_and_duplicate_ids_before_commit() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("session"));
        let mut checkpoint = SessionCheckpoint::empty();
        checkpoint.workspace_revision = 1;
        checkpoint.batches.push(SavedBatch {
            batch_id: 1,
            output_directory: directory.path().to_path_buf(),
            paused: false,
            items: vec![SavedBatchItem {
                item_id: 2,
                source: SavedBatchSource::File {
                    path: directory.path().join("input.png"),
                    identity: None,
                },
                display_name: "input.png".to_owned(),
                output_name: "input-cutout.png".to_owned(),
                state: SavedBatchState::Queued,
                error: None,
                output: None,
            }],
        });
        store.commit(&checkpoint).unwrap();
        let valid = fs::read(store.manifest_path()).unwrap();
        checkpoint.batches[0].items[0].output_name = "../escape.png".to_owned();
        assert!(store.commit(&checkpoint).is_err());
        checkpoint.batches[0].items[0].output_name = "C:escape.png".to_owned();
        assert!(store.commit(&checkpoint).is_err());
        checkpoint.batches[0].items[0].output_name = "input-cutout.png".to_owned();
        checkpoint.batches[0].items[0].output = Some(SavedBatchOutput {
            sha256: "not-a-digest".to_owned(),
            bytes: 1,
        });
        assert!(store.commit(&checkpoint).is_err());
        checkpoint.batches[0].items[0].output = None;
        checkpoint.batches[0].items[0].item_id = 1;
        assert!(store.commit(&checkpoint).is_err());
        assert_eq!(fs::read(store.manifest_path()).unwrap(), valid);
    }

    #[test]
    fn clear_touches_only_session_files_and_rejects_path_traversal_refs() {
        let directory = tempfile::tempdir().unwrap();
        let export = directory.path().join("user-export.png");
        fs::write(&export, b"export").unwrap();
        let model = directory.path().join("models").join("model.onnx");
        fs::create_dir_all(model.parent().unwrap()).unwrap();
        fs::write(&model, b"model").unwrap();
        let store = SessionStore::new(directory.path().join("session"));
        let source = store.put_bytes(b"source").unwrap();
        let mut checkpoint = SessionCheckpoint {
            schema_version: SCHEMA_VERSION,
            workspace_revision: 1,
            selected_asset_id: Some(7),
            assets: vec![asset(source)],
            batches: Vec::new(),
        };
        store.commit(&checkpoint).unwrap();
        checkpoint.assets[0].source.sha256 = "../user-export.png".to_owned();
        assert!(store.commit(&checkpoint).is_err());
        fs::write(
            store.root.join(".tmp-old-manifest"),
            b"private pending manifest",
        )
        .unwrap();
        fs::write(
            store.blobs_dir().join(".tmp-old-blob"),
            b"private pending blob",
        )
        .unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&export, store.root.join("external-link")).unwrap();
            std::os::unix::fs::symlink(
                directory.path().join("models"),
                store.root.join("external-dir"),
            )
            .unwrap();
        }
        store.clear().unwrap();
        assert!(!store.root.exists());
        assert!(store.load().unwrap().is_none());
        assert_eq!(fs::read(export).unwrap(), b"export");
        assert_eq!(fs::read(model).unwrap(), b"model");
    }

    #[test]
    fn pruning_follows_only_the_committed_manifest_and_updates_usage() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("session"));
        let source = store.put_bytes(b"source").unwrap();
        let orphan = store.put_bytes(b"incomplete save").unwrap();
        assert_eq!(
            store.prune_unreferenced().unwrap(),
            store.disk_usage_bytes().unwrap()
        );
        assert!(store.blobs_dir().join(&orphan.sha256).exists());
        let checkpoint = SessionCheckpoint {
            schema_version: SCHEMA_VERSION,
            workspace_revision: 1,
            selected_asset_id: Some(7),
            assets: vec![asset(source.clone())],
            batches: Vec::new(),
        };
        store.commit(&checkpoint).unwrap();
        let before = store.disk_usage_bytes().unwrap();
        let after = store.prune_unreferenced().unwrap();
        assert_eq!(before - after, orphan.bytes);
        assert!(!store.blobs_dir().join(&orphan.sha256).exists());
        assert_eq!(store.read_blob(&source).unwrap(), b"source");

        let mut empty = SessionCheckpoint::empty();
        empty.workspace_revision = 2;
        store.commit(&empty).unwrap();
        store.prune_unreferenced().unwrap();
        assert!(!store.blobs_dir().join(&source.sha256).exists());
        assert_eq!(
            store.disk_usage_bytes().unwrap(),
            fs::metadata(store.manifest_path()).unwrap().len()
        );
    }

    #[test]
    fn malformed_manifest_never_authorizes_pruning() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("session"));
        let source = store.put_bytes(b"source").unwrap();
        store
            .commit(&SessionCheckpoint {
                schema_version: SCHEMA_VERSION,
                workspace_revision: 1,
                selected_asset_id: Some(7),
                assets: vec![asset(source.clone())],
                batches: Vec::new(),
            })
            .unwrap();
        fs::write(store.manifest_path(), b"broken").unwrap();
        assert!(store.prune_unreferenced().is_err());
        assert!(store.blobs_dir().join(&source.sha256).exists());
    }

    #[test]
    fn stale_pending_files_are_counted_then_pruned_only_with_a_valid_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("session"));
        let source = store.put_bytes(b"source").unwrap();
        let pending = store.root.join(format!("{PENDING_PREFIX}manifest"));
        let blob_pending = store.blobs_dir().join(".tmp-legacy-blob");
        fs::write(&pending, b"pending manifest").unwrap();
        fs::write(&blob_pending, b"pending blob").unwrap();
        let before = store.disk_usage_bytes().unwrap();
        let future = SystemTime::now() + PENDING_GRACE + Duration::from_secs(1);
        store.prune_unreferenced_at(future).unwrap();
        assert!(pending.exists());
        assert!(blob_pending.exists());
        store
            .commit(&SessionCheckpoint {
                schema_version: SCHEMA_VERSION,
                workspace_revision: 1,
                selected_asset_id: Some(7),
                assets: vec![asset(source.clone())],
                batches: Vec::new(),
            })
            .unwrap();
        store.prune_unreferenced_at(SystemTime::now()).unwrap();
        assert!(pending.exists());
        assert!(blob_pending.exists());
        let after = store.prune_unreferenced_at(future).unwrap();
        assert!(!pending.exists());
        assert!(!blob_pending.exists());
        assert!(store.blobs_dir().join(source.sha256).exists());
        assert!(after < before + fs::metadata(store.manifest_path()).unwrap().len());
    }

    #[test]
    fn repeated_source_detects_same_size_changes_and_commit_rechecks_existing_blobs() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("source.png");
        fs::write(&source_path, b"source one").unwrap();
        let store = SessionStore::new(directory.path().join("session"));
        let first = store.put_file(&source_path).unwrap();
        let mut checkpoint = SessionCheckpoint {
            schema_version: SCHEMA_VERSION,
            workspace_revision: 1,
            selected_asset_id: Some(7),
            assets: vec![asset(first.clone())],
            batches: Vec::new(),
        };
        store.commit(&checkpoint).unwrap();

        fs::write(&source_path, b"source two").unwrap();
        let second = store.put_file(&source_path).unwrap();
        assert_ne!(first, second);
        checkpoint.workspace_revision = 2;
        checkpoint.assets[0].source = second.clone();
        store.commit(&checkpoint).unwrap();
        assert_eq!(store.read_blob(&second).unwrap(), b"source two");

        let manifest_before = fs::read(store.manifest_path()).unwrap();
        fs::write(store.blobs_dir().join(&second.sha256), b"corruption").unwrap();
        assert_eq!(store.put_file(&source_path).unwrap(), second);
        checkpoint.workspace_revision = 3;
        assert!(store.commit(&checkpoint).is_err());
        assert_eq!(fs::read(store.manifest_path()).unwrap(), manifest_before);
    }

    #[test]
    #[ignore = "manual 64 MiB checkpoint IO probe"]
    fn repeated_large_file_checkpoint_io_probe() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("large-source.bin");
        let mut source = fs::File::create(&source_path).unwrap();
        let mut chunk = vec![0_u8; 1024 * 1024];
        let mut seed = 0x1234_5678_u32;
        for _ in 0..64 {
            for byte in &mut chunk {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *byte = seed as u8;
            }
            source.write_all(&chunk).unwrap();
        }
        source.sync_all().unwrap();
        let store = SessionStore::new(directory.path().join("session"));
        let started = Instant::now();
        let reference = store.put_file(&source_path).unwrap();
        let checkpoint = SessionCheckpoint {
            schema_version: SCHEMA_VERSION,
            workspace_revision: 1,
            selected_asset_id: Some(7),
            assets: vec![asset(reference.clone())],
            batches: Vec::new(),
        };
        store.commit(&checkpoint).unwrap();
        let first = started.elapsed();
        let mut repeats = Vec::new();
        for _ in 0..3 {
            let started = Instant::now();
            assert_eq!(store.put_file(&source_path).unwrap(), reference);
            store.commit(&checkpoint).unwrap();
            repeats.push(started.elapsed());
        }
        println!("64 MiB first={first:?}, repeated={repeats:?}");
    }
}
