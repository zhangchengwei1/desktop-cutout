import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ShortcutAction } from '../shared/shortcuts';

export async function listenApplicationActions(onAction: (action: ShortcutAction) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  return listen<ShortcutAction>('application-action', event => onAction(event.payload));
}

export async function setShortcutRecording(recording: boolean): Promise<void> {
  if (isTauri()) await invoke('set_shortcut_recording', { recording });
}
