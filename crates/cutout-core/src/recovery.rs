//! Coordinates opt-in recovery with preferences and workspace writes. Every
//! checkpoint, toggle and cleanup passes through one gate, so a late save
//! cannot recreate private blobs after recovery has been disabled.
use crate::{
    jobs::Workspace,
    preferences::{Preferences, PreferencesSnapshot, PreferencesStore},
    session::SessionStore,
};
use serde::Serialize;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryStatus {
    pub enabled: bool,
    pub saved_revision: Option<u64>,
    pub storage_bytes: Option<u64>,
    pub error: Option<String>,
    pub maintenance_warning: Option<String>,
    pub blocked: bool,
}

pub struct RecoveryController {
    preferences: Arc<PreferencesStore>,
    workspace: Arc<Workspace>,
    session: SessionStore,
    gate: Mutex<RecoveryStatus>,
    on_change: Box<dyn Fn() + Send + Sync>,
}

impl RecoveryController {
    pub fn new(
        preferences: Arc<PreferencesStore>,
        workspace: Arc<Workspace>,
        session: SessionStore,
        on_change: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let enabled = preferences.snapshot().value.restore_session;
        let storage_bytes = session.disk_usage_bytes().ok();
        Self {
            preferences,
            workspace,
            session,
            gate: Mutex::new(RecoveryStatus {
                enabled,
                saved_revision: None,
                storage_bytes,
                error: None,
                maintenance_warning: None,
                blocked: false,
            }),
            on_change: Box::new(on_change),
        }
    }

    pub fn status(&self) -> RecoveryStatus {
        self.gate.lock().unwrap().clone()
    }

    /// Startup validation never overwrites an invalid or newer manifest.
    pub fn restore_at_startup(&self) -> Result<bool, String> {
        let mut state = self.gate.lock().unwrap();
        if !state.enabled {
            return Ok(false);
        }
        match self.workspace.restore(&self.session) {
            Ok(restored) => {
                state.saved_revision = restored.then(|| self.workspace.snapshot().revision - 1);
                state.error = None;
                state.blocked = false;
                self.refresh_storage(&mut state);
                drop(state);
                (self.on_change)();
                Ok(restored)
            }
            Err(error) => {
                state.error = Some(format!("会话恢复失败：{error}"));
                state.blocked = true;
                state.storage_bytes = self.session.disk_usage_bytes().ok();
                drop(state);
                (self.on_change)();
                Err(error)
            }
        }
    }

    pub fn update_other_preferences(
        &self,
        expected_revision: u64,
        value: Preferences,
    ) -> Result<PreferencesSnapshot, String> {
        let _state = self.gate.lock().unwrap();
        if value.restore_session != self.preferences.snapshot().value.restore_session {
            return Err("请使用恢复会话开关更改此设置".to_owned());
        }
        self.preferences.update(expected_revision, value)
    }

    pub fn set_enabled(
        &self,
        expected_revision: u64,
        enabled: bool,
    ) -> Result<PreferencesSnapshot, String> {
        let mut state = self.gate.lock().unwrap();
        let current = self.preferences.snapshot();
        if current.revision != expected_revision {
            return Err("设置已在另一窗口更新，请重试".to_owned());
        }
        if current.value.restore_session == enabled {
            return Ok(current);
        }
        let mut value = current.value;
        value.restore_session = enabled;
        if enabled {
            if state.blocked {
                return Err("请先关闭恢复并清理损坏的会话数据".to_owned());
            }
            // A disabled session may be a remnant of a failed cleanup. Do not
            // silently replace or delete it while opting in again.
            match self.session.load() {
                Ok(Some(_)) => return Err("存在未清理的旧会话，请先清理会话数据".to_owned()),
                Err(error) => return Err(format!("旧会话无法读取，请先清理会话数据：{error}")),
                Ok(None) => {}
            }
            match self.workspace.checkpoint(&self.session) {
                Ok(revision) => {
                    let saved = match self.preferences.update(expected_revision, value) {
                        Ok(saved) => saved,
                        Err(error) => {
                            let _ = self.session.clear();
                            return Err(error);
                        }
                    };
                    state.enabled = true;
                    state.saved_revision = Some(revision);
                    state.error = None;
                    self.refresh_storage(&mut state);
                    drop(state);
                    (self.on_change)();
                    Ok(saved)
                }
                Err(error) => {
                    // An incomplete first checkpoint must not retain private copies.
                    let _ = self.session.clear();
                    state.error = Some(format!("无法开启会话恢复：{error}"));
                    state.storage_bytes = self.session.disk_usage_bytes().ok();
                    drop(state);
                    (self.on_change)();
                    Err(error)
                }
            }
        } else {
            let saved = self.preferences.update(expected_revision, value)?;
            state.enabled = false;
            state.saved_revision = None;
            state.blocked = false;
            state.maintenance_warning = None;
            state.error = self
                .session
                .clear()
                .err()
                .map(|error| format!("会话副本清理失败：{error}"));
            state.storage_bytes = self.session.disk_usage_bytes().ok();
            let cleanup_error = state.error.clone();
            drop(state);
            (self.on_change)();
            if let Some(error) = cleanup_error {
                Err(error)
            } else {
                Ok(saved)
            }
        }
    }

    /// Retry cleanup after a disk or permissions failure. Never accepts a
    /// frontend path: the store is fixed to the application's private folder.
    pub fn clear_disabled_data(&self) -> Result<(), String> {
        let mut state = self.gate.lock().unwrap();
        if state.enabled {
            return Err("请先关闭会话恢复".to_owned());
        }
        match self.session.clear() {
            Ok(()) => {
                state.error = None;
                state.maintenance_warning = None;
                state.storage_bytes = Some(0);
                drop(state);
                (self.on_change)();
                Ok(())
            }
            Err(error) => {
                state.error = Some(format!("会话副本清理失败：{error}"));
                state.storage_bytes = self.session.disk_usage_bytes().ok();
                drop(state);
                (self.on_change)();
                Err(error)
            }
        }
    }

    /// Called from a debounced worker and again before application exit.
    pub fn save_if_enabled(&self) -> Result<(), String> {
        let mut state = self.gate.lock().unwrap();
        if !state.enabled {
            return Ok(());
        }
        if state.blocked {
            return Err(state
                .error
                .clone()
                .unwrap_or_else(|| "会话恢复已暂停".to_owned()));
        }
        if state
            .saved_revision
            .is_some_and(|saved| saved >= self.workspace.snapshot().revision)
        {
            if state.maintenance_warning.is_some() {
                self.refresh_storage(&mut state);
                drop(state);
                (self.on_change)();
            }
            return Ok(());
        }
        match self.workspace.checkpoint(&self.session) {
            Ok(revision) => {
                state.saved_revision = Some(revision);
                state.error = None;
                self.refresh_storage(&mut state);
                drop(state);
                (self.on_change)();
                Ok(())
            }
            Err(error) => {
                state.error = Some(format!("会话保存失败：{error}"));
                state.storage_bytes = self.session.disk_usage_bytes().ok();
                drop(state);
                (self.on_change)();
                Err(error)
            }
        }
    }

    pub fn report_preview_error(&self, error: String) {
        let mut state = self.gate.lock().unwrap();
        state.error = Some(format!("恢复预览失败：{error}"));
        drop(state);
        (self.on_change)();
    }

    fn refresh_storage(&self, state: &mut RecoveryStatus) {
        match self.session.prune_unreferenced() {
            Ok(bytes) => {
                state.storage_bytes = Some(bytes);
                state.maintenance_warning = None;
            }
            Err(error) => {
                state.storage_bytes = self.session.disk_usage_bytes().ok();
                state.maintenance_warning = Some(format!("会话已保存，但旧数据清理失败：{error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::Path,
        thread,
        time::{Duration, Instant},
    };

    fn controller(root: &std::path::Path) -> RecoveryController {
        RecoveryController::new(
            Arc::new(PreferencesStore::open(
                root.join("preferences.json"),
                |_| {},
            )),
            Arc::new(Workspace::new(|_| {}).unwrap()),
            SessionStore::new(root.join("session")),
            || {},
        )
    }

    #[test]
    fn opt_in_saves_and_restart_restores_then_opt_out_clears() {
        let directory = tempfile::tempdir().unwrap();
        let first = controller(directory.path());
        assert!(!first.status().enabled);
        first.set_enabled(0, true).unwrap();
        assert!(directory.path().join("session/manifest.json").exists());
        assert!(first.status().storage_bytes.unwrap() > 0);
        drop(first);
        let second = controller(directory.path());
        assert!(second.restore_at_startup().unwrap());
        assert!(second.status().enabled);
        second.set_enabled(0, false).unwrap();
        assert!(!directory.path().join("session/manifest.json").exists());
        assert_eq!(second.status().storage_bytes, Some(0));
        second.save_if_enabled().unwrap();
        assert!(!directory.path().join("session/blobs").exists());
    }

    #[test]
    fn corrupt_manifest_blocks_automatic_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        let first = controller(directory.path());
        first.set_enabled(0, true).unwrap();
        drop(first);
        let manifest = directory.path().join("session/manifest.json");
        fs::write(&manifest, b"broken").unwrap();
        let second = controller(directory.path());
        assert!(second.restore_at_startup().is_err());
        assert!(second.save_if_enabled().is_err());
        assert_eq!(fs::read(&manifest).unwrap(), b"broken");
        second.set_enabled(0, false).unwrap();
        assert!(!manifest.exists());
    }

    #[test]
    fn later_edit_is_saved_after_source_moves_and_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/images/hard-edge-opaque.png");
        let source = directory.path().join("source.png");
        fs::copy(fixture, &source).unwrap();
        let first = controller(directory.path());
        let imported = first.workspace.import(&source).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while first.workspace.snapshot().assets[0].state != crate::jobs::AssetState::Ready {
            assert!(Instant::now() < until, "preview did not finish");
            thread::sleep(Duration::from_millis(10));
        }
        first.set_enabled(0, true).unwrap();
        fs::remove_file(source).unwrap();
        first
            .workspace
            .switch_workflow(imported.asset_id, crate::jobs::Workflow::Selection)
            .unwrap();
        first.save_if_enabled().unwrap();
        drop(first);
        let second = controller(directory.path());
        assert!(second.restore_at_startup().unwrap());
        let asset = &second.workspace.snapshot().assets[0];
        assert_eq!(asset.workflow, crate::jobs::Workflow::Selection);
        assert!(second.workspace.hydrate_preview(asset.asset_id).unwrap());
        let blob = second.session.load().unwrap().unwrap().assets[0]
            .source
            .sha256
            .clone();
        second.workspace.remove(asset.asset_id).unwrap();
        let blob_path = directory.path().join("session/blobs").join(&blob);
        assert!(blob_path.exists());
        second.save_if_enabled().unwrap();
        assert!(!blob_path.exists());
    }

    #[test]
    fn ordinary_preferences_update_cannot_bypass_recovery_toggle() {
        let directory = tempfile::tempdir().unwrap();
        let controller = controller(directory.path());
        let mut value = Preferences::default();
        value.restore_session = true;
        assert!(controller.update_other_preferences(0, value).is_err());
        assert!(!controller.status().enabled);
        assert!(!directory.path().join("session").exists());
    }
}
