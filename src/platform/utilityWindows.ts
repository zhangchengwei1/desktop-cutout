import { invoke, isTauri } from '@tauri-apps/api/core';
import type { Language } from './preferences';

export type UtilityView = 'settings' | 'resources' | 'help' | 'about';

export async function openUtilityWindow(view: UtilityView): Promise<void> {
  if (!isTauri()) {
    const opened = window.open(`/?utility=${view}`, `clearframe-${view}`);
    if (!opened) throw new Error('Please allow this local preview to open a window.');
    return;
  }
  await invoke('open_utility_window', { view });
}

export async function setMenuLanguage(language: Language): Promise<void> {
  if (isTauri()) await invoke('set_menu_language', { language });
}
