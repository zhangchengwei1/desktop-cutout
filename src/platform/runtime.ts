import { invoke, isTauri } from '@tauri-apps/api/core';

// 与 cutout-core 的 RuntimeInfo 对齐；浏览器预览不模拟原生调用成功。
export type RuntimeInfo = {
  coreVersion: string;
  inferenceReady: boolean;
};

export function isDesktop(): boolean {
  return isTauri();
}

export async function getRuntimeInfo(): Promise<RuntimeInfo | null> {
  if (!isTauri()) return null;
  return invoke<RuntimeInfo>('runtime_info');
}
