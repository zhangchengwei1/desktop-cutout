import { useMessages } from '../i18n';
import type { Language } from '../platform/preferences';
import { useState } from 'react';
import packageInfo from '../../package.json';
import { displayShortcut, resolveShortcuts, shortcutActions } from '../shared/shortcuts';
import type { ShortcutOverrides } from '../shared/shortcuts';
import type { RuntimeInfo } from '../platform/runtime';

type Tab = 'guide' | 'about';

export function HelpPanel({ language, runtime, onModels, initialTab = 'guide', overrides = {} }: {
  initialTab?: Tab;
  overrides?: ShortcutOverrides;
  language: Language;
  runtime: RuntimeInfo | null;
  onModels: () => void;
}) {
  const [tab, setTab] = useState<Tab>(initialTab);
  const { copy: t, extra: e } = useMessages('help', language);
  const modifier = navigator.userAgent.includes('Mac') ? '⌘' : 'Ctrl+';
  const keys = resolveShortcuts(overrides);
  const shortcutText = useMessages('shortcuts', language);
  const shortcuts = [
    ...shortcutActions.map(action => [shortcutText.actions[action], displayShortcut(keys[action]) || shortcutText.unassigned]),
    [t.paste, `${modifier}V`], [t.undo, `${modifier}Z`], [t.redo, `${modifier}Shift+Z`],
  ];

  return <section className="help-page" aria-labelledby="help-title">
    <div className="help-heading"><h2 id="help-title">{t.title}</h2><p>{t.intro}</p></div>
    <div className="help-layout">
      <nav className="help-nav" aria-label={t.title}>
        <button type="button" className={tab === 'guide' ? 'active' : ''} aria-current={tab === 'guide' ? 'page' : undefined} onClick={() => setTab('guide')}>{t.guide}</button>
        <button type="button" className={tab === 'about' ? 'active' : ''} aria-current={tab === 'about' ? 'page' : undefined} onClick={() => setTab('about')}>{t.about}</button>
      </nav>
      {tab === 'guide' ? <div className="help-content">
        <h3>{t.guide}</h3><p>{e.inputFormats}</p>
        <ol className="guide-steps">{t.steps.map((step) => <li key={step.title}><strong>{step.title}</strong><p>{step.body}</p></li>)}</ol>
        <p className="help-keyboard">{e.keyboardSelection}</p>
        <h3>{t.shortcuts}</h3>
        <dl className="shortcut-list">{shortcuts.map(([action, keys]) => <div key={action}><dt>{action}</dt><dd><kbd>{keys}</kbd></dd></div>)}</dl>
        <section className="help-detail"><h3>{e.troubleshoot}</h3>{e.faults.map((fault) => <details className="help-fault" key={fault.title}><summary>{fault.title}</summary><p>{fault.body}</p></details>)}<button type="button" onClick={onModels}>{t.openModels}</button></section>
      </div> : <div className="help-content">
        <h3>{t.about}</h3>
        <dl className="about-facts"><div><dt>{t.version}</dt><dd>{packageInfo.version}</dd></div><div><dt>{t.core}</dt><dd>{runtime?.coreVersion ?? t.unavailable}</dd></div></dl>
        <section className="help-detail"><h3>{t.privacy}</h3><p>{t.privacyBody}</p></section>
        <section className="help-detail"><h3>{e.updates}</h3><p>{e.updatesBody}</p></section>
        <section className="help-detail"><h3>{t.licenses}</h3><p>{t.licensesBody}</p><button type="button" onClick={onModels}>{t.openModels}</button></section>
      </div>}
    </div>
  </section>;
}
