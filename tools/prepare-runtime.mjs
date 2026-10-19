import { createHash, randomUUID } from 'node:crypto';
import { createReadStream, createWriteStream } from 'node:fs';
import { copyFile, mkdir, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { Readable, Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const manifest = JSON.parse(await readFile(join(root, 'tools/runtime-manifest.json'), 'utf8'));
const execFileAsync = promisify(execFile);

export async function validateMsvcDirectory(directory) {
  for (const name of ['msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll']) {
    const path = join(directory, name);
    try { await validateWindowsLibrary(path); } catch (error) { throw new Error(`Missing MSVC redistributable or invalid DLL: ${error.message}`); }
  }
}

export async function validateWindowsLibrary(path) {
  let bytes;
  try { bytes = await readFile(path); } catch { throw new Error(`Missing Windows runtime DLL: ${path}`); }
  const offset = bytes.length >= 64 ? bytes.readUInt32LE(0x3c) : -1;
  if (bytes.toString('ascii', 0, 2) !== 'MZ' || offset < 0 || offset + 6 > bytes.length
    || bytes.toString('ascii', offset, offset + 4) !== 'PE\0\0' || bytes.readUInt16LE(offset + 4) !== 0x8664) {
    throw new Error(`File is not a Windows x64 DLL: ${path}`);
  }
}

async function msvcDirectory() {
  if (process.env.CLEARFRAME_MSVC_REDIST_DIR) {
    const directory = resolve(process.env.CLEARFRAME_MSVC_REDIST_DIR);
    await validateMsvcDirectory(directory);
    return directory;
  }
  let redist = process.env.VCToolsRedistDir;
  if (!redist) {
    const vswhere = join(process.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)',
      'Microsoft Visual Studio/Installer/vswhere.exe');
    const { stdout } = await execFileAsync(vswhere, ['-latest', '-products', '*',
      '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath']);
    if (!stdout.trim()) throw new Error('Install current Visual Studio C++ Build Tools with x64 redistributables.');
    const base = join(stdout.trim(), 'VC/Redist/MSVC');
    const versions = (await readdir(base)).filter(name => /^\d+\.\d+\.\d+$/.test(name))
      .sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
    if (!versions.length) throw new Error('MSVC redistributables not found; set CLEARFRAME_MSVC_REDIST_DIR.');
    redist = join(base, versions[0]);
  }
  const architectureDir = join(redist, 'x64');
  const folders = (await readdir(architectureDir)).filter(name => /^Microsoft\.VC\d+\.CRT$/.test(name));
  for (const folder of folders.sort().reverse()) {
    const directory = join(architectureDir, folder);
    try { await validateMsvcDirectory(directory); return directory; } catch { /* Try another installed toolset. */ }
  }
  throw new Error('Missing x64 MSVC CRT DLLs; set CLEARFRAME_MSVC_REDIST_DIR to a current redistributable CRT directory.');
}

export function runtimeTarget(platform = process.platform, arch = process.arch) {
  const target = `${platform}-${arch}`;
  const entry = manifest.targets[target];
  if (!entry) throw new Error(`Unsupported installer target: ${target}. Use macOS Apple Silicon or Windows x64.`);
  return { version: manifest.version, ...entry, target,
    url: entry.url ?? `https://github.com/microsoft/onnxruntime/releases/download/v${manifest.version}/${entry.archive}` };
}

export function validateNugetMetadata(xml, entry) {
  if (!xml.includes(`<id>${entry.packageId}</id>`) || !xml.includes(`<version>${entry.version}</version>`)) {
    throw new Error(`NuGet identity/version mismatch: ${entry.packageId}`);
  }
}

// Exported so both pinned Windows packages can be checked on a non-Windows host.
export async function stageRuntimeArchive(archive, entry, temporary, staged) {
  await verifyArchive(archive, entry);
  await mkdir(temporary, { recursive: true });
  await run('tar', ['-xf', archive, '-C', temporary]);
  const extracted = entry.format === 'nuget' ? temporary
    : join(temporary, entry.archive.replace(/\.(tgz|zip)$/, ''));
  if (entry.format === 'nuget') {
    validateNugetMetadata(await readFile(join(extracted, `${entry.packageId}.nuspec`), 'utf8'), entry);
  } else if ((await readFile(join(extracted, 'VERSION_NUMBER'), 'utf8')).trim() !== entry.version) {
    throw new Error('Runtime version does not match the pinned manifest');
  }
  const notices = join(staged, 'notices', ...(entry.packageId ? [entry.packageId] : []));
  await mkdir(notices, { recursive: true });
  for (const name of entry.notices ?? ['LICENSE', 'ThirdPartyNotices.txt', 'Privacy.md', 'VERSION_NUMBER', 'GIT_COMMIT_ID']) {
    await copyFile(join(extracted, name), join(notices, name));
  }
  const libraries = entry.libraries ?? [entry.library];
  if (entry.library && !libraries.includes(entry.library)) throw new Error('Runtime library is missing');
  const files = {};
  for (const source of libraries) {
    const name = source.endsWith('.dylib') ? 'libonnxruntime.dylib' : basename(source);
    const path = join(extracted, source);
    if (name.endsWith('.dll')) await validateWindowsLibrary(path);
    await copyFile(path, join(staged, name));
    files[name] = await sha256(join(staged, name));
  }
  return files;
}

export async function sha256(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}

export async function verifyArchive(path, entry) {
  if ((await stat(path)).size !== entry.bytes) throw new Error(`Runtime archive size mismatch: ${path}`);
  if (await sha256(path) !== entry.sha256) throw new Error(`Runtime archive SHA-256 mismatch: ${path}`);
}

export function run(command, args, options = {}) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(command, args, { cwd: root, stdio: 'inherit', ...options });
    child.once('error', reject);
    child.once('exit', (code, signal) => {
      if (code === 0) resolveRun();
      else reject(new Error(`${command} failed (${signal ?? code})`));
    });
  });
}

async function downloadArchive(path, entry) {
  const partial = `${path}.${randomUUID()}.part`;
  try {
    console.log(`Downloading ${entry.packageId ?? 'ONNX Runtime'} ${entry.version}…`);
    const response = await fetch(entry.url, { signal: AbortSignal.timeout(20 * 60 * 1000) });
    if (!response.ok || !response.body) throw new Error(`Runtime download failed: HTTP ${response.status}`);
    let bytes = 0;
    const limit = new Transform({ transform(chunk, encoding, callback) {
      bytes += chunk.length;
      callback(bytes > entry.bytes ? new Error('Runtime archive exceeds expected size') : null, chunk);
    } });
    await pipeline(Readable.fromWeb(response.body), limit, createWriteStream(partial, { flags: 'wx' }));
    await verifyArchive(partial, entry);
    await rename(partial, path);
  } finally {
    await rm(partial, { force: true });
  }
}

async function obtainArchive(entry, cache, archivePath) {
  const archive = archivePath ? resolve(archivePath) : join(cache, entry.archive);
  if (archivePath) {
    await verifyArchive(archive, entry);
  } else {
    try {
      await verifyArchive(archive, entry);
    } catch {
      await downloadArchive(archive, entry);
    }
  }
  return archive;
}

export async function prepareRuntime({ archivePath = process.env.CLEARFRAME_ORT_ARCHIVE } = {}) {
  const entry = runtimeTarget();
  const cache = join(root, 'target/installer-cache');
  await mkdir(cache, { recursive: true });
  const temporary = join(cache, `extract-${randomUUID()}`);
  const staged = join(root, 'resources', `runtime-${randomUUID()}`);
  const destination = join(root, 'resources/runtime');
  try {
    await mkdir(staged);
    const files = {};
    const packages = [entry, ...(entry.dependencies ?? [])];
    for (const [index, dependency] of packages.entries()) {
      const override = index === 0 ? archivePath : process.env.CLEARFRAME_DIRECTML_ARCHIVE;
      const archive = await obtainArchive(dependency, cache, override);
      Object.assign(files, await stageRuntimeArchive(archive, dependency, join(temporary, String(index)), staged));
    }
    let msvc;
    if (entry.target === 'win32-x64') {
      const directory = await msvcDirectory();
      for (const name of (await readdir(directory)).filter(name => name.toLowerCase().endsWith('.dll'))) {
        await copyFile(join(directory, name), join(staged, name));
        files[name] = await sha256(join(staged, name));
      }
      msvc = { source: directory, deployment: 'app-local' };
      await writeFile(join(staged, 'notices/MSVC.txt'),
        'Microsoft Visual C++ Runtime, redistributed from the build machine Visual Studio Redist directory.\n'
        + 'Subject to the Microsoft Visual Studio license terms.\n'
        + 'https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files\n'
        + 'App-local runtime security updates are delivered with ClearFrame application updates.\n');
    }
    await writeFile(join(staged, 'runtime.json'), `${JSON.stringify({
      version: entry.version, target: entry.target, source: entry.url,
      archiveBytes: entry.bytes, archiveSha256: entry.sha256, files, msvc,
      dependencies: (entry.dependencies ?? []).map(dependency => ({
        packageId: dependency.packageId, version: dependency.version, source: dependency.url,
        archiveBytes: dependency.bytes, archiveSha256: dependency.sha256,
      })),
    }, null, 2)}\n`);
    await rm(destination, { recursive: true, force: true });
    await rename(staged, destination);
    console.log(`Prepared ${entry.target} runtime and license notices: resources/runtime/`);
    return entry;
  } finally {
    await rm(temporary, { recursive: true, force: true });
    await rm(staged, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  prepareRuntime().catch(error => { console.error(error.message); process.exitCode = 1; });
}
