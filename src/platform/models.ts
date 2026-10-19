import { invoke, isTauri } from '@tauri-apps/api/core';
import { subscribeNativeSnapshot } from './subscriptions';

export type ModelState =
  | 'missing' | 'queued' | 'downloading' | 'verifying'
  | 'cancel_requested' | 'cancelled' | 'file_verified' | 'session_ready' | 'failed';

export type ModelSpec = {
  id: string;
  revision: string;
  bytes: number;
  sha256: string;
  downloadUrl: string;
  license: string;
  licenseUrl: string;
  tensorContract: string;
  verifiedBackend: string;
};

export type ModelSnapshot = {
  spec: ModelSpec;
  state: ModelState;
  receivedBytes: number;
  error: string | null;
};

export type FeatureSnapshot = {
  filesVerified: boolean;
  sessionReady: boolean;
  missingModelIds: string[];
};

export type ModelStoreSnapshot = {
  revision: number;
  models: ModelSnapshot[];
  features: {
    automatic: FeatureSnapshot;
    selection: FeatureSnapshot;
    matting: FeatureSnapshot;
  };
};

export async function getModelSnapshot(): Promise<ModelStoreSnapshot | null> {
  if (!isTauri()) return null;
  return invoke<ModelStoreSnapshot>('model_snapshot');
}

export async function downloadModel(modelId: string): Promise<void> {
  await invoke('download_model', { modelId });
}

export async function cancelModelDownload(modelId: string): Promise<void> {
  await invoke('cancel_model_download', { modelId });
}

export async function clearModel(modelId: string): Promise<void> {
  await invoke('clear_model', { modelId });
}

export async function openModelLicense(modelId: string): Promise<void> {
  await invoke('open_model_license', { modelId });
}

export async function subscribeModels(
  onSnapshot: (snapshot: ModelStoreSnapshot) => void,
  onError: (reason: unknown) => void,
): Promise<() => void> {
  return subscribeNativeSnapshot('models-changed', getModelSnapshot, onSnapshot, onError, snapshot => snapshot.revision);
}
