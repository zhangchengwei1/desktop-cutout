import { invoke, isTauri } from '@tauri-apps/api/core';
import { subscribeNativeSnapshot } from './subscriptions';

export type AssetState = 'pending' | 'ready' | 'released' | 'failed';
export type Workflow = 'automatic' | 'selection';
export type SelectionPoint = { x: number; y: number; label: 0 | 1 };
export type Background = { kind: 'transparent' } | { kind: 'solid'; rgb: [number, number, number] } | { kind: 'image'; displayName: string };
export type BackgroundChoice = { kind: 'transparent' } | { kind: 'solid'; rgb: [number, number, number] } | { kind: 'image'; path: string };
export type TaskState =
  | 'queued'
  | 'running'
  | 'cancel_requested'
  | 'cancelled'
  | 'succeeded'
  | 'failed'
  | 'stale';

export type AssetSnapshot = {
  assetId: number;
  workflowId: number;
  displayName: string;
  width: number | null;
  height: number | null;
  generation: number;
  maskRevision: number;
  resultRevision: number;
  backgroundRevision: number;
  background: Background;
  previewRevision: number;
  previewAvailable: boolean;
  resultAvailable: boolean;
  workflow: Workflow;
  selectionPoints: SelectionPoint[];
  canUndo: boolean;
  canRedo: boolean;
  state: AssetState;
  error: string | null;
};

export type TaskSnapshot = {
  taskId: number;
  workflowId: number;
  assetId: number;
  generation: number;
  selectionGeneration: number;
  maskRevision: number;
  kind: 'preview' | 'automatic' | 'selection' | 'refinement' | 'export';
  state: TaskState;
  stage: string;
  error: string | null;
};

export type WorkspaceSnapshot = {
  revision: number;
  selectedAssetId: number | null;
  selectionGeneration: number;
  assets: AssetSnapshot[];
  tasks: TaskSnapshot[];
  batches: BatchSnapshot[];
};

export type BatchSnapshot = {
  batchId: number;
  outputDirectory: string;
  paused: boolean;
  total: number;
  queued: number;
  running: number;
  succeeded: number;
  failed: number;
  cancelled: number;
};

export type BatchItemSnapshot = {
  itemId: number;
  batchId: number;
  displayName: string;
  outputName: string;
  state: TaskState;
  stage: string;
  error: string | null;
};

export type BatchPage = { total: number; items: BatchItemSnapshot[] };
export type BatchFilter = 'all' | 'active' | 'completed' | 'issues';

export type ImportResult = {
  assetId: number;
  workflowId: number;
  taskId: number;
  selectionGeneration: number;
};

export async function getWorkspaceSnapshot(): Promise<WorkspaceSnapshot | null> {
  if (!isTauri()) return null;
  return invoke<WorkspaceSnapshot>('workspace_snapshot');
}

export async function importAsset(path: string): Promise<ImportResult> {
  return invoke<ImportResult>('import_asset', { path });
}

export async function createBatch(outputDir: string, paths: string[]): Promise<number> {
  return invoke<number>('create_batch', { outputDir, paths });
}

export async function createBatchFromZip(outputDir: string, archive: string): Promise<number> {
  return invoke<number>('create_batch_from_zip', { outputDir, archive });
}

export async function appendBatch(batchId: number, paths: string[]): Promise<number> {
  return invoke<number>('append_batch', { batchId, paths });
}

export async function appendBatchZip(batchId: number, archive: string): Promise<number> {
  return invoke<number>('append_batch_zip', { batchId, archive });
}

export async function exportBatchZip(batchId: number, destination: string): Promise<number> {
  return invoke<number>('export_batch_zip', { batchId, destination });
}

export async function getBatchItems(batchId: number, offset: number, limit = 100, filter: BatchFilter = 'all'): Promise<BatchPage> {
  return invoke<BatchPage>('batch_items', { batchId, offset, limit, filter });
}

export async function reviewBatchItem(itemId: number): Promise<ImportResult> {
  return invoke<ImportResult>('review_batch_item', { itemId });
}

export async function getBatchResultPreviewUrl(itemId: number): Promise<string> {
  return invoke<string>('batch_result_preview_url', { itemId });
}

export async function setBatchPaused(batchId: number, paused: boolean): Promise<void> {
  await invoke('set_batch_paused', { batchId, paused });
}

export async function cancelBatch(batchId: number): Promise<void> {
  await invoke('cancel_batch', { batchId });
}

export async function retryBatchItem(itemId: number): Promise<void> {
  await invoke('retry_batch_item', { itemId });
}

export async function retryFailedBatch(batchId: number): Promise<number> {
  return invoke<number>('retry_failed_batch', { batchId });
}

export type BatchFolderImport = { batchId: number; added: number; skipped: number };

export async function createBatchFromFolder(outputDir: string, folder: string): Promise<BatchFolderImport> {
  return invoke<BatchFolderImport>('create_batch_from_folder', { outputDir, folder });
}

export async function appendBatchFolder(batchId: number, folder: string): Promise<BatchFolderImport> {
  return invoke<BatchFolderImport>('append_batch_folder', { batchId, folder });
}

export async function removeBatch(batchId: number): Promise<void> {
  await invoke('remove_batch', { batchId });
}

export async function pasteAsset(): Promise<ImportResult[]> {
  return invoke<ImportResult[]>('paste_asset');
}

export async function selectAsset(assetId: number): Promise<number> {
  return invoke<number>('select_asset', { assetId });
}

export async function cancelTask(taskId: number): Promise<void> {
  await invoke('cancel_task', { taskId });
}

export async function retryPreview(assetId: number): Promise<number> {
  return invoke<number>('retry_preview', { assetId });
}

export async function releasePreview(assetId: number): Promise<void> {
  await invoke('release_preview', { assetId });
}

export async function removeAsset(assetId: number): Promise<void> {
  await invoke('remove_asset', { assetId });
}

export async function getPreviewUrl(assetId: number, result = false): Promise<string> {
  return invoke<string>('preview_url', { assetId, result });
}

export async function autoCutout(assetId: number): Promise<number> {
  return invoke<number>('auto_cutout', { assetId });
}

export async function switchWorkflow(assetId: number, workflow: Workflow): Promise<void> {
  await invoke('switch_workflow', { assetId, workflow });
}

export async function selectPoints(assetId: number, points: SelectionPoint[]): Promise<number> {
  return invoke<number>('select_points', { assetId, points });
}

export async function refineMask(assetId: number, expectedRevision: number, edge: number): Promise<number> {
  return invoke<number>('refine_mask', { assetId, expectedRevision, edge });
}

export async function setBackground(assetId: number, expectedMaskRevision: number, expectedBackgroundRevision: number, choice: BackgroundChoice): Promise<boolean> {
  return invoke<boolean>('set_background', { assetId, expectedMaskRevision, expectedBackgroundRevision, choice });
}

export type Stroke = { points: { x: number; y: number }[]; radius: number; value: 0 | 255 };

export async function paintMask(assetId: number, expectedRevision: number, stroke: Stroke): Promise<boolean> {
  return invoke<boolean>('paint_mask', { assetId, expectedRevision, stroke });
}

export async function stepHistory(assetId: number, expectedRevision: number, redo: boolean): Promise<boolean> {
  return invoke<boolean>('step_history', { assetId, expectedRevision, redo });
}

export async function exportPng(assetId: number, path: string): Promise<number> {
  return invoke<number>('export_png', { assetId, path });
}

// Event payloads only signal a revision. A fresh snapshot is authoritative,
// including after a webview reload; late async responses cannot roll it back.
export async function subscribeWorkspace(
  onSnapshot: (snapshot: WorkspaceSnapshot) => void,
  onError: (reason: unknown) => void,
): Promise<() => void> {
  return subscribeNativeSnapshot('workspace-changed', getWorkspaceSnapshot, onSnapshot, onError, snapshot => snapshot.revision);
}
