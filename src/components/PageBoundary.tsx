import { Component, Suspense } from 'react';
import type { ReactNode } from 'react';
import { resolveSystemLanguage, useMessages } from '../i18n';
import type { Language } from '../platform/preferences';

type PageProps = { language?: Language; title?: string; fullWindow?: boolean };

export function PageLoading({ language = resolveSystemLanguage(), title = 'ClearFrame Desktop', fullWindow = false }: PageProps) {
  const t = useMessages('runtime', language).copy;
  return <section className={`page-state${fullWindow ? ' page-state-window' : ''}`} role="status" aria-busy="true">
    <div className="page-state-content"><h2>{title}</h2><p>{t.pageLoading}</p>
      <div className="page-skeleton" aria-hidden="true"><span /><span /><span /></div>
    </div>
  </section>;
}

function PageLoadError({ language = resolveSystemLanguage(), fullWindow = false }: PageProps) {
  const t = useMessages('runtime', language).copy;
  return <section className={`page-state${fullWindow ? ' page-state-window' : ''}`} role="alert">
    <div className="page-state-content"><h2>{t.pageError}</h2><p>{t.pageErrorHelp}</p>
      <button type="button" onClick={() => window.location.reload()}>{t.reloadWindow}</button>
    </div>
  </section>;
}

class PageErrorBoundary extends Component<PageProps & { children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  render() { return this.state.failed ? <PageLoadError {...this.props} /> : this.props.children; }
}

// Keep navigation and the other page's state outside asynchronous page loading.
export function PageBoundary({ children, ...props }: PageProps & { children: ReactNode }) {
  return <PageErrorBoundary {...props}><Suspense fallback={<PageLoading {...props} />}>{children}</Suspense></PageErrorBoundary>;
}
