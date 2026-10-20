import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { runtimeTarget, verifyArchive, validateMsvcDirectory, validateNugetMetadata } from '../../tools/prepare-runtime.mjs';
import { macosSigningConfig } from '../../tools/installer-signing.mjs';

test('ad-hoc builds can load bundled libraries while certificate builds retain hardened runtime', () => {
  assert.equal(macosSigningConfig('-').bundle.macOS.hardenedRuntime, false);
  assert.equal(macosSigningConfig('Developer ID Application: Example (TEAM)').bundle.macOS.hardenedRuntime, true);
  assert.throws(() => macosSigningConfig(''), /identity is required/);
});

test('unsupported architectures fail instead of silently packaging the wrong runtime', () => {
  assert.throws(() => runtimeTarget('darwin', 'x64'), /Unsupported installer target/);
  assert.throws(() => runtimeTarget('win32', 'arm64'), /Unsupported installer target/);
  assert.equal(runtimeTarget('darwin', 'arm64').version, '1.28.0');
  const windows = runtimeTarget('win32', 'x64');
  assert.equal(windows.version, '1.24.4');
  assert.equal(windows.library, 'runtimes/win-x64/native/onnxruntime.dll');
  assert.equal(windows.dependencies[0].version, '1.15.4');
  assert.deepEqual(windows.dependencies[0].libraries, ['bin/x64-win/DirectML.dll']);
});

test('NuGet metadata rejects a different package or runtime version', () => {
  const entry = runtimeTarget('win32', 'x64');
  const xml = `<package><id>${entry.packageId}</id><version>${entry.version}</version></package>`;
  validateNugetMetadata(xml, entry);
  assert.throws(() => validateNugetMetadata(xml.replace('1.24.4', '1.28.0'), entry), /identity\/version mismatch/);
  assert.throws(() => validateNugetMetadata(xml.replace('.DirectML', ''), entry), /identity\/version mismatch/);
});

test('Windows prerequisite check rejects missing DLLs and incorrect architectures', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'clearframe-msvc-test-'));
  try {
    await assert.rejects(validateMsvcDirectory(directory), /Missing MSVC redistributable/);
    const bytes = Buffer.alloc(128);
    bytes.write('MZ'); bytes.writeUInt32LE(64, 0x3c); bytes.write('PE\0\0', 64); bytes.writeUInt16LE(0x14c, 68);
    for (const name of ['msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll']) {
      await writeFile(join(directory, name), bytes);
    }
    await assert.rejects(validateMsvcDirectory(directory), /not a Windows x64 DLL/);
    bytes.writeUInt16LE(0x8664, 68);
    for (const name of ['msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll']) {
      await writeFile(join(directory, name), bytes);
    }
    await validateMsvcDirectory(directory);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test('archive integrity rejects truncation and equal-size corruption before extraction', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'clearframe-runtime-test-'));
  const path = join(directory, 'runtime.tgz');
  const content = Buffer.from('verified bytes');
  const entry = { bytes: content.length, sha256: createHash('sha256').update(content).digest('hex') };
  try {
    await writeFile(path, content);
    await verifyArchive(path, entry);
    await writeFile(path, content.subarray(1));
    await assert.rejects(verifyArchive(path, entry), /size mismatch/);
    await writeFile(path, Buffer.alloc(content.length));
    await assert.rejects(verifyArchive(path, entry), /SHA-256 mismatch/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
