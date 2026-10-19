//! Portable application accelerators. Standard text editing keys remain native.
use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

pub fn defaults() -> &'static BTreeMap<String, String> {
    static DEFAULTS: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
        serde_json::from_str(include_str!("../../../resources/shortcuts.json"))
            .expect("invalid bundled shortcuts")
    });
    &DEFAULTS
}

pub fn resolved(overrides: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut result = defaults().clone();
    result.extend(overrides.clone());
    result
}

pub fn validate(overrides: &BTreeMap<String, String>) -> Result<(), String> {
    if overrides.keys().any(|key| !defaults().contains_key(key)) {
        return Err("[shortcut_invalid] Unknown shortcut action".into());
    }
    let mut used = HashSet::new();
    for value in resolved(overrides)
        .values()
        .filter(|value| !value.is_empty())
    {
        let parts: Vec<_> = value.split('+').collect();
        let key = *parts.last().unwrap();
        let modifiers = &parts[..parts.len() - 1];
        let function_key = key
            .strip_prefix('F')
            .and_then(|key| key.parse::<u8>().ok())
            .is_some_and(|key| (1..=12).contains(&key));
        let valid_key = function_key
            || key.len() == 1
                && key.bytes().all(|key| {
                    key.is_ascii_uppercase() || key.is_ascii_digit() || b",.;/[]-=\\".contains(&key)
                });
        let canonical: Vec<_> = ["CmdOrCtrl", "Alt", "Shift"]
            .into_iter()
            .filter(|item| modifiers.contains(item))
            .collect();
        if !valid_key
            || modifiers != canonical
            || (!function_key && !modifiers.contains(&"CmdOrCtrl") && !modifiers.contains(&"Alt"))
        {
            return Err(
                "[shortcut_invalid] Use Ctrl/Command or Alt with a letter, number or function key"
                    .into(),
            );
        }
        if [
            "CmdOrCtrl+Q",
            "CmdOrCtrl+W",
            "CmdOrCtrl+C",
            "CmdOrCtrl+X",
            "CmdOrCtrl+V",
            "CmdOrCtrl+A",
            "CmdOrCtrl+Z",
            "CmdOrCtrl+Shift+Z",
            "CmdOrCtrl+Y",
            "Alt+F4",
        ]
        .contains(&value.as_str())
        {
            return Err(
                "[shortcut_reserved] This shortcut is reserved for system or text editing".into(),
            );
        }
        if !used.insert(value.clone()) {
            return Err("[shortcut_conflict] Shortcut is already assigned".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conflicts_defaults_reserved_keys_and_unknown_actions_are_rejected() {
        assert!(validate(&BTreeMap::new()).is_ok());
        for (action, key) in [
            ("export", "CmdOrCtrl+O"),
            ("import", "CmdOrCtrl+V"),
            ("import", "Shift+R"),
            ("import", "Shift+CmdOrCtrl+R"),
            ("import", "Alt+F4"),
            ("unknown", "F2"),
        ] {
            assert!(validate(&BTreeMap::from([(action.into(), key.into())])).is_err());
        }
        assert!(validate(&BTreeMap::from([
            ("import".into(), "".into()),
            ("export".into(), "CmdOrCtrl+O".into())
        ]))
        .is_ok());
    }
}
