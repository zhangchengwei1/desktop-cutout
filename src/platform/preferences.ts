import { invoke, isTauri } from '@tauri-apps/api/core';
import { subscribeNativeSnapshot } from './subscriptions';
import type { ShortcutOverrides } from '../shared/shortcuts';

export type Language = 'zh' | 'en' | 'ja';
export type ExportNaming = 'cutout' | 'clearframe';
export type ShellCompletion = 'progress' | 'open_folder' | 'silent';
export type Preferences = {
  shortcuts: ShortcutOverrides;
  shellCompletion: ShellCompletion;
  schemaVersion: number;
  language: Language | null;
  exportNaming: ExportNaming;
  restoreSession: boolean;
  batchOutputDirectory: string | null;
};
export type PreferencesSnapshot = {
  revision: number;
  value: Preferences;
  loadError: string | null;
};
export type RecoveryStatus = {
  enabled: boolean;
  savedRevision: number | null;
  storageBytes: number | null;
  error: string | null;
  maintenanceWarning: string | null;
  blocked: boolean;
};

export async function getPreferencesSnapshot(): Promise<PreferencesSnapshot | null> {
  if (!isTauri()) return null;
  return invoke<PreferencesSnapshot>('preferences_snapshot');
}

export async function configureContextMenu(): Promise<'configured' | 'cancelled' | 'installer_required'> {
  return invoke('configure_context_menu');
}

export async function updatePreferences(expectedRevision: number, value: Preferences): Promise<PreferencesSnapshot> {
  return invoke<PreferencesSnapshot>('update_preferences', { expectedRevision, value });
}

export async function setSessionRecovery(expectedRevision: number, enabled: boolean): Promise<PreferencesSnapshot> {
  return invoke<PreferencesSnapshot>('set_session_recovery', { expectedRevision, enabled });
}

export async function clearSessionData(): Promise<void> {
  return invoke<void>('clear_session_data');
}

export async function getRecoveryStatus(): Promise<RecoveryStatus | null> {
  if (!isTauri()) return null;
  return invoke<RecoveryStatus>('recovery_status');
}

export async function subscribeRecovery(
  onSnapshot: (snapshot: RecoveryStatus) => void,
  onError: (reason: unknown) => void,
): Promise<() => void> {
  return subscribeNativeSnapshot('recovery-changed', getRecoveryStatus, onSnapshot, onError);
}

// Returns true when a native settings window was opened (macOS).
export async function openSettings(): Promise<boolean> {
  if (!isTauri()) return false;
  return invoke<boolean>('open_settings');
}

export async function subscribePreferences(
  onSnapshot: (snapshot: PreferencesSnapshot) => void,
  onError: (reason: unknown) => void,
): Promise<() => void> {
  return subscribeNativeSnapshot('preferences-changed', getPreferencesSnapshot, onSnapshot, onError, snapshot => snapshot.revision);
}
