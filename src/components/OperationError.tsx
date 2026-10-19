import { useMessages } from '../i18n';
import type { Language } from '../platform/preferences';

export function OperationError({ message, language, className = '' }: {
  message: string; language: Language; className?: string;
}) {
  const t = useMessages('errors', language).copy;
  const friendly = message.startsWith('[source_changed]') ? t.sourceChanged : message.startsWith('[runtime_unavailable]') || /failed to load from .+dlopen failed/.test(message)
    ? t.runtime : message.startsWith('[output_unavailable]') ? t.output
    : message.startsWith('[folder_unavailable]') ? t.folder
    : message.startsWith('[folder_empty]') ? t.emptyFolder
    : message.startsWith('[folder_limit]') ? t.folderLimit : null;
  return <div className={`operation-error ${className}`}><p role="alert">{friendly ?? message}</p>{friendly && <details><summary>{t.details}</summary><p>{message}</p></details>}</div>;
}
