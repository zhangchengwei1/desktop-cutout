export type StopSubscription = () => void;

export type SnapshotSource<T> = {
  read: () => Promise<T | null>;
  listen: (refresh: () => void) => Promise<StopSubscription>;
  revision?: (snapshot: T) => number;
};

// Signals are hints: serialize reads, coalesce bursts, then fetch once more if
// an event arrived during a read. Disposal suppresses late results and errors.
export async function subscribeSnapshot<T>(
  source: SnapshotSource<T>,
  onSnapshot: (snapshot: T) => void,
  onError: (reason: unknown) => void,
): Promise<StopSubscription> {
  let active = true;
  let reading = false;
  let pending = false;
  let latestRevision = -1;
  const refresh = async () => {
    pending = true;
    if (reading || !active) return;
    reading = true;
    try {
      while (pending && active) {
        pending = false;
        try {
          const snapshot = await source.read();
          if (!active || snapshot === null) continue;
          const revision = source.revision?.(snapshot);
          if (revision !== undefined && revision <= latestRevision) continue;
          if (revision !== undefined) latestRevision = revision;
          onSnapshot(snapshot);
        } catch (reason) {
          if (active) onError(reason);
        }
      }
    } finally {
      reading = false;
    }
  };
  const unlisten = await source.listen(() => { void refresh(); });
  void refresh();
  return () => {
    if (!active) return;
    active = false;
    unlisten();
  };
}
