import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { promisify } from 'node:util';

const run = promisify(execFile);
const root = new URL('../../', import.meta.url);

test('native consent resources have matching nonempty English, Chinese and Japanese keys', async () => {
  const resources = await Promise.all(['en', 'zh', 'ja'].map(async language =>
    JSON.parse(await readFile(new URL(`src-tauri/installer/context/locales/${language}.json`, root), 'utf8'))));
  for (const resource of resources) {
    assert.deepEqual(Object.keys(resource).sort(), Object.keys(resources[0]).sort());
    assert.ok(Object.values(resource).every(value => typeof value === 'string' && value.trim()));
  }
});

test('Finder workflow packages preserve file selections as literal arguments', { skip: process.platform === 'win32' }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'clearframe-workflow-test-'));
  try {
    await run(process.env.CLEARFRAME_PYTHON ?? 'python3', [new URL('tools/generate-context-workflows.py', root).pathname, directory]);
    // Parse the real generated plist, then execute its command with a test
    // executable and hostile-looking names. Never interpolate selection data.
    await run(process.env.CLEARFRAME_PYTHON ?? 'python3', ['-c', `
import json, os, plistlib, shlex, subprocess, sys, time
from pathlib import Path
root = Path(sys.argv[1])
executable = root / "capture 日本語 ' & args"
executable.write_text('#!/usr/bin/env python3\\nimport json, sys\\nfrom pathlib import Path\\nPath(' + repr(str(root / 'captured.json')) + ').write_text(json.dumps(sys.argv[1:]))\\n')
executable.chmod(0o755)
paths = ["--models.png", "a b.png", "日本語.png", "$(touch injected).jpg", "apostrophe'quote.webp"]
for mode in ['single', 'batch']:
    package = root / f'ClearFrame {mode.title()}.workflow/Contents'
    info = plistlib.loads((package / 'Info.plist').read_bytes())
    assert info['NSServices'][0]['NSRequiredContext']['NSApplicationIdentifier'] == 'com.apple.finder'
    workflow = plistlib.loads((package / 'Resources/document.wflow').read_bytes())
    action = workflow['actions'][0]['action']['ActionParameters']
    assert action['inputMethod'] == 1
    command = action['COMMAND_STRING'].replace('__CLEARFRAME_EXECUTABLE__', shlex.quote(str(executable)))
    result = subprocess.run(['/bin/sh', '-c', command, 'workflow', *paths], capture_output=True, text=True, check=True)
    captured = root / 'captured.json'\n    deadline = time.monotonic() + 5\n    while not captured.exists() and time.monotonic() < deadline: time.sleep(0.01)\n    assert json.loads(captured.read_text()) == ['--shell-cutout', mode, '--', *paths]\n    captured.unlink()
    assert not (root / 'injected').exists()
    for language in ['en', 'zh_CN', 'ja']:
        assert (package / f'Resources/{language}.lproj/ServicesMenu.strings').is_file()
`, directory]);
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});
