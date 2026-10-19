import { translate, useMessages } from '../i18n';
import { useEffect, useRef, useState } from 'react';
import { OperationError } from './OperationError';
import { chooseImageDirectory, chooseImages, chooseOutputDirectory, chooseZipArchive, chooseZipDestination } from '../platform/nativeFiles';
import { getPreferencesSnapshot, updatePreferences } from '../platform/preferences';
import type { Language, PreferencesSnapshot } from '../platform/preferences';
import { appendBatch, appendBatchFolder, appendBatchZip, cancelBatch, createBatch, createBatchFromFolder, createBatchFromZip, exportBatchZip, getBatchItems, getBatchResultPreviewUrl, removeBatch, retryBatchItem, retryFailedBatch, reviewBatchItem, setBatchPaused } from '../platform/workspace';
import type { BatchFilter, BatchPage, BatchSnapshot, TaskState } from '../platform/workspace';

type LoadedPage = { batchId: number; offset: number; filter: BatchFilter; value: BatchPage };
type PageError = { batchId: number; offset: number; filter: BatchFilter; message: string };
type BatchNotice = { kind: 'folder'; added: number; skipped: number } | { kind: 'zip'; destination: string; count: number };
const PAGE_SIZE = 50;

function BatchResultPreview({ itemId, outputName, language, onClose }: {
  itemId: number; outputName: string; language: Language; onClose: () => void;
}) {
  const t = useMessages('batch', language).resultCopy;
  const previewRef = useRef<HTMLDivElement>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    if (!loading) previewRef.current?.scrollIntoView({ block: 'nearest' });
  }, [loading, failed]);
  useEffect(() => {
    let active = true;
    setUrl(null); setFailed(false); setLoading(true);
    void getBatchResultPreviewUrl(itemId).then(
      (value) => { if (active) setUrl(`${value}?attempt=${retry}`); },
      () => { if (active) { setFailed(true); setLoading(false); } },
    );
    return () => { active = false; };
  }, [itemId, retry]);
  return <div ref={previewRef} className="batch-result-preview" role="region" aria-labelledby={`batch-result-${itemId}`}>
    <div className="batch-result-heading"><h3 id={`batch-result-${itemId}`}>{t.preview}</h3><button type="button" className="text-button" onClick={onClose}>{t.close}</button></div>
    <p className="batch-result-name">{outputName}</p>
    {loading && <p role="status">{t.loading}</p>}
    {failed ? <div className="batch-result-error"><p role="alert">{t.failed}</p><button type="button" onClick={() => setRetry((value) => value + 1)}>{t.retry}</button></div>
      : url && <div className="batch-result-image"><img key={url} src={url} alt={`${t.preview}: ${outputName}`} hidden={loading} onLoad={() => setLoading(false)} onError={() => { setFailed(true); setLoading(false); }} /></div>}
  </div>;
}

export function BatchPanel({ language, preferences, batches, revision, modelReady, nativeAvailable, visible, onModels, onReview }: {
  language: Language;
  preferences: PreferencesSnapshot | null;
  batches: BatchSnapshot[];
  revision: number;
  modelReady: boolean;
  nativeAvailable: boolean;
  visible: boolean;
  onModels: () => void;
  onReview: () => void;
}) {
  const { copy: t, extras: e, resultCopy: r } = useMessages('batch', language);
  const [outputDirectory, setOutputDirectory] = useState<string | null>(null);
  const [previewItemId, setPreviewItemId] = useState<number | null>(null);
  const [filter, setFilter] = useState<BatchFilter>('all');
  const busyRef = useRef(false);
  const pendingBatchRef = useRef<number | null>(null);
  const [selectedBatchId, setSelectedBatchId] = useState<number | null>(null);
  const [offset, setOffset] = useState(0);
  const [loaded, setLoaded] = useState<LoadedPage | null>(null);
  const [pageError, setPageError] = useState<PageError | null>(null);
  const [retryKey, setRetryKey] = useState(0);
  const [busy, setBusy] = useState(false);
  const [zipBusy, setZipBusy] = useState(false);
  const [folderBusy, setFolderBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<BatchNotice | null>(null);
  const batch = batches.find((entry) => entry.batchId === selectedBatchId) ?? null;

  useEffect(() => { setOutputDirectory(preferences?.value.batchOutputDirectory ?? null); }, [preferences]);
  useEffect(() => { setPreviewItemId(null); }, [batch?.batchId, filter, offset, visible]);

  useEffect(() => {
    // Admission returns before the asynchronous workspace snapshot may arrive.
    // Keep the new selection until that snapshot contains the accepted batch.
    if (pendingBatchRef.current === selectedBatchId && selectedBatchId != null) {
      if (!batches.some((entry) => entry.batchId === selectedBatchId)) return;
      pendingBatchRef.current = null;
    }
    if (!batches.length) { setSelectedBatchId(null); return; }
    if (selectedBatchId == null || !batches.some((entry) => entry.batchId === selectedBatchId)) {
      setSelectedBatchId(batches[batches.length - 1].batchId);
      setOffset(0);
    }
  }, [batches, selectedBatchId]);
  useEffect(() => {
    if (!batch || !visible) return;
    let active = true;
    setPageError(null);
    const timer = window.setTimeout(() => {
      void getBatchItems(batch.batchId, offset, PAGE_SIZE, filter).then(
        (value) => { if (active) { if (offset > 0 && offset >= value.total) { setOffset(Math.max(0, Math.ceil(value.total / PAGE_SIZE) - 1) * PAGE_SIZE); return; } setLoaded({ batchId: batch.batchId, offset, filter, value }); setPageError(null); } },
        (reason) => { if (active) { setLoaded(null); setPageError({ batchId: batch.batchId, offset, filter, message: String(reason) }); } },
      );
    }, 80);
    return () => { active = false; window.clearTimeout(timer); };
  }, [batch?.batchId, offset, filter, revision, retryKey, visible]);

  async function act(action: () => Promise<unknown>) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    setNotice(null);
    try { await action(); } catch (reason) { setError(String(reason)); } finally { busyRef.current = false; setBusy(false); }
  }
  function selectCreatedBatch(batchId: number) {
    pendingBatchRef.current = batchId;
    setSelectedBatchId(batchId);
    setOffset(0);
  }
  async function rememberDirectory(path: string | null) {
    // Read the latest shared settings so another window's language or recovery
    // change cannot be overwritten by an old batch-page snapshot.
    const current = await getPreferencesSnapshot();
    if (!current) throw new Error(e.unavailable);
    await updatePreferences(current.revision, { ...current.value, batchOutputDirectory: path });
    setOutputDirectory(path);
  }
  async function selectDirectory() {
    const path = await chooseOutputDirectory(outputDirectory ?? undefined);
    if (path) await rememberDirectory(path);
    return path;
  }
  async function startBatch() {
    await act(async () => {
      const paths = await chooseImages();
      if (!paths.length) return;
      const outputDir = outputDirectory ?? await selectDirectory();
      if (!outputDir) return;
      selectCreatedBatch(await createBatch(outputDir, paths));
    });
  }
  async function addImages() {
    if (!batch) return;
    await act(async () => {
      const paths = await chooseImages();
      if (paths.length) await appendBatch(batch.batchId, paths);
    });
  }
  async function importFolder(append = false) {
    await act(async () => {
      const folder = await chooseImageDirectory();
      if (!folder) return;
      setFolderBusy(true);
      try {
        let imported;
        if (append && batch) {
          imported = await appendBatchFolder(batch.batchId, folder);
        } else {
          const outputDir = outputDirectory ?? await selectDirectory();
          if (!outputDir) return;
          imported = await createBatchFromFolder(outputDir, folder);
          selectCreatedBatch(imported.batchId);
        }
        setNotice({ kind: 'folder', added: imported.added, skipped: imported.skipped });
      } finally { setFolderBusy(false); }
    });
  }
  async function startZipBatch() {
    await act(async () => {
      const archive = await chooseZipArchive();
      if (!archive) return;
      const outputDir = outputDirectory ?? await selectDirectory();
      if (!outputDir) return;
      selectCreatedBatch(await createBatchFromZip(outputDir, archive));
    });
  }
  async function addZip() {
    if (!batch) return;
    await act(async () => {
      const archive = await chooseZipArchive();
      if (archive) await appendBatchZip(batch.batchId, archive);
    });
  }
  async function saveZip() {
    if (!batch || busy || zipBusy) return;
    const batchId = batch.batchId;
    setZipBusy(true);
    setError(null);
    setNotice(null);
    try {
      const destination = await chooseZipDestination(`clearframe-batch-${batch.batchId}.zip`);
      if (!destination) return;
      const count = await exportBatchZip(batchId, destination);
      setNotice({ kind: 'zip', destination, count });
    } catch (reason) { setError(String(reason)); } finally { setZipBusy(false); }
  }
  const page = loaded && loaded.batchId === batch?.batchId && loaded.offset === offset && loaded.filter === filter ? loaded.value : null;
  const visiblePageError = pageError && pageError.batchId === batch?.batchId && pageError.offset === offset && pageError.filter === filter ? pageError.message : null;
  const finished = batch ? batch.succeeded + batch.failed + batch.cancelled : 0;
  const active = !!batch && batch.queued + batch.running > 0;
  const states: Record<TaskState, string> = {
    queued: t.queued, running: t.running, cancel_requested: t.cancelRequested,
    cancelled: t.cancelled, succeeded: t.succeeded, failed: t.failed, stale: t.stale,
  };

  return <section className="batch-page">
    <div className="batch-heading">
      <div><h2>{t.title}</h2><p>{t.intro}</p></div>
      {batch && modelReady ? <div className="batch-heading-actions"><button type="button" className="primary-button" disabled={busy} onClick={() => void startBatch()}>{t.newBatch}</button><button type="button" disabled={busy} onClick={() => void importFolder()}>{t.folderNew}</button><button type="button" disabled={busy} onClick={() => void startZipBatch()}>{t.zipNew}</button></div>
        : nativeAvailable && !modelReady ? <button type="button" onClick={onModels}>{t.model}</button> : null}
    </div>
    <p className="batch-note" role={folderBusy ? 'status' : undefined}>{folderBusy ? t.folderScanning : t.folderHint}</p>
    <div className="batch-output-setting">
      <div><h3>{r.folder}</h3><p className="batch-output-path" title={outputDirectory ?? undefined}>{outputDirectory ?? r.unset}</p><p>{r.hint}</p></div>
      <div className="batch-output-actions"><button type="button" disabled={busy || !nativeAvailable} onClick={() => void act(selectDirectory)}>{outputDirectory ? r.change : r.choose}</button>{outputDirectory && <button type="button" className="text-button" disabled={busy || !nativeAvailable} onClick={() => void act(() => rememberDirectory(null))}>{r.ask}</button>}</div>
    </div>
    {!batch ? <div className="batch-empty"><h3>{t.empty}</h3><p>{nativeAvailable ? t.emptyHint : e.unavailable}</p>{modelReady && <div className="batch-empty-actions"><button type="button" className="primary-button" disabled={busy} onClick={() => void startBatch()}>{t.choose}</button><button type="button" disabled={busy} onClick={() => void importFolder()}>{t.folderNew}</button><button type="button" disabled={busy} onClick={() => void startZipBatch()}>{t.zipNew}</button></div>}</div> : <>
      <div className="batch-selector"><label htmlFor="batch-picker">{t.batch}</label><select id="batch-picker" value={batch.batchId} onChange={(event) => { setSelectedBatchId(Number(event.target.value)); setOffset(0); }}>{batches.map((entry) => <option key={entry.batchId} value={entry.batchId}>{t.batch} {entry.batchId} · {entry.total} {t.images}</option>)}</select></div>
      <div className="batch-summary" aria-live="polite">
        <div className="batch-summary-line"><strong>{t.processed} {finished} / {batch.total}</strong><span>{batch.paused ? e.paused : t.running} {batch.running} · {t.waiting} {batch.queued}</span></div>
        <progress max={Math.max(1, batch.total)} value={finished} aria-label={e.progress} />
        <div className="batch-summary-breakdown"><span>{t.complete} {batch.succeeded}</span><span>{t.failed} {batch.failed}</span><span>{t.cancelled} {batch.cancelled}</span></div>
        <p>{t.output}: <span title={batch.outputDirectory}>{batch.outputDirectory}</span></p>
      </div>
      <div className="batch-actions">
        {batch.failed > 0 && <button type="button" disabled={busy || zipBusy || !modelReady} onClick={() => void act(() => retryFailedBatch(batch.batchId))}>{t.retryFailed}</button>}
        <button type="button" disabled={busy || !modelReady} onClick={() => void addImages()}>{t.add}</button>
        <button type="button" disabled={busy || !modelReady} onClick={() => void importFolder(true)}>{t.folderAdd}</button>
        <button type="button" disabled={busy || !modelReady} onClick={() => void addZip()}>{t.zipAdd}</button>
        {batch.total > 0 && (active ? !batch.paused : batch.succeeded > 0) && <button type="button" disabled={busy || zipBusy} onClick={() => void saveZip()}>{zipBusy ? t.zipSaving : t.zipSave}</button>}
        {active && <button type="button" disabled={busy} onClick={() => void act(() => setBatchPaused(batch.batchId, !batch.paused))}>{batch.paused ? t.resume : t.pause}</button>}
        {active && <button type="button" className="text-button danger" disabled={busy} title={t.cancelHint} onClick={() => void act(() => cancelBatch(batch.batchId))}>{t.cancel}</button>}
        {!active && <button type="button" className="text-button" disabled={busy || zipBusy} onClick={() => void act(() => removeBatch(batch.batchId))}>{t.remove}</button>}
      </div>
      {zipBusy && <p className="batch-note" role="status">{t.zipHint}</p>}
      {batch.paused && active && <p className="batch-note" role="status">{batch.failed > 0 ? t.recovery : t.pauseHint}</p>}
      <details className="batch-rules"><summary>{e.rules}</summary><p>{e.rulesBody}</p></details>
      <div className="batch-filters" role="group" aria-label={e.filter}>{(['all', 'active', 'completed', 'issues'] as const).map((value) => <button type="button" key={value} aria-pressed={filter === value} className={filter === value ? 'active' : ''} onClick={() => { setFilter(value); setOffset(0); }}>{e.filters[value]}</button>)}</div>
      <div className="batch-list" aria-busy={!page && !visiblePageError}>
        {visiblePageError ? <div className="batch-list-message"><p role="alert">{t.loadFailed} {visiblePageError}</p><button type="button" onClick={() => setRetryKey((value) => value + 1)}>{t.retry}</button></div>
          : page ? page.items.length ? <ul>{page.items.map((item) => <li key={item.itemId}>
          <div className="batch-item-names"><strong title={item.displayName}>{item.displayName}</strong><small title={item.outputName}>→ {item.outputName}</small></div>
          <span className={`batch-state batch-state-${item.state}`}>{item.state === 'running' ? (t.stages[item.stage as keyof typeof t.stages] ?? states[item.state]) : states[item.state]}</span>
          <div className="batch-item-actions">{item.state === 'succeeded' && <button id={`batch-view-${item.itemId}`} type="button" aria-expanded={previewItemId === item.itemId} className="text-button" onClick={() => setPreviewItemId(previewItemId === item.itemId ? null : item.itemId)}>{r.view}</button>}<button type="button" className="text-button" disabled={busy} onClick={() => void act(async () => { await reviewBatchItem(item.itemId); onReview(); })}>{e.review}</button>
          {['failed', 'cancelled'].includes(item.state) && <button type="button" className="text-button" disabled={busy || zipBusy} onClick={() => void act(() => retryBatchItem(item.itemId))}>{t.retry}</button>}</div>
          {item.error && <OperationError className="batch-item-error" message={item.error} language={language} />}
          {previewItemId === item.itemId && item.state === 'succeeded' && visible && <BatchResultPreview key={item.itemId} itemId={item.itemId} outputName={item.outputName} language={language} onClose={() => { setPreviewItemId(null); document.getElementById(`batch-view-${item.itemId}`)?.focus(); }} />}
        </li>)}</ul> : <p className="batch-list-message">{e.noMatches}</p> : <p className="batch-list-message">{t.loading}</p>}
      </div>
      {page && page.total > PAGE_SIZE && <div className="batch-pagination"><button type="button" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}>{t.previous}</button><span>{offset + 1}–{Math.min(offset + PAGE_SIZE, page.total)} / {page.total}</span><button type="button" disabled={offset + PAGE_SIZE >= page.total} onClick={() => setOffset(offset + PAGE_SIZE)}>{t.next}</button></div>}
    </>}
    {error && <OperationError className="batch-error" message={error} language={language} />}
    {notice && <p className="batch-note" role="status">{notice.kind === 'folder'
      ? translate('batch', language, 'copy.folderAdded', { count: notice.added }) + (notice.skipped ? ' ' + translate('batch', language, 'copy.folderSkipped', { count: notice.skipped }) : '')
      : `${t.zipSaved}: ${notice.destination} (${notice.count} ${t.images})`}</p>}
  </section>;
}
