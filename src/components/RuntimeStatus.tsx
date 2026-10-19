import { useMessages } from '../i18n';
import type { Language } from '../platform/preferences';
import type { RuntimeInfo } from '../platform/runtime';

export type RuntimeState =
  | { status: 'loading' }
  | { status: 'browser' }
  | { status: 'connected'; info: RuntimeInfo }
  | { status: 'error' };

export function RuntimeStatus({ state, onRetry, language = 'zh' }: { state: RuntimeState; onRetry: () => void; language?: Language }) {
  const t = useMessages('runtime', language).copy;
  return <section className="runtime" aria-label={t.title} aria-live="polite" aria-atomic="true">
    {state.status === 'loading' && <p>{t.loading}</p>}
    {state.status === 'browser' && <><strong>{t.browser}</strong><p>{t.browserHelp}</p></>}
    {state.status === 'connected' && <><h2>{t.connected}</h2><dl><div><dt>{t.core}</dt><dd>{state.info.coreVersion}</dd></div><div><dt>{t.inference}</dt><dd>{state.info.inferenceReady ? t.ready : t.missing}</dd></div></dl></>}
    {state.status === 'error' && <><strong>{t.error}</strong><p>{t.errorHelp}</p><button type="button" onClick={onRetry}>{t.retry}</button></>}
  </section>;
}
