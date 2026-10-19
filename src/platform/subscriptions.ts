import { isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { subscribeSnapshot } from '../shared/snapshotSubscription';
import type { StopSubscription } from '../shared/snapshotSubscription';

export async function subscribeNativeSnapshot<T>(
  event: string,
  read: () => Promise<T | null>,
  onSnapshot: (snapshot: T) => void,
  onError: (reason: unknown) => void,
  revision?: (snapshot: T) => number,
): Promise<StopSubscription> {
  if (!isTauri()) return () => {};
  return subscribeSnapshot({ read, revision, listen: refresh => listen(event, refresh) }, onSnapshot, onError);
}
