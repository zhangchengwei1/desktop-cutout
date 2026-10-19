import { useState } from 'react';
import { useMessages } from '../i18n';
import type { Language } from '../platform/preferences';
import { setShortcutRecording } from '../platform/applicationMenu';
import { displayShortcut, resolveShortcuts, shortcutActions, shortcutFromEvent, shortcutIssue } from '../shared/shortcuts';
import type { ShortcutAction, ShortcutOverrides } from '../shared/shortcuts';

export function ShortcutSettings({ language, overrides, disabled, onSave }: {
  language: Language; overrides: ShortcutOverrides; disabled: boolean; onSave: (overrides: ShortcutOverrides) => Promise<void>;
}) {
  const t = useMessages('shortcuts', language);
  const [recording, setRecording] = useState<ShortcutAction | null>(null);
  const [error, setError] = useState<string | null>(null);
  const resolved = resolveShortcuts(overrides);
  async function stop() {
    setRecording(null);
    try { await setShortcutRecording(false); } catch (reason) { setError(String(reason)); }
  }
  return <div className="shortcut-settings">
    <p className="settings-muted">{t.help}</p>
    <div className="shortcut-settings-list">{shortcutActions.map(action => <div className="settings-row" key={action}>
      <span id={`shortcut-${action}`}>{t.actions[action]}</span>
      <div className="shortcut-controls">
        <button type="button" className="shortcut-recorder" aria-labelledby={`shortcut-${action}`} aria-describedby="shortcut-recording-help" disabled={disabled} onClick={async event => {
          const button = event.currentTarget;
          button.focus();
          setError(null);
          try {
            await setShortcutRecording(true);
            if (document.activeElement !== button) { await setShortcutRecording(false); return; }
            setRecording(action);
          } catch (reason) { setError(String(reason)); }
        }} onBlur={() => { if (recording === action) void stop(); }} onKeyDown={event => {
          if (recording !== action || event.key === 'Tab') return;
          event.preventDefault(); event.stopPropagation();
          if (event.key === 'Escape') { void stop(); return; }
          if (/^(Control|Meta|Alt|Shift)/.test(event.code)) return;
          const shortcut = shortcutFromEvent(event.nativeEvent);
          if (!shortcut) { setError(t.invalid); return; }
          const issue = shortcutIssue(action, shortcut, overrides);
          if (issue) { setError(t[issue]); return; }
          void stop().then(() => onSave({ ...overrides, [action]: shortcut }));
        }}>{recording === action ? t.recording : displayShortcut(resolved[action]) || t.unassigned}</button>
        <button type="button" className="text-button" disabled={disabled || !resolved[action]} aria-label={`${t.disable}: ${t.actions[action]}`} onClick={() => { void stop().then(() => onSave({ ...overrides, [action]: '' })); }}>{t.disable}</button>
      </div>
    </div>)}</div>
    <p id="shortcut-recording-help" className="settings-muted">{t.recordHelp}</p>
    <button type="button" disabled={disabled || !Object.keys(overrides).length} onClick={() => { setError(null); void stop().then(() => onSave({})); }}>{t.reset}</button>
    {error && <p className="settings-error" role="alert">{error}</p>}
  </div>;
}
