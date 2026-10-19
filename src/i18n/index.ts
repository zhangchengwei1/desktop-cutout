import i18next from 'i18next';
import type { Resource, ResourceKey } from 'i18next';
import resourcesToBackend from 'i18next-resources-to-backend';
import { initReactI18next, useTranslation } from 'react-i18next';
import { useMemo } from 'react';
import type { Language } from '../platform/preferences';

type Messages = {
  shortcuts: typeof import('./locales/en/shortcuts.json')['ui'];
  menu: typeof import('./locales/en/menu.json')['ui'];
  shell: typeof import('./locales/en/shell.json')['ui'];
  app: typeof import('./locales/en/app.json')['ui'];
  batch: typeof import('./locales/en/batch.json')['ui'];
  help: typeof import('./locales/en/help.json')['ui'];
  settings: typeof import('./locales/en/settings.json')['ui'];
  models: typeof import('./locales/en/models.json')['ui'];
  setup: typeof import('./locales/en/setup.json')['ui'];
  runtime: typeof import('./locales/en/runtime.json')['ui'];
  errors: typeof import('./locales/en/errors.json')['ui'];
};

// Shell and error copy must be available before any page or language can suspend.
const bundled = import.meta.glob<ResourceKey>('./locales/*/{app,menu,runtime,errors}.json', { eager: true, import: 'default' });
const resources: Resource = {};
for (const [path, messages] of Object.entries(bundled)) {
  const [, language, file] = path.split('/').slice(1);
  (resources[language] ??= {})[file.replace('.json', '')] = messages;
}
const loaders = import.meta.glob(['./locales/*/*.json', '!./locales/*/{app,menu,runtime,errors}.json']);

void i18next.use(resourcesToBackend(async (language: string, namespace: string) => {
  const load = loaders[`./locales/${language}/${namespace}.json`];
  if (!load) throw new Error(`Unknown translation resource: ${language}/${namespace}`);
  return load();
})).use(initReactI18next).init({
  lng: 'en',
  fallbackLng: 'en',
  supportedLngs: ['zh', 'en', 'ja'],
  load: 'languageOnly',
  ns: [],
  resources,
  partialBundledLanguages: true,
  defaultNS: 'app',
  interpolation: { escapeValue: false },
  react: { useSuspense: true },
});

// Cache structured, typed messages until the language/namespace changes.
// Queue progress rerenders do not rebuild translation objects.
export function useMessages<N extends keyof Messages>(namespace: N, language: Language): Messages[N] {
  const { t } = useTranslation(namespace, { lng: language });
  return useMemo(() => t('ui', { returnObjects: true }) as Messages[N], [t]);
}

export function translate(namespace: keyof Messages, language: Language, key: string, options: Record<string, unknown> = {}): string {
  return i18next.t(`ui.${key}`, { ...options, ns: namespace, lng: language }) as string;
}

export function resolveSystemLanguage(locale = navigator.language): Language {
  const language = locale.toLowerCase().split(/[-_]/)[0];
  return language === 'ja' || language === 'en' ? language : 'zh';
}
