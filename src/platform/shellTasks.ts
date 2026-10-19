import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Language } from './preferences';

export type ShellOperation = {
  id: number;
  phase: 'queued' | 'preparing' | 'processing' | 'done' | 'failed' | 'cancelling' | 'cancelled';
  total: number;
  succeeded: number;
  failed: number;
  cancelled: number;
  outputDirectories: string[];
  error: string | null;
};
export type ShellSnapshot = { language: Language | null; operations: ShellOperation[] };
export async function getShellSnapshot(): Promise<ShellSnapshot | null> {
  return isTauri() ? invoke<ShellSnapshot>('shell_snapshot') : null;
}
export async function cancelShellOperation(id: number): Promise<void> {
  return invoke('shell_cancel', { id });
}
export async function retryShellOperation(id: number): Promise<void> {
  return invoke('shell_retry', { id });
}
export async function openShellResult(id: number, index: number): Promise<void> {
  return invoke('shell_open_result', { id, index });
}
export async function openShellWorkbench(): Promise<void> {
  return invoke('shell_open_workbench');
}
export async function listenShellWorkbench(callback: () => void): Promise<() => void> {
  return isTauri() ? listen('shell-open-workbench', callback) : () => {};
}
