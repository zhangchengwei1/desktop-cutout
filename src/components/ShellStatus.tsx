import { useEffect, useState } from 'react';
import { useAsyncAction } from '../shared/useAsyncAction';
import { resolveSystemLanguage, translate, useMessages } from '../i18n';
import { cancelShellOperation, getShellSnapshot, openShellResult, openShellWorkbench, retryShellOperation } from '../platform/shellTasks';
import type { ShellSnapshot } from '../platform/shellTasks';

export function ShellStatus() {
  const [snapshot, setSnapshot] = useState<ShellSnapshot | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { busy, run } = useAsyncAction(reason => setError(String(reason)), () => setError(null));
  const language = snapshot?.language ?? resolveSystemLanguage();
  const t = useMessages('shell', language);
  useEffect(() => { document.documentElement.lang = language; }, [language]);
  useEffect(() => {
    let active = true;
    let refreshing = false;
    const refresh = async () => {
      if (refreshing || document.hidden) return;
      refreshing = true;
      try {
        const value = await getShellSnapshot();
        if (active && value) setSnapshot(value);
      } catch (reason) { if (active) setError(String(reason)); }
      finally { refreshing = false; }
    };
    void refresh();
    const timer = window.setInterval(() => { void refresh(); }, 500);
    return () => { active = false; window.clearInterval(timer); };
  }, []);
  const operation = snapshot?.operations.find((item) => item.id === selected) ?? snapshot?.operations[0];
  const running = operation && ['queued', 'preparing', 'processing', 'cancelling'].includes(operation.phase);
  async function action(work: () => Promise<void>) {
    await run(async () => { await work(); setSnapshot(await getShellSnapshot()); });
  }
  return <main className="shell-status" aria-labelledby="shell-title">
    <header><h1 id="shell-title">{operation ? t.phases[operation.phase] : t.title}</h1>
      <p>{running ? t.background : t.saved}</p></header>
    {snapshot && snapshot.operations.length > 1 && <label className="shell-history">{t.operation}
      <select value={operation?.id ?? ''} onChange={(event) => setSelected(Number(event.target.value))}>
        {snapshot.operations.map((item) => <option key={item.id} value={item.id}>#{item.id} · {t.phases[item.phase]}</option>)}
      </select></label>}
    <div role="status" aria-live="polite" aria-atomic="true">
      {operation && operation.total > 0 && <><progress max={operation.total} value={operation.succeeded + operation.failed + operation.cancelled} aria-label={t.progress} />
        <p className="shell-counts">{translate('shell', language, 'counts', { count: operation.total, succeeded: operation.succeeded, failed: operation.failed, cancelled: operation.cancelled })}</p></>}
      {operation?.phase === 'preparing' && <p>{t.preparing}</p>}
      {operation?.phase === 'failed' && <p className="shell-failure">{operation.error?.startsWith('[source_changed]') ? t.sourceChanged : t.failure}</p>}
      {!operation && <p>{snapshot ? t.empty : t.loading}</p>}
    </div>
    {!!operation?.outputDirectories.length && <div className="shell-results">{operation.outputDirectories.map((directory, index) =>
      <button key={directory} type="button" disabled={busy} title={directory} onClick={() => void action(() => openShellResult(operation.id, index))}>
        <span>{t.openFolder}</span><small>{directory}</small>
      </button>)}</div>}
    {(error || operation?.error) && <details className="shell-details"><summary>{t.details}</summary><p>{error ?? operation?.error}</p></details>}
    <footer>
      {operation?.phase === 'failed' && <button type="button" className="primary-button" disabled={busy} onClick={() => void action(() => retryShellOperation(operation.id))}>{t.retry}</button>}
      {(running || operation?.phase === 'failed') && operation?.phase !== 'cancelling' && operation && <button type="button" disabled={busy} onClick={() => void action(() => cancelShellOperation(operation.id))}>{t.cancel}</button>}
      <button type="button" className="text-button" disabled={busy} onClick={() => void action(openShellWorkbench)}>{t.workbench}</button>
    </footer>
    <p className="shell-hint">{t.closeHint}</p>
  </main>;
}
