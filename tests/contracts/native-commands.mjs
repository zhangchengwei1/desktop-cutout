import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

const root = fileURLToPath(new URL('../../', import.meta.url));
const read = (path) => readFileSync(join(root, path), 'utf8');
const main = read('src-tauri/src/main.rs');
const build = read('src-tauri/build.rs');
const capability = JSON.parse(read('src-tauri/capabilities/main.json'));
const registered = new Set(main.match(/generate_handler!\[([\s\S]*?)\]/)[1]
  .split(',').map(value => value.trim().split('::').at(-1)).filter(Boolean));
const declared = new Set([...build.matchAll(/"([a-z_]+)"/g)].map((match) => match[1]));
const permitted = new Set(capability.permissions);
const invoked = new Set();
for (const file of readdirSync(join(root, 'src/platform')).filter((file) => file.endsWith('.ts'))) {
  for (const match of read(`src/platform/${file}`).matchAll(/invoke(?:<[^;]*?>)?\(\s*'([a-z_]+)'/g)) invoked.add(match[1]);
}
assert.ok(invoked.size >= 35, 'IPC discovery must include the batch and editing commands');
for (const command of invoked) {
  assert.ok(registered.has(command), `${command} is missing from generate_handler`);
  assert.ok(declared.has(command), `${command} is missing from build.rs`);
  assert.ok(permitted.has(`allow-${command.replaceAll('_', '-')}`), `${command} is missing from the main capability`);
}
const settings = JSON.parse(read('src-tauri/capabilities/settings.json'));
assert.deepEqual(settings.windows, ['settings']);
assert.ok(!settings.permissions.some((permission) => /batch|import|export|model/.test(permission)), 'Settings must retain its restricted capability');
for (const name of ['resources', 'help']) {
  const utility = JSON.parse(read(`src-tauri/capabilities/${name}.json`));
  assert.ok(!utility.permissions.some(permission => /batch|asset|preview|export|paint|history/.test(permission)), `${name} must not access workspace images`);
  assert.ok(!utility.permissions.includes('allow-update-preferences'), `${name} preferences must be read-only`);
}
const app = read('src/app/App.tsx');
const header = app.slice(app.indexOf('<header'), app.indexOf('</header>'));
assert.ok(!/header-actions|resource-link|openSettings|setPage\('help'\)/.test(header), 'Core header must only contain core navigation');
assert.ok(!/page === '(settings|help|resources)'/.test(app), 'Utility panels must not replace the core workspace');
for (const directory of ['src/components', 'src/features', 'src/app']) {
  for (const file of readdirSync(join(root, directory), { recursive: true }).filter(file => /\.tsx?$/.test(file))) {
    assert.ok(!read(`${directory}/${file}`).includes('@tauri-apps/'), `${directory}/${file} crosses the native boundary`);
  }
}
console.log(`Verified ${invoked.size} frontend commands across handler, build manifest and window permissions.`);

const extensions = JSON.parse(read('resources/image-formats.json')).extensions;
const shell = read('src-tauri/installer/context/windows-shell.cpp');
const hooks = read('src-tauri/installer/context/windows-hooks.nsh');
for (const extension of extensions) {
  assert.ok(shell.includes(`L".${extension}"`), `${extension} is missing from Explorer adapter`);
  assert.ok(hooks.includes(`SystemFileAssociations\\.${extension}\\shellex`), `${extension} uninstall registration missing`);
  assert.ok(hooks.includes(`CF_CONTEXT_HANDLER "SystemFileAssociations\\.${extension}"`), `${extension} install registration missing`);
}
