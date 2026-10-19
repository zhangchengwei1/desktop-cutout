import { resolveSystemLanguage, useMessages } from '../i18n';
import { startTransition, useEffect, useState } from 'react';
import { PageBoundary } from './PageBoundary';
import { useSubscription } from '../shared/useSubscription';
import { ShortcutSettings } from './ShortcutSettings';
import { setMenuLanguage } from '../platform/utilityWindows';
import { isDesktop } from '../platform/runtime';
import { clearSessionData, configureContextMenu, getPreferencesSnapshot, getRecoveryStatus, setSessionRecovery, subscribePreferences, subscribeRecovery, updatePreferences } from '../platform/preferences';
import type { ShellCompletion, ExportNaming, Language, Preferences, PreferencesSnapshot, RecoveryStatus } from '../platform/preferences';

function formatBytes(bytes: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB'];
  let size = bytes;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) { size /= 1024; unit++; }
  return `${size.toFixed(unit === 0 || size >= 10 ? 0 : 1)} ${units[unit]}`;
}

export function SettingsPanel({ language, onBack, onModels }: { language?: Language; onBack?: () => void; onModels?: () => void }) {
  const [snapshot, setSnapshot] = useState<PreferencesSnapshot | null>(null);
  const [recovery, setRecovery] = useState<RecoveryStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [tab, setTab] = useState<'general' | 'shortcuts' | 'storage' | 'privacy'>('general');
  const [retry, setRetry] = useState(0);
  const [confirmClear, setConfirmClear] = useState(false);
  const systemLanguage: Language = resolveSystemLanguage();
  const resolvedLanguage = snapshot?.value.language ?? language ?? systemLanguage;
  const { copy: t, extra: e } = useMessages('settings', resolvedLanguage);
  const value: Preferences = snapshot?.value ?? { shortcuts: {}, shellCompletion: 'progress', schemaVersion: 1, language: null, exportNaming: 'cutout', restoreSession: false, batchOutputDirectory: null };
  const disabled = busy || !snapshot || !!snapshot.loadError;
  useEffect(() => { document.documentElement.lang = resolvedLanguage; void setMenuLanguage(resolvedLanguage).catch(reason => setError(String(reason))); }, [resolvedLanguage]);
  useEffect(() => { setError(null); }, [retry]);
  useSubscription(subscribePreferences, value => startTransition(() => setSnapshot(value)), reason => setError(String(reason)), retry);
  useSubscription(subscribeRecovery, setRecovery, reason => setError(String(reason)), retry);
  async function save(value: Preferences) {
    if (!snapshot || busy) return;
    setBusy(true);
    setError(null);
    try { const next = await updatePreferences(snapshot.revision, value); startTransition(() => setSnapshot(next)); }
    catch (reason) {
      setError(String(reason));
      try { setSnapshot(await getPreferencesSnapshot()); } catch { /* Preserve the last readable value. */ }
    } finally { setBusy(false); }
  }
  async function changeRecovery(enabled: boolean) {
    if (!snapshot || busy) return;
    setBusy(true);
    setError(null);
    try { setSnapshot(await setSessionRecovery(snapshot.revision, enabled)); }
    catch (reason) {
      setError(String(reason));
      try { setSnapshot(await getPreferencesSnapshot()); } catch { /* Keep last readable value. */ }
    } finally {
      try { setRecovery(await getRecoveryStatus()); } catch { /* Event listener also refreshes. */ }
      setBusy(false);
    }
  }
  async function clearRecovery() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try { await clearSessionData(); setRecovery(await getRecoveryStatus()); }
    catch (reason) { setError(String(reason)); }
    finally { setConfirmClear(false); setBusy(false); }
  }
  async function configureMenus() {
    if (disabled) return;
    setBusy(true);
    setError(null);
    try { if (await configureContextMenu() === 'installer_required') setError(t.shellInstallerHelp); }
    catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  }
  return <section className="settings-panel" lang={resolvedLanguage} aria-labelledby="settings-title">
    <div className="settings-heading">{onBack && <button type="button" className="text-button" onClick={onBack}>{t.back}</button>}<h2 id="settings-title">{t.title}</h2><p>{t.intro}</p></div>
    <nav className="settings-tabs" aria-label={t.title}>{(['general', 'shortcuts', 'storage', 'privacy'] as const).map((item) => <button type="button" key={item} className={tab === item ? 'active' : ''} aria-current={tab === item ? 'page' : undefined} onClick={() => setTab(item)}>{e[item]}</button>)}</nav>
    {!snapshot && <p className="settings-muted" role="status">{error ?? (isDesktop() ? t.loading : e.preview)}{isDesktop() && error && <button type="button" onClick={() => setRetry((number) => number + 1)}>{e.retry}</button>}</p>}
    {snapshot?.loadError && <p className="settings-error" role="alert">{snapshot.loadError}</p>}
    {tab === 'general' && <div className="settings-fields">
      <label className="settings-row"><span>{t.language}<small>{t.languageHelp}</small></span><select value={value.language ?? 'system'} disabled={disabled} onChange={(event) => void save({ ...value, language: event.target.value === 'system' ? null : event.target.value as Language })}><option value="system">{t.system}</option><option value="zh">中文</option><option value="en">English</option><option value="ja">日本語</option></select></label>
      <label className="settings-row"><span>{t.naming}<small>{t.namingHelp}</small></span><select value={value.exportNaming} disabled={disabled} onChange={(event) => void save({ ...value, exportNaming: event.target.value as ExportNaming })}><option value="cutout">-cutout.png</option><option value="clearframe">-clearframe.png</option></select></label>
      <label className="settings-row"><span>{t.shellCompletion}<small>{t.shellCompletionHelp}</small></span><select value={value.shellCompletion ?? 'progress'} disabled={disabled} onChange={(event) => void save({ ...value, shellCompletion: event.target.value as ShellCompletion })}><option value="progress">{t.shellProgress}</option><option value="open_folder">{t.shellOpenFolder}</option><option value="silent">{t.shellSilent}</option></select></label>
      <div className="settings-row"><span>{t.shellMenus}<small>{t.shellMenusHelp}</small></span><button type="button" disabled={disabled} onClick={() => void configureMenus()}>{t.shellManage}</button></div>
      <div className="settings-row"><span>{e.format}<small>{e.formatHelp}</small></span><strong className="settings-format">PNG</strong></div>
    </div>}
    {tab === 'shortcuts' && <PageBoundary language={resolvedLanguage} title={e.shortcuts}><ShortcutSettings language={resolvedLanguage} overrides={value.shortcuts ?? {}} disabled={disabled} onSave={shortcuts => save({ ...value, shortcuts })} /></PageBoundary>}
    {tab === 'storage' && <div className="settings-fields">
      <label className="settings-row"><span>{t.recovery}<small>{t.recoveryHelp}</small></span><input type="checkbox" role="switch" checked={value.restoreSession} disabled={disabled} onChange={(event) => void changeRecovery(event.target.checked)} /></label>
      {recovery?.storageBytes != null && <p className="settings-usage">{t.usage}: {formatBytes(recovery.storageBytes)}</p>}
      {recovery?.error && <p className="settings-error" role="alert">{recovery.error}</p>}
      {recovery?.maintenanceWarning && <p className="settings-error" role="alert">{recovery.maintenanceWarning}</p>}
      <div className="settings-row"><span>{e.copies}<small>{e.clearHelp}</small></span></div>
      {!value.restoreSession && <div className="settings-action">{confirmClear ? <><button type="button" disabled={disabled} onClick={() => void clearRecovery()}>{e.confirmClear}</button><button type="button" className="text-button" disabled={busy} onClick={() => setConfirmClear(false)}>{e.cancel}</button></> : <button type="button" disabled={disabled} onClick={() => setConfirmClear(true)}>{t.clear}</button>}</div>}
    </div>}
    {tab === 'privacy' && <div className="settings-privacy"><section><h3>{e.privacy}</h3><p>{e.privacyBody}</p></section><section><h3>{e.quality}</h3><p>{e.qualityBody}</p>{onModels && <button type="button" onClick={onModels}>{e.models}</button>}</section></div>}
    {busy && <p className="settings-muted" role="status">{e.saving}</p>}
    {error && snapshot && <p className="settings-error" role="alert">{error}<button type="button" onClick={() => setRetry((number) => number + 1)}>{e.retry}</button></p>}
  </section>;
}
