import { useMessages } from '../i18n';
import type { Language } from '../platform/preferences';
import { useState } from 'react';
import { useAsyncAction } from '../shared/useAsyncAction';
import { cancelModelDownload, clearModel, downloadModel, openModelLicense } from '../platform/models';
import type { ModelSnapshot, ModelStoreSnapshot } from '../platform/models';

type Feature = 'automatic' | 'selection' | 'matting';

const featureModels: Record<Feature, string[]> = {
  automatic: ['isnet-auto-high'],
  selection: ['mobilesam-image-encoder', 'mobilesam-mask-decoder'],
  matting: ['vitmatte-high'],
};

function formatSize(bytes: number): string {
  return (bytes / (1024 * 1024)).toFixed(1) + ' MiB';
}

function ModelRow({ model, language }: { model: ModelSnapshot; language: Language }) {
  const { copy: t, detailsCopy: d } = useMessages('models', language);
  const [error, setError] = useState<string | null>(null);
  const { busy, run: perform } = useAsyncAction(reason => setError(String(reason)), () => setError(null));
  const [confirmClear, setConfirmClear] = useState(false);
  const canDownload = ['missing', 'cancelled', 'failed'].includes(model.state);
  const canCancel = ['queued', 'downloading'].includes(model.state) || (model.state === 'verifying' && model.receivedBytes > 0);
  const canClear = model.state === 'file_verified';
  const showProgress = ['downloading', 'cancel_requested'].includes(model.state) || (model.state === 'verifying' && model.receivedBytes > 0);
  async function run(action: () => Promise<void>) {
    await perform(async () => { try { await action(); } finally { setConfirmClear(false); } });
  }
  return <li className="model-row">
    <div className="model-main"><div><strong>{t.names[model.spec.id as keyof typeof t.names] ?? model.spec.id}</strong><span className="model-meta">{formatSize(model.spec.bytes)} · {model.spec.license}</span></div><span className={`model-state model-state-${model.state}`}>{t.states[model.state]}</span></div>
    {showProgress && <div className="model-progress"><progress value={model.receivedBytes} max={model.spec.bytes} aria-label={(t.names[model.spec.id as keyof typeof t.names] ?? model.spec.id) + ' ' + t.progress} /><span>{formatSize(model.receivedBytes)} / {formatSize(model.spec.bytes)}</span></div>}
    {(canDownload || canCancel || canClear) && <div className="model-actions">
      {canDownload && <button type="button" disabled={busy} onClick={() => void run(() => downloadModel(model.spec.id))}>{t.download}</button>}
      {canCancel && <button type="button" disabled={busy} onClick={() => void run(() => cancelModelDownload(model.spec.id))}>{t.cancel}</button>}
      {canClear && (confirmClear ? <><button type="button" disabled={busy} onClick={() => void run(() => clearModel(model.spec.id))}>{d.clear}</button><button type="button" className="text-button" disabled={busy} onClick={() => setConfirmClear(false)}>{d.cancel}</button></> : <button type="button" disabled={busy} onClick={() => setConfirmClear(true)}>{t.clear}</button>)}
    </div>}
    <details className="model-details"><summary>{d.details}</summary><dl><div><dt>{d.revision}</dt><dd>{model.spec.revision}</dd></div><div><dt>{d.integrity}</dt><dd>{model.spec.sha256}</dd></div><div><dt>{d.environment}</dt><dd>{model.spec.verifiedBackend}</dd></div></dl><button type="button" disabled={busy} onClick={() => void run(() => openModelLicense(model.spec.id))}>{d.open}</button></details>
    {model.error && <p className="model-error" role="alert">{model.error}</p>}
    {error && <p className="model-error" role="alert">{error}</p>}
  </li>;
}

export function ModelResources({ snapshot, language = 'zh' }: { snapshot: ModelStoreSnapshot; language?: Language }) {
  const { copy: t, detailsCopy: d } = useMessages('models', language);
  const { automatic, selection, matting } = snapshot.features;
  const ready = [automatic, selection, matting].filter((feature) => feature.filesVerified).length;
  return <section className="model-resources" aria-labelledby="models-heading">
    <div className="models-heading"><div><h2 id="models-heading">{t.title}</h2><p>{t.hint}</p></div><span>{ready} / 3 {t.count}</span></div>
    {(['automatic', 'selection', 'matting'] as const).map((feature) => {
      const featureSnapshot = snapshot.features[feature];
      const models = snapshot.models.filter((model) => featureModels[feature].includes(model.spec.id));
      return <section className="model-group" key={feature} aria-label={t.groups[feature]}>
        <div className="model-group-heading"><h3>{t.groups[feature]}</h3><span className={featureSnapshot.filesVerified ? 'feature-state ready' : 'feature-state'}>{featureSnapshot.sessionReady ? d.session : featureSnapshot.filesVerified ? t.featureReady : t.featureMissing}</span></div>
        <ul className="model-list">{models.map((model) => <ModelRow key={model.spec.id} model={model} language={language} />)}</ul>
      </section>;
    })}
    <div className="model-cache-summary"><span>{d.cache}: {formatSize(snapshot.models.filter((model) => ['file_verified', 'session_ready'].includes(model.state)).reduce((sum, model) => sum + model.spec.bytes, 0))}</span><p>{d.quality}</p></div>
  </section>;
}
