import { useEffect, useRef } from 'react';
import type { StopSubscription } from './snapshotSubscription';

type Subscribe<T> = (
  onValue: (value: T) => void,
  onError: (reason: unknown) => void,
) => Promise<StopSubscription>;

// The callbacks stay current without tearing down an event subscription on
// every render. A late listener registration is disposed after unmount/retry.
export function useSubscription<T>(
  subscribe: Subscribe<T>,
  onValue: (value: T) => void,
  onError: (reason: unknown) => void,
  retry = 0,
): void {
  const callbacks = useRef({ onValue, onError });
  useEffect(() => { callbacks.current = { onValue, onError }; });
  useEffect(() => {
    let active = true;
    let stop: StopSubscription | undefined;
    const report = (reason: unknown) => { if (active) callbacks.current.onError(reason); };
    void subscribe(value => { if (active) callbacks.current.onValue(value); }, report)
      .then(unlisten => { if (active) stop = unlisten; else unlisten(); }, report);
    return () => { active = false; stop?.(); };
  }, [subscribe, retry]);
}
