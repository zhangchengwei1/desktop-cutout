import { resolveSystemLanguage, useMessages } from '../i18n';
import { listenApplicationActions } from '../platform/applicationMenu';
import type { ShortcutAction } from '../shared/shortcuts';
import { OperationError } from '../components/OperationError';
import { lazy, startTransition, Suspense, useEffect, useRef, useState } from 'react';
import { PageBoundary, PageLoading } from '../components/PageBoundary';
import { useSubscription } from '../shared/useSubscription';
import { useAsyncAction } from '../shared/useAsyncAction';
import { PreviewCanvas } from '../features/workspace/PreviewCanvas';
import type { WorkspaceTool } from '../features/workspace/PreviewCanvas';
import { PreviewApplicationMenu } from '../components/PreviewApplicationMenu';
import { openUtilityWindow, setMenuLanguage } from '../platform/utilityWindows';
import { RuntimeStatus } from '../components/RuntimeStatus';
import type { RuntimeState } from '../components/RuntimeStatus';
import { downloadModel, subscribeModels } from '../platform/models';
import type { ModelStoreSnapshot } from '../platform/models';
import { chooseBackgroundImage, chooseImages, choosePngDestination, listenImageDrops } from '../platform/nativeFiles';
import { subscribePreferences } from '../platform/preferences';
import type { Language, PreferencesSnapshot } from '../platform/preferences';
import { getRuntimeInfo, isDesktop } from '../platform/runtime';
import { listenShellWorkbench } from '../platform/shellTasks';
import { autoCutout, cancelTask, exportPng, getPreviewUrl, importAsset, paintMask, pasteAsset, refineMask, removeAsset, retryPreview, selectAsset, selectPoints, setBackground, stepHistory, subscribeWorkspace, switchWorkflow } from '../platform/workspace';
import type { AssetSnapshot, BackgroundChoice, SelectionPoint, WorkspaceSnapshot } from '../platform/workspace';

const ModelSetup = lazy(() => import('../components/ModelSetup').then(module => ({ default: module.ModelSetup })));
const BatchPanel = lazy(() => import('../components/BatchPanel').then(module => ({ default: module.BatchPanel })));

function isTextInput(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName));
}
const systemLanguage: Language = resolveSystemLanguage();

export function App() {
  const [lang, setLang] = useState<Language>(systemLanguage);
  useEffect(() => { document.documentElement.lang = lang; }, [lang]);
  const messages = useMessages('app', lang);
  const t = messages.labels;
  const f = messages.feedback;
  const [runtime, setRuntime] = useState<RuntimeState>({ status: 'loading' });
  const [attempt, setAttempt] = useState(0);
  const [workspaceError, setWorkspaceError] = useState<string | null>(null);
  const [workspaceRetry, setWorkspaceRetry] = useState(0);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [previewRetry, setPreviewRetry] = useState(0);
  const [returnToBatch, setReturnToBatch] = useState(false);
  const [workspace, setWorkspace] = useState<WorkspaceSnapshot | null>(null);
  const [models, setModels] = useState<ModelStoreSnapshot | null>(null);
  const [modelSetupError, setModelSetupError] = useState<string | null>(null);
  const [modelSetupDismissed, setModelSetupDismissed] = useState(false);
  const [modelRetry, setModelRetry] = useState(0);
  const [modelSubscriptionRetry, setModelSubscriptionRetry] = useState(0);
  const autoDownloadAttempted = useRef(new Set<string>());
  const autoDownloadInFlight = useRef<string | null>(null);
  const [preferences, setPreferences] = useState<PreferencesSnapshot | null>(null);
  const [page, setPage] = useState<'work' | 'batch'>(new URLSearchParams(location.search).has('batch') ? 'batch' : 'work');
  useEffect(() => {
    let active = true;
    let stop: (() => void) | undefined;
    void listenShellWorkbench(() => { if (active) setPage('batch'); }).then((unlisten) => { if (active) stop = unlisten; else unlisten(); });
    return () => { active = false; stop?.(); };
  }, []);
  const [batchVisited, setBatchVisited] = useState(false);
  useEffect(() => { if (page === 'batch') setBatchVisited(true); }, [page]);
  const [assetRailOpen, setAssetRailOpen] = useState(false);
  const pageRef = useRef(page);
  pageRef.current = page;
  const [original, setOriginal] = useState(false);
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const imageContextRef = useRef('');
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => { void setMenuLanguage(lang).catch(error => setNotice(String(error))); }, [lang]);
  const { busy, busyRef, run } = useAsyncAction(error => setNotice(String(error)), () => setNotice(null));
  const [tool, setTool] = useState<WorkspaceTool>('auto');
  const [pointLabel, setPointLabel] = useState<0 | 1>(1);
  const [newSelection, setNewSelection] = useState(false);
  const [brushValue, setBrushValue] = useState<0 | 255>(255);
  const [brushRadius, setBrushRadius] = useState(16);
  const [refineEdge, setRefineEdge] = useState(12);
  const [solidColor, setSolidColor] = useState('#ffffff');
  useEffect(() => {
    let active = true;
    void getRuntimeInfo().then((info) => { if (active) setRuntime(info ? { status: 'connected', info } : { status: 'browser' }); }, () => { if (active) setRuntime({ status: 'error' }); });
    return () => { active = false; };
  }, [attempt]);
  useSubscription(subscribeWorkspace, value => { setWorkspace(value); setWorkspaceError(null); }, error => setWorkspaceError(String(error)), workspaceRetry);
  useSubscription(subscribePreferences, value => { setPreferences(value); startTransition(() => setLang(value.value.language ?? systemLanguage)); }, error => setNotice(String(error)), workspaceRetry);
  useSubscription(subscribeModels, value => { setModels(value); setModelSetupError(null); }, error => setModelSetupError(String(error)), modelSubscriptionRetry);
  useEffect(() => {
    if (!models) return;
    const inFlight = autoDownloadInFlight.current;
    if (inFlight) {
      const state = models.models.find((model) => model.spec.id === inFlight)?.state;
      if (!state || !['file_verified', 'session_ready', 'failed', 'cancelled'].includes(state)) return;
      autoDownloadInFlight.current = null;
    }
    const next = models.models.find((model) =>
      !autoDownloadAttempted.current.has(model.spec.id)
      && ['missing', 'failed', 'cancelled'].includes(model.state));
    if (!next) return;
    autoDownloadAttempted.current.add(next.spec.id);
    autoDownloadInFlight.current = next.spec.id;
    void downloadModel(next.spec.id).catch((error) => {
      autoDownloadInFlight.current = null;
      setModelSetupError(String(error));
    });
  }, [models, modelRetry]);
  useEffect(() => {
    if (runtime.status !== 'connected') return;
    let active = true;
    let stop: (() => void) | undefined;
    void listenImageDrops((paths) => { if (pageRef.current === 'work') void importPaths(paths); }).then((unlisten) => { if (active) stop = unlisten; else unlisten(); }, (error) => { if (active) setNotice(String(error)); });
    return () => { active = false; stop?.(); };
  }, [runtime.status]);
  useEffect(() => {
    if (runtime.status !== 'connected' || page !== 'work') return;
    const onPaste = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() !== 'v' || !(event.metaKey || event.ctrlKey)) return;
      const target = event.target;
      if (isTextInput(target) || busyRef.current) return;
      event.preventDefault();
      void run(async () => { await pasteAsset(); setOriginal(false); setReturnToBatch(false); });
    };
    window.addEventListener('keydown', onPaste);
    return () => window.removeEventListener('keydown', onPaste);
  }, [runtime.status, page]);

  const selected = workspace?.assets.find((asset) => asset.assetId === workspace.selectedAssetId);
  const task = selected && [...(workspace?.tasks ?? [])].reverse().find((item) => item.assetId === selected.assetId && ['queued', 'running', 'cancel_requested'].includes(item.state));
  const modelReady = models?.features.automatic.filesVerified ?? false;
  const selectionReady = models?.features.selection.filesVerified ?? false;
  const mattingReady = models?.features.matting.filesVerified ?? false;
  const modelsReady = models?.models.every((model) => ['file_verified', 'session_ready'].includes(model.state)) ?? false;
  const showModelSetup = runtime.status === 'connected' && page === 'work' && !modelSetupDismissed && !modelsReady;
  const result = !!selected?.resultAvailable && !original;
  const menuAction = useRef<(action: ShortcutAction) => void>(() => {});
  menuAction.current = action => {
    if (action === 'workbench') { setPage('work'); return; }
    if (action === 'batch') { setPage('batch'); return; }
    if (runtime.status !== 'connected' || busyRef.current || showModelSetup) return;
    if (action === 'import') { setPage('work'); void choose(); return; }
    if (page !== 'work' || !selected || task) return;
    if (action === 'export' && selected.resultAvailable) void save(selected);
    if (action === 'toggleOriginal' && selected.resultAvailable) setOriginal(value => !value);
    if (action === 'automatic' && modelReady && selected.state === 'ready') void run(async () => { await autoCutout(selected.assetId); setTool('auto'); setOriginal(false); });
  };
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void listenApplicationActions(action => menuAction.current(action)).then(unlisten => { if (disposed) unlisten(); else stop = unlisten; }, error => setNotice(String(error)));
    return () => { disposed = true; stop?.(); };
  }, []);

  useEffect(() => { setTool(selected?.workflow === 'selection' ? 'point' : 'auto'); setNewSelection(false); setOriginal(false); }, [selected?.assetId]);
  useEffect(() => { if (selected?.background.kind === 'solid') setSolidColor('#' + selected.background.rgb.map((value) => value.toString(16).padStart(2, '0')).join('')); }, [selected?.assetId, selected?.backgroundRevision]);
  useEffect(() => {
    if (runtime.status !== 'connected' || page !== 'work') return;
    const onHistory = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || isTextInput(event.target)) return;
      const key = event.key.toLowerCase();
      if (key !== 'z' && !(event.ctrlKey && key === 'y')) return;
      const redo = event.shiftKey || key === 'y';
      if (!selected || busy || task || (redo ? !selected.canRedo : !selected.canUndo)) return;
      event.preventDefault();
      void run(async () => { if (!await stepHistory(selected.assetId, selected.maskRevision, redo)) throw new Error(f.changed); });
    };
    window.addEventListener('keydown', onHistory);
    return () => window.removeEventListener('keydown', onHistory);
  }, [runtime.status, page, selected?.assetId, selected?.maskRevision, selected?.canUndo, selected?.canRedo, selected?.resultAvailable, busy, task?.taskId, lang]);
  useEffect(() => {
    let active = true;
    const context = `${selected?.assetId ?? 'none'}:${result}`;
    if (imageContextRef.current !== context) {
      imageContextRef.current = context;
      setImageUrl(null);
    }
    setPreviewError(null);
    if (!selected?.previewAvailable) { setImageUrl(null); return () => { active = false; }; }
    void getPreviewUrl(selected.assetId, result).then((url) => { if (active) setImageUrl(url); }, (error) => { if (active) setPreviewError(String(error)); });
    return () => { active = false; };
  }, [selected?.assetId, selected?.previewRevision, selected?.resultRevision, selected?.previewAvailable, result, previewRetry]);

  async function importMany(paths: string[]) {
    const failures: string[] = [];
    for (const path of paths) {
      try { await importAsset(path); setPage('work'); setOriginal(false); setReturnToBatch(false); }
      catch (error) { failures.push(String(error)); }
    }
    if (failures.length) throw new Error(failures.join('\n'));
  }
  async function importPaths(paths: string[]) {
    await run(() => importMany(paths));
  }
  async function choose() {
    if (runtime.status !== 'connected') return;
    await run(async () => { await importMany(await chooseImages()); });
  }
  async function save(asset: AssetSnapshot) {
    await run(async () => {
      const suffix = preferences?.value.exportNaming === 'clearframe' ? '-clearframe.png' : '-cutout.png';
      const path = await choosePngDestination(asset.displayName.replace(/\.[^.]+$/, '') + suffix);
      if (path) await exportPng(asset.assetId, path);
    });
  }
  async function removeCurrent() {
    if (!selected) return;
    const remaining = workspace?.assets.filter((asset) => asset.assetId !== selected.assetId) ?? [];
    await run(async () => {
      await removeAsset(selected.assetId);
      if (remaining.length) await selectAsset(remaining[remaining.length - 1].assetId);
      setReturnToBatch(false);
    });
  }
  const lastExport = selected && [...(workspace?.tasks ?? [])].reverse().find((item) => item.assetId === selected.assetId && item.kind === 'export');
  const lastTask = selected && [...(workspace?.tasks ?? [])].reverse().find((item) => item.assetId === selected.assetId && item.kind !== 'preview');
  const lastError = lastTask?.state === 'failed' ? lastTask.error : null;
  function addPoint(position: { x: number; y: number }) {
    if (!selected || !selectionReady || task || busyRef.current) return;
    const previous = newSelection ? [] : selected.selectionPoints;
    if (previous.length >= 64) { setNotice(f.pointLimit); return; }
    const point: SelectionPoint = { ...position, label: previous.length ? pointLabel : 1 };
    void run(async () => { await selectPoints(selected.assetId, [...previous, point]); setNewSelection(false); });
  }
  function recoverPreview() {
    if (!selected) return;
    setPreviewError(null);
    if (selected.previewAvailable) setPreviewRetry((value) => value + 1);
    else void run(() => retryPreview(selected.assetId));
  }
  function applyBackground(choice: BackgroundChoice) {
    if (!selected) return;
    void run(async () => {
      if (!await setBackground(selected.assetId, selected.maskRevision, selected.backgroundRevision, choice)) throw new Error(f.changed);
      setOriginal(false);
    });
  }
  function chooseBackground() {
    if (!selected) return;
    void run(async () => {
      const path = await chooseBackgroundImage();
      if (path && !await setBackground(selected.assetId, selected.maskRevision, selected.backgroundRevision, { kind: 'image', path })) throw new Error(f.changed);
      if (path) setOriginal(false);
    });
  }

  return <main className="desktop-shell" lang={lang}>
    <header className="app-header">
      {!isDesktop() && <PreviewApplicationMenu language={lang} onError={error => setNotice(String(error))} />}
      <div className="app-identity"><svg className="brand-mark" aria-hidden="true" viewBox="0 0 16 16" fill="none"><rect x="1.5" y="1.5" width="13" height="13" stroke="currentColor" /><path d="M5 5h6v6H5z" fill="currentColor" /></svg><span className="brand">ClearFrame Desktop</span></div>
      <nav className="primary-nav" aria-label={f.navigation}>
        <button type="button" className={page === 'work' ? 'text-button active' : 'text-button'} aria-current={page === 'work' ? 'page' : undefined} onClick={() => setPage('work')}>{t.work}</button>
        <button type="button" className={page === 'batch' ? 'text-button active' : 'text-button'} aria-current={page === 'batch' ? 'page' : undefined} onClick={() => setPage('batch')}>{messages.batchLabels}</button>
      </nav>
    </header>
    {runtime.status !== 'connected' && <div className="runtime-banner"><RuntimeStatus state={runtime} language={lang} onRetry={() => { setRuntime({ status: 'loading' }); setAttempt((value) => value + 1); setWorkspaceRetry((value) => value + 1); setModelSubscriptionRetry((value) => value + 1); }} /></div>}
    {workspaceError && <div className="page-feedback" role="alert"><span>{f.loadFailed}: {workspaceError}</span><button type="button" onClick={() => setWorkspaceRetry((value) => value + 1)}>{f.retry}</button></div>}
    <div className="batch-surface" hidden={page !== 'batch'}>{(page === 'batch' || batchVisited) && <PageBoundary language={lang} title={messages.batchLabels}><BatchPanel language={lang} preferences={preferences} batches={workspace?.batches ?? []} revision={workspace?.revision ?? 0} modelReady={modelReady} visible={page === 'batch'} nativeAvailable={runtime.status === 'connected'} onModels={() => void openUtilityWindow('resources').catch(error => setNotice(String(error)))} onReview={() => { setReturnToBatch(true); setOriginal(true); setModelSetupDismissed(true); setPage('work'); }} /></PageBoundary>}</div>
    {page === 'work' && (runtime.status === 'connected' && !workspace && !workspaceError ? <PageLoading language={lang} title={t.work} /> : !selected ? <section className="empty-workspace">
      <div className="empty-checker"><div className="empty-card"><span className="empty-icon" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="M4 8V5a1 1 0 0 1 1-1h3m8 0h3a1 1 0 0 1 1 1v3m0 8v3a1 1 0 0 1-1 1h-3m-8 0H5a1 1 0 0 1-1-1v-3" /><path d="m7 15 3-3 2 2 3-4 3 5H7Z" /></svg></span><h2>{t.empty}</h2><button type="button" className="primary-button" disabled={busy || runtime.status !== 'connected'} onClick={() => void choose()}>{t.add}</button><p>{t.hint}</p></div></div>
      {!!workspace?.assets.length && <div className="existing-assets" aria-label={t.images}>{workspace.assets.map((asset) => <button type="button" key={asset.assetId} disabled={busy} onClick={() => void run(() => selectAsset(asset.assetId))}>{asset.displayName}</button>)}</div>}
    </section> : <div className={assetRailOpen ? 'workbench rail-open' : 'workbench'}>
      <aside className="asset-rail" aria-label={t.images}>
        <div className="rail-heading"><strong>{t.images} ({workspace?.assets.length ?? 0})</strong><button type="button" className="icon-button" title={t.add} aria-label={t.add} disabled={busy} onClick={() => void choose()}><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 2v12M2 8h12" /></svg></button></div>
        <ul>{workspace?.assets.map((asset) => <li key={asset.assetId}><button type="button" className={asset.assetId === selected.assetId ? 'asset-choice selected' : 'asset-choice'} aria-current={asset.assetId === selected.assetId ? 'true' : undefined} disabled={busy} onClick={() => { setAssetRailOpen(false); void run(() => selectAsset(asset.assetId)); }}><span className="asset-name">{asset.displayName}</span><small>{asset.error ? f.previewFailed : asset.width && asset.height ? asset.width + ' × ' + asset.height : t.preview}</small></button></li>)}</ul>
      </aside>
      <section className="canvas-panel" aria-label={f.preview}>
        <div className="canvas-toolbar"><div className="canvas-toolbar-title"><button type="button" className="rail-toggle" aria-label={t.images} aria-expanded={assetRailOpen} onClick={() => setAssetRailOpen((value) => !value)}><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 4h12M2 8h12M2 12h12" /></svg></button><span title={selected.displayName}>{selected.displayName}</span></div>
          <div className="canvas-toolbar-actions"><div className="segmented"><button type="button" className={!result ? 'selected' : ''} aria-pressed={!result} onClick={() => setOriginal(true)}>{t.original}</button><button type="button" className={result ? 'selected' : ''} aria-pressed={result} disabled={!selected.resultAvailable} onClick={() => setOriginal(false)}>{t.result}</button></div><button type="button" className="primary-button export-button" disabled={!selected.resultAvailable || !!task || busy} onClick={() => void save(selected)}>{t.save}</button></div>
        </div>
        <div className="canvas-history"><div className="history-actions"><button type="button" disabled={!selected.canUndo || !!task || busy} onClick={() => void run(async () => { if (!await stepHistory(selected.assetId, selected.maskRevision, false)) throw new Error(f.changed); })}>{t.undo}</button><button type="button" disabled={!selected.canRedo || !!task || busy} onClick={() => void run(async () => { if (!await stepHistory(selected.assetId, selected.maskRevision, true)) throw new Error(f.changed); })}>{t.redo}</button></div>{returnToBatch && <button type="button" className="text-button" onClick={() => setPage('batch')}>{f.backBatch}</button>}</div>
        <PreviewCanvas asset={selected} language={lang} imageUrl={imageUrl} previewError={previewError}
          result={result} tool={tool} disabled={busy || !!task} selectionReady={selectionReady} newSelection={newSelection}
          brushRadius={brushRadius} brushValue={brushValue} onPoint={addPoint} onRecover={recoverPreview}
          onImageError={() => { setImageUrl(null); setPreviewError(f.previewFailed); }}
          onStroke={stroke => { void run(async () => {
            if (!await paintMask(selected.assetId, selected.maskRevision, stroke)) throw new Error(f.changed);
          }); }} />
        <div className="canvas-status" role="status"><span>{selected.width && selected.height ? `${selected.width} × ${selected.height}` : selected.error ? f.previewFailed : t.preview}</span><span>{task ? task.state === 'cancel_requested' ? t.cancel : t.waiting : lastExport?.state === 'succeeded' ? t.saved : selected.workflow === 'selection' ? t.point : t.auto}</span></div>
      </section>
      <aside className="tool-panel" aria-label={f.tools}>
        <div className="tool-switch">
          <button type="button" className={tool === 'auto' ? 'active' : ''} aria-pressed={tool === 'auto'} disabled={busy || !!task} onClick={() => void run(async () => { await switchWorkflow(selected.assetId, 'automatic'); setTool('auto'); setOriginal(false); })}>{t.auto}</button>
          <button type="button" className={tool === 'point' ? 'active' : ''} aria-pressed={tool === 'point'} disabled={busy || !!task} onClick={() => void run(async () => { await switchWorkflow(selected.assetId, 'selection'); setTool('point'); setOriginal(true); })}>{t.point}</button>
          <button type="button" className={tool === 'brush' ? 'active' : ''} aria-pressed={tool === 'brush'} disabled={!selected.resultAvailable || busy || !!task} onClick={() => { setTool('brush'); setOriginal(false); }}>{t.brush}</button>
        </div>
        <div className="secondary-tools"><button type="button" className={tool === 'refine' ? 'active' : ''} aria-pressed={tool === 'refine'} disabled={!selected.resultAvailable || busy || !!task} onClick={() => { setTool('refine'); setOriginal(false); }}>{t.edge}</button><button type="button" className={tool === 'background' ? 'active' : ''} aria-pressed={tool === 'background'} disabled={!selected.resultAvailable || busy || !!task} onClick={() => { setTool('background'); setOriginal(false); }}>{t.backgroundTitle}</button></div>
        <div className="current-tool">
          {tool === 'auto' && (!modelReady ? <button type="button" className="primary-button" onClick={() => void openUtilityWindow('resources').catch(error => setNotice(String(error)))}>{t.missing}</button> : <button type="button" className="primary-button" disabled={!!task || busy || selected.state !== 'ready'} onClick={() => void run(async () => { await autoCutout(selected.assetId); setOriginal(false); })}>{task ? t.waiting : t.auto}</button>)}
          {tool === 'point' && (!selectionReady ? <button type="button" className="primary-button" onClick={() => void openUtilityWindow('resources').catch(error => setNotice(String(error)))}>{t.pointMissing}</button> : <>
            <p>{t.pointHint}</p><div className="segmented brush-mode"><button type="button" className={pointLabel === 1 ? 'selected' : ''} aria-pressed={pointLabel === 1} onClick={() => setPointLabel(1)}>{t.foreground}</button><button type="button" className={pointLabel === 0 ? 'selected' : ''} aria-pressed={pointLabel === 0} disabled={newSelection || selected.selectionPoints.length === 0} onClick={() => setPointLabel(0)}>{t.background}</button></div>
            <p>{f.selection} · {newSelection ? 0 : selected.selectionPoints.length} / 64</p><button type="button" disabled={!!task || busy} onClick={() => { setNewSelection(true); setPointLabel(1); setOriginal(true); }}>{t.newSelection}</button><p id="point-keyboard-help" className="keyboard-help">{f.keyboard}</p>
          </>)}
          {tool === 'brush' && <><div className="segmented brush-mode"><button type="button" className={brushValue === 255 ? 'selected' : ''} aria-pressed={brushValue === 255} onClick={() => setBrushValue(255)}>{t.keep}</button><button type="button" className={brushValue === 0 ? 'selected' : ''} aria-pressed={brushValue === 0} onClick={() => setBrushValue(0)}>{t.erase}</button></div><label className="brush-size">{t.radius} · {brushRadius} px<input type="range" min="1" max="128" value={brushRadius} disabled={busy || !!task} onChange={(event) => setBrushRadius(Number(event.target.value))} /></label></>}
          {tool === 'refine' && (mattingReady ? <><label className="brush-size">{t.edge} · {refineEdge} px<input type="range" min="2" max="64" value={refineEdge} disabled={!!task || busy} onChange={(event) => setRefineEdge(Number(event.target.value))} /></label><button type="button" disabled={!!task || busy} onClick={() => void run(async () => { await refineMask(selected.assetId, selected.maskRevision, refineEdge); setOriginal(false); })}>{t.refine}</button></> : <button type="button" onClick={() => void openUtilityWindow('resources').catch(error => setNotice(String(error)))}>{t.refineMissing}</button>)}
          {tool === 'background' && <><div className="background-mode"><button type="button" aria-pressed={selected.background.kind === 'transparent'} disabled={!!task || busy} onClick={() => applyBackground({ kind: 'transparent' })}>{t.transparent}</button><button type="button" aria-pressed={selected.background.kind === 'solid'} disabled={!!task || busy} onClick={() => applyBackground({ kind: 'solid', rgb: [1, 3, 5].map((offset) => Number.parseInt(solidColor.slice(offset, offset + 2), 16)) as [number, number, number] })}>{t.solid}</button><button type="button" aria-pressed={selected.background.kind === 'image'} disabled={!!task || busy} onClick={chooseBackground}>{t.picture}</button></div><label className="background-color">{t.backgroundColor}<input type="color" value={solidColor} disabled={!!task || busy} onChange={(event) => setSolidColor(event.target.value)} /></label>{selected.background.kind === 'image' && <p className="background-filename">{selected.background.displayName}</p>}</>}
        </div>
        {task && <button type="button" className="text-button" disabled={busy || task.state === 'cancel_requested'} onClick={() => void run(() => cancelTask(task.taskId))}>{t.cancel}</button>}
        <button type="button" className="text-button danger" disabled={busy} onClick={() => void removeCurrent()}>{t.remove}</button>
        {lastError && <OperationError className="error-text" message={lastError} language={lang} />}
        {selected.error && <p className="error-text" role="alert">{selected.error}</p>}
      </aside>
    </div>)}
    {notice && <div className="notice" role="alert">{notice}<button type="button" aria-label={f.dismiss} onClick={() => setNotice(null)}>×</button></div>}
    {showModelSetup && <Suspense fallback={null}><ModelSetup snapshot={models} language={lang} error={modelSetupError} onRetry={() => {
      for (const model of models?.models ?? []) if (['failed', 'cancelled', 'missing'].includes(model.state)) autoDownloadAttempted.current.delete(model.spec.id);
      setModelSetupError(null); setModelRetry((value) => value + 1);
      if (!models) setModelSubscriptionRetry((value) => value + 1);
    }} onResources={() => { setModelSetupDismissed(true); void openUtilityWindow('resources').catch(error => setNotice(String(error))); }} /></Suspense>}
  </main>;
}
