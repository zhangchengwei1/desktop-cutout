import type { ShortcutOverrides } from '../shared/shortcuts';
import { lazy, startTransition, useEffect, useState } from 'react';
import { PageBoundary, PageLoading } from './PageBoundary';
import { resolveSystemLanguage } from '../i18n';
import { subscribePreferences } from '../platform/preferences';
import { subscribeModels } from '../platform/models';
import type { ModelStoreSnapshot } from '../platform/models';
import { getRuntimeInfo, isDesktop } from '../platform/runtime';
import type { RuntimeInfo } from '../platform/runtime';
import { openUtilityWindow, setMenuLanguage } from '../platform/utilityWindows';
import type { UtilityView } from '../platform/utilityWindows';
import { useSubscription } from '../shared/useSubscription';
import { useMessages } from '../i18n';

const ModelResources = lazy(() => import('./ModelResources').then(module => ({ default: module.ModelResources })));
const HelpPanel = lazy(() => import('./HelpPanel').then(module => ({ default: module.HelpPanel })));

export function UtilityWindow({ view }: { view: Exclude<UtilityView, 'settings'> }) {
  const [overrides, setOverrides] = useState<ShortcutOverrides>({});
  const [language, setLanguage] = useState(resolveSystemLanguage);
  const [error, setError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const menu = useMessages('menu', language);
  const feedback = useMessages('app', language).feedback;
  useSubscription(subscribePreferences, snapshot => { startTransition(() => setLanguage(snapshot.value.language ?? resolveSystemLanguage())); setOverrides(snapshot.value.shortcuts ?? {}); }, reason => setError(String(reason)), retry);
  useEffect(() => {
    document.documentElement.lang = language;
    document.title = menu[view];
    void setMenuLanguage(language).catch(reason => setError(String(reason)));
  }, [language, menu, view]);
  useEffect(() => {
    if (view === 'resources') return;
    let active = true;
    void getRuntimeInfo().then(value => { if (active) setRuntime(value); }, reason => { if (active) setError(String(reason)); });
    return () => { active = false; };
  }, [view, retry]);
  function openModels() { void openUtilityWindow('resources').catch(reason => setError(String(reason))); }
  return <main className="utility-window">
    <PageBoundary language={language} title={menu[view]} fullWindow>
      {view === 'resources' ? <Resources language={language} retry={retry} onError={reason => setError(String(reason))} />
        : <HelpPanel overrides={overrides} language={language} runtime={runtime} onModels={openModels} initialTab={view === 'about' ? 'about' : 'guide'} />}
    </PageBoundary>
    {error && <div className="page-feedback" role="alert">{error}<button type="button" onClick={() => { setError(null); setRetry(value => value + 1); }}>{feedback.retry}</button></div>}
  </main>;
}

function Resources({ language, retry, onError }: { language: ReturnType<typeof resolveSystemLanguage>; retry: number; onError: (reason: unknown) => void }) {
  const [snapshot, setSnapshot] = useState<ModelStoreSnapshot | null>(null);
  const title = useMessages('menu', language).resources;
  const runtimeText = useMessages('runtime', language).copy;
  useSubscription(subscribeModels, setSnapshot, onError, retry);
  return <div className="resources-page">{snapshot ? <ModelResources snapshot={snapshot} language={language} /> : isDesktop() ? <PageLoading language={language} title={title} /> : <p role="status">{runtimeText.browserHelp}</p>}</div>;
}
