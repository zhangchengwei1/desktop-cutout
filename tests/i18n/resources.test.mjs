import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import test from 'node:test';
import { createInstance } from 'i18next';

const root = new URL('../../src/i18n/locales/', import.meta.url);
const languages = ['en', 'zh', 'ja'];
const namespaces = (await readdir(new URL('en/', root))).map(file => file.replace('.json', '')).sort();
const resources = Object.fromEntries(await Promise.all(languages.map(async language => [language,
  Object.fromEntries(await Promise.all(namespaces.map(async namespace => [namespace,
    JSON.parse(await readFile(new URL(`${language}/${namespace}.json`, root), 'utf8')),
  ]))),
])));

function leaves(value, prefix = '') {
  return Object.entries(value).flatMap(([key, entry]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    return typeof entry === 'object' && entry !== null ? leaves(entry, path) : [[path, entry]];
  });
}

test('all languages have matching keys, nonempty text and interpolation variables', () => {
  for (const namespace of namespaces) {
    const english = Object.fromEntries(leaves(resources.en[namespace]));
    for (const language of languages) {
      const translated = Object.fromEntries(leaves(resources[language][namespace]));
      assert.deepEqual(Object.keys(translated).sort(), Object.keys(english).sort(), `${language}/${namespace}`);
      for (const [key, text] of Object.entries(translated)) {
        assert.equal(typeof text, 'string', `${language}/${namespace}/${key}`);
        assert.ok(text.trim(), `${language}/${namespace}/${key}`);
        assert.deepEqual([...text.matchAll(/{{\s*(\w+)\s*}}/g)].map(match => match[1]).sort(),
          [...english[key].matchAll(/{{\s*(\w+)\s*}}/g)].map(match => match[1]).sort(), key);
      }
    }
  }
});

test('i18next handles counts, fallback and language switches without stale messages', async () => {
  const i18n = createInstance();
  await i18n.init({ resources, lng: 'en', fallbackLng: 'en', ns: namespaces, defaultNS: 'batch', interpolation: { escapeValue: false } });
  assert.equal(i18n.t('ui.copy.folderAdded', { count: 1 }), 'Added 1 image.');
  assert.equal(i18n.t('ui.copy.folderAdded', { count: 2 }), 'Added 2 images.');
  await i18n.changeLanguage('zh');
  assert.equal(i18n.t('ui.copy.folderAdded', { count: 2 }), '已加入 2 张图片。');
  await i18n.changeLanguage('ja');
  assert.ok(i18n.t('ui.copy.folderAdded', { count: 2 }).includes('2'));
  assert.equal(i18n.t('ui.copy.folderAdded', { lng: 'fr', count: 2 }), 'Added 2 images.');
  assert.equal(i18n.t('ui', { returnObjects: true }).copy.folderNew, resources.ja.batch.ui.copy.folderNew);
});
