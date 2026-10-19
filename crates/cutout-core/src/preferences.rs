//! Versioned, atomic preferences shared by desktop windows. User images and
//! editing state remain in the workspace, never in this small settings file.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Zh,
    En,
    Ja,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportNaming {
    Cutout,
    Clearframe,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellCompletion {
    #[default]
    Progress,
    OpenFolder,
    Silent,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    #[serde(default)]
    pub shortcuts: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub shell_completion: ShellCompletion,
    pub schema_version: u32,
    pub language: Option<Language>,
    pub export_naming: ExportNaming,
    #[serde(default)]
    pub restore_session: bool,
    #[serde(default)]
    pub batch_output_directory: Option<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            shortcuts: Default::default(),
            shell_completion: ShellCompletion::Progress,
            schema_version: SCHEMA_VERSION,
            language: None,
            export_naming: ExportNaming::Cutout,
            restore_session: false,
            batch_output_directory: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesSnapshot {
    pub revision: u64,
    pub value: Preferences,
    pub load_error: Option<String>,
}

struct State {
    snapshot: PreferencesSnapshot,
    writable: bool,
}

pub struct PreferencesStore {
    path: PathBuf,
    state: Mutex<State>,
    on_change: Box<dyn Fn(u64) + Send + Sync>,
}

impl PreferencesStore {
    pub fn open(path: PathBuf, on_change: impl Fn(u64) + Send + Sync + 'static) -> Self {
        let (value, load_error, writable) = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Preferences>(&bytes) {
                Ok(value)
                    if value.schema_version == SCHEMA_VERSION
                        && crate::shortcuts::validate(&value.shortcuts).is_ok() =>
                {
                    (value, None, true)
                }
                Ok(_) => (
                    Preferences::default(),
                    Some("设置文件版本或快捷键无效，原文件保持不变".to_owned()),
                    false,
                ),
                Err(error) => (
                    Preferences::default(),
                    Some(format!("设置文件无法读取：{error}")),
                    false,
                ),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Preferences::default(), None, true)
            }
            Err(error) => (
                Preferences::default(),
                Some(format!("设置文件无法读取：{error}")),
                false,
            ),
        };
        Self {
            path,
            state: Mutex::new(State {
                snapshot: PreferencesSnapshot {
                    revision: 0,
                    value,
                    load_error,
                },
                writable,
            }),
            on_change: Box::new(on_change),
        }
    }

    pub fn snapshot(&self) -> PreferencesSnapshot {
        self.state.lock().unwrap().snapshot.clone()
    }

    pub fn update(
        &self,
        expected_revision: u64,
        value: Preferences,
    ) -> Result<PreferencesSnapshot, String> {
        crate::shortcuts::validate(&value.shortcuts)?;
        if value.schema_version != SCHEMA_VERSION {
            return Err("设置文件版本不兼容".to_owned());
        }
        if value.batch_output_directory.as_ref().is_some_and(|path| {
            path.len() > 4096 || path.contains('\0') || !Path::new(path).is_absolute()
        }) {
            return Err("批量输出文件夹必须是有效的绝对路径".to_owned());
        }
        let mut state = self.state.lock().unwrap();
        if !state.writable {
            return Err(state
                .snapshot
                .load_error
                .clone()
                .unwrap_or_else(|| "设置文件不可写".to_owned()));
        }
        if state.snapshot.revision != expected_revision {
            return Err("设置已在另一窗口更新，请重试".to_owned());
        }
        write_atomic(&self.path, &value).map_err(|error| error.to_string())?;
        state.snapshot.revision += 1;
        state.snapshot.value = value;
        state.snapshot.load_error = None;
        let snapshot = state.snapshot.clone();
        drop(state);
        (self.on_change)(snapshot.revision);
        Ok(snapshot)
    }
}

fn write_atomic(path: &Path, value: &Preferences) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().ok_or("设置文件目录无效")?;
    fs::create_dir_all(parent)?;
    let mut pending = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut pending, value)?;
    pending.write_all(b"\n")?;
    pending.as_file().sync_all()?;
    pending.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_preferences_default_to_progress_and_all_completion_choices_persist() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        fs::write(
            &path,
            br#"{"schemaVersion":1,"language":null,"exportNaming":"cutout"}"#,
        )
        .unwrap();
        let store = PreferencesStore::open(path.clone(), |_| {});
        assert_eq!(
            store.snapshot().value.shell_completion,
            ShellCompletion::Progress
        );
        for choice in [
            ShellCompletion::OpenFolder,
            ShellCompletion::Silent,
            ShellCompletion::Progress,
        ] {
            let snapshot = store.snapshot();
            let mut value = snapshot.value;
            value.shell_completion = choice;
            store.update(snapshot.revision, value).unwrap();
            assert_eq!(
                PreferencesStore::open(path.clone(), |_| {})
                    .snapshot()
                    .value
                    .shell_completion,
                choice
            );
        }
    }
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn updates_are_atomic_versioned_and_reopen_with_the_saved_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        let notifications = Arc::new(AtomicUsize::new(0));
        let notified = Arc::clone(&notifications);
        let store = PreferencesStore::open(path.clone(), move |_| {
            notified.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(store.snapshot().value, Preferences::default());
        let value = Preferences {
            shortcuts: std::collections::BTreeMap::from([("import".into(), "CmdOrCtrl+I".into())]),
            shell_completion: ShellCompletion::Progress,
            schema_version: SCHEMA_VERSION,
            language: Some(Language::Ja),
            export_naming: ExportNaming::Clearframe,
            restore_session: false,
            batch_output_directory: Some(directory.path().to_string_lossy().into_owned()),
        };
        let saved = store.update(0, value.clone()).unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        assert!(store.update(0, Preferences::default()).is_err());
        let mut conflict = value.clone();
        conflict
            .shortcuts
            .insert("export".into(), "CmdOrCtrl+I".into());
        assert!(store.update(1, conflict).is_err());
        assert_eq!(store.snapshot().value, value);
        assert_eq!(PreferencesStore::open(path, |_| {}).snapshot().value, value);
    }

    #[test]
    fn failed_write_keeps_the_last_saved_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        let store = PreferencesStore::open(path.clone(), |_| {});
        fs::create_dir(&path).unwrap();
        let changed = Preferences {
            language: Some(Language::En),
            ..Preferences::default()
        };
        assert!(store.update(0, changed).is_err());
        assert_eq!(store.snapshot().revision, 0);
        assert_eq!(store.snapshot().value, Preferences::default());
    }

    #[test]
    fn a_newer_schema_is_not_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        fs::write(
            &path,
            r#"{"schemaVersion":2,"language":"ja","exportNaming":"cutout"}"#,
        )
        .unwrap();
        let store = PreferencesStore::open(path.clone(), |_| {});
        assert!(store.snapshot().load_error.is_some());
        assert!(store.update(0, Preferences::default()).is_err());
        assert!(fs::read_to_string(path)
            .unwrap()
            .contains("\"schemaVersion\":2"));
    }

    #[test]
    fn malformed_settings_are_not_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        fs::write(&path, b"{broken").unwrap();
        let store = PreferencesStore::open(path.clone(), |_| {});
        assert!(store.snapshot().load_error.is_some());
        assert!(store.update(0, Preferences::default()).is_err());
        assert_eq!(fs::read(path).unwrap(), b"{broken");
    }

    #[test]
    fn old_preferences_default_to_recovery_off() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        fs::write(
            &path,
            r#"{"schemaVersion":1,"language":"en","exportNaming":"cutout"}"#,
        )
        .unwrap();
        let store = PreferencesStore::open(path, |_| {});
        assert!(!store.snapshot().value.restore_session);
        assert!(store.snapshot().value.batch_output_directory.is_none());
        assert!(store.snapshot().load_error.is_none());
    }

    #[test]
    fn invalid_batch_output_directory_does_not_change_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let store = PreferencesStore::open(directory.path().join("preferences.json"), |_| {});
        let value = Preferences {
            batch_output_directory: Some("relative/results".to_owned()),
            ..Preferences::default()
        };
        assert!(store.update(0, value).is_err());
        assert_eq!(store.snapshot().revision, 0);
        assert_eq!(store.snapshot().value, Preferences::default());
    }
}
