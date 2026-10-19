import { execFile } from 'node:child_process';
import { mkdir, readFile, realpath, rename, rm } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { root, run, sha256 } from './prepare-runtime.mjs';

export async function buildDmg(env) {
  const execFileAsync = promisify(execFile);
  const { stdout } = await execFileAsync(env.CLEARFRAME_PYTHON ?? 'python3', ['-c',
    'import json,sys;print(json.dumps({"executable":sys.executable,"version":list(sys.version_info[:2])}))']);
  const python = JSON.parse(stdout);
  if (python.version[0] !== 3 || python.version[1] < 10) throw new Error('macOS DMG packaging requires Python 3.10+ (or set CLEARFRAME_PYTHON).');
  // Resolve managed Python shims so venv finds the original standard library.
  const executable = await realpath(python.executable);
  const directory = join(root, 'target/installer-python');
  await run(executable, ['-m', 'venv', directory], { env });
  const isolated = join(directory, 'bin/python');
  const wheelDir = join(root, 'target/installer-cache/dmg-wheels');
  await mkdir(wheelDir, { recursive: true });
  const wheels = JSON.parse(await readFile(join(root, 'tools/dmg-wheels.json'), 'utf8'));
  for (const wheel of wheels) {
    const destination = join(wheelDir, wheel.filename);
    try { if (await sha256(destination) === wheel.sha256) continue; } catch { /* Fetch missing cache. */ }
    const partial = `${destination}.${randomUUID()}.part`;
    try {
      await run('curl', ['--fail', '--location', '--retry', '2', '--max-time', '120',
        '--output', partial, wheel.url], { env });
      if (await sha256(partial) !== wheel.sha256) throw new Error(`DMG build dependency SHA-256 mismatch: ${wheel.filename}`);
      await rename(partial, destination);
    } finally {
      await rm(partial, { force: true });
    }
  }
  await run(isolated, ['-m', 'pip', 'install', '--disable-pip-version-check', '--require-hashes',
    '--no-index', '--find-links', wheelDir, '--only-binary=:all:',
    '-r', join(root, 'tools/dmg-requirements.txt')], { env });
  await run(isolated, [join(root, 'tools/build-dmg.py')], { env });
}
