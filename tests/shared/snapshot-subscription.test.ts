import assert from 'node:assert/strict';
import { test } from 'node:test';
import { subscribeSnapshot } from '../../src/shared/snapshotSubscription.ts';

const tick = () => new Promise<void>(resolve => setImmediate(resolve));

function fixture() {
  let signal = () => {};
  let stopped = 0;
  const reads: Array<{ resolve: (value: { revision: number } | null) => void; reject: (reason: unknown) => void }> = [];
  const values: number[] = [];
  const errors: unknown[] = [];
  const start = () => subscribeSnapshot({
    read: () => new Promise<{ revision: number } | null>((resolve, reject) => { reads.push({ resolve, reject }); }),
    listen: async (refresh: () => void) => { signal = refresh; return () => { stopped++; }; },
    revision: snapshot => snapshot.revision,
  }, snapshot => values.push(snapshot.revision), reason => errors.push(reason));
  return { start, reads, values, errors, signal: () => signal(), stopped: () => stopped };
}

test('coalesces bursts without losing the last change or publishing older revisions', async () => {
  const f = fixture();
  const stop = await f.start();
  for (let i = 0; i < 20; i++) f.signal();
  assert.equal(f.reads.length, 1);
  f.reads[0].resolve({ revision: 5 });
  await tick();
  assert.equal(f.reads.length, 2);
  f.reads[1].resolve({ revision: 4 });
  await tick();
  assert.deepEqual(f.values, [5]);
  f.signal();
  f.reads[2].resolve({ revision: 6 });
  await tick();
  assert.deepEqual(f.values, [5, 6]);
  stop(); stop();
  assert.equal(f.stopped(), 1);
});

test('disposal suppresses pending reads and errors', async () => {
  const f = fixture();
  const stop = await f.start();
  f.signal();
  stop();
  f.reads[0].reject(new Error('late failure'));
  await tick();
  assert.deepEqual(f.values, []);
  assert.deepEqual(f.errors, []);
  assert.equal(f.reads.length, 1);
});

test('a failed read can recover on the next signal', async () => {
  const f = fixture();
  const stop = await f.start();
  f.reads[0].reject('offline');
  await tick();
  assert.deepEqual(f.errors, ['offline']);
  f.signal();
  f.reads[1].resolve({ revision: 1 });
  await tick();
  assert.deepEqual(f.values, [1]);
  stop();
});

test('unversioned status sources publish each fresh value', async () => {
  let signal = () => {};
  let value = false;
  const seen: boolean[] = [];
  const stop = await subscribeSnapshot({
    read: async () => value,
    listen: async refresh => { signal = refresh; return () => {}; },
  }, snapshot => seen.push(snapshot), reason => { throw reason; });
  await tick();
  value = true; signal();
  await tick();
  assert.deepEqual(seen, [false, true]);
  stop();
});
