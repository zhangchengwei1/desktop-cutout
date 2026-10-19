import { useMessages } from '../i18n';
import type { Language } from '../platform/preferences';
import { openUtilityWindow } from '../platform/utilityWindows';
import type { UtilityView } from '../platform/utilityWindows';

// Browser previews have no OS menu bar. The shipped desktop uses native menus.
export function PreviewApplicationMenu({ language, onError }: { language: Language; onError: (reason: unknown) => void }) {
  const text = useMessages('menu', language);
  return <details className="preview-application-menu">
    <summary aria-label={text.previewMenu}>ClearFrame</summary>
    <div className="preview-menu-items">
      {(['about', 'settings', 'resources', 'help'] as UtilityView[]).map(view => <button key={view} type="button" onClick={event => {
        event.currentTarget.closest('details')?.removeAttribute('open');
        void openUtilityWindow(view).catch(onError);
      }}>{text[view]}</button>)}
    </div>
  </details>;
}
