import { useEffect, useRef, useState } from 'react';

// A ref blocks duplicate submissions before React commits the disabled state.
// Views own error wording and any domain-specific follow-up work.
export function useAsyncAction(onError: (reason: unknown) => void, onStart: () => void) {
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const active = useRef(true);
  useEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);

  async function run(action: () => Promise<unknown>): Promise<void> {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      onStart();
      await action();
    } catch (reason) {
      if (active.current) onError(reason);
    } finally {
      busyRef.current = false;
      if (active.current) setBusy(false);
    }
  }
  return { busy, busyRef, run };
}
