import { useMessages } from '../i18n';
import type { Language } from '../platform/preferences';
import * as Dialog from '@radix-ui/react-dialog';
import { useRef } from 'react';
import type { ModelStoreSnapshot } from '../platform/models';


export function ModelSetup({ snapshot, language, error, onRetry, onResources }: {
  snapshot: ModelStoreSnapshot | null;
  language: Language;
  error: string | null;
  onRetry: () => void;
  onResources: () => void;
}) {
  const previousFocus = useRef<Element | null>(null);
  const t = useMessages('setup', language).copy;
  const names = useMessages('models', language).copy.names;
  const models = snapshot?.models ?? [];
  const total = models.reduce((sum, model) => sum + model.spec.bytes, 0);
  const received = models.reduce((sum, model) => sum + (
    model.state === 'file_verified' || model.state === 'session_ready'
      ? model.spec.bytes : Math.min(model.receivedBytes, model.spec.bytes)
  ), 0);
  const percent = total ? Math.floor(received / total * 100) : 0;
  const current = models.find((model) => ['downloading', 'verifying', 'cancel_requested'].includes(model.state) && model.receivedBytes > 0)
    ?? models.find((model) => ['queued', 'downloading'].includes(model.state));
  const failed = models.find((model) => model.state === 'failed' || model.state === 'cancelled');
  const active = !snapshot || models.some((model) => ['queued', 'downloading', 'cancel_requested'].includes(model.state) || (model.state === 'verifying' && model.receivedBytes > 0))
    || models.some((model) => model.state === 'missing' || model.state === 'verifying');
  const message = error ?? (!active ? failed?.error ?? (failed ? t.failed : null) : null);
  const status = current
    ? `${current.state === 'verifying' ? t.verifying : t.downloading} ${names[current.spec.id as keyof typeof names] ?? current.spec.id}`
    : models.some((model) => model.state === 'verifying') || !snapshot ? t.checking : t.ready;

  return <Dialog.Root open onOpenChange={open => { if (!open) onResources(); }}>
    <Dialog.Portal><Dialog.Content className="model-setup" lang={language}
      aria-labelledby="model-setup-title" aria-describedby="model-setup-detail"
      onOpenAutoFocus={() => { previousFocus.current = document.activeElement; }}
      onCloseAutoFocus={event => {
        event.preventDefault();
        const previous = previousFocus.current;
        if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
      }}
      onInteractOutside={event => event.preventDefault()}
      onKeyDown={event => event.stopPropagation()}>
    <div className="model-setup-content">
      <svg className="model-setup-mark" viewBox="0 0 48 48" fill="none" aria-hidden="true"><rect x="5" y="5" width="38" height="38" rx="11" stroke="currentColor" strokeWidth="2" /><path d="M15 15h18v18H15z" fill="currentColor" /></svg>
      <Dialog.Title asChild><h1 id="model-setup-title">{message ? t.failed : t.title}</h1></Dialog.Title>
      {message ? <p className="model-setup-error" role="alert">{message}</p> : <p className="model-setup-status" aria-live="polite">{status}</p>}
      <div className="model-setup-progress"><progress value={received} max={total || 1} aria-label={t.progress} /><span>{percent}%</span></div>
      <Dialog.Description asChild><p id="model-setup-detail" className="model-setup-detail">{t.detail}</p></Dialog.Description>
      <div className="model-setup-actions">{message && <button type="button" className="primary-button" onClick={onRetry}>{t.retry}</button>}<button type="button" onClick={onResources}>{t.resources}</button></div>
    </div>
    </Dialog.Content></Dialog.Portal>
  </Dialog.Root>;
}
