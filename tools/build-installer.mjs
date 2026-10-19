import { copyFile, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { prepareRuntime, root, run, sha256 } from './prepare-runtime.mjs';
import { buildDmg } from './build-dmg.mjs';
import { macosSigningConfig } from './installer-signing.mjs';
import { prepareContextMenu } from './prepare-context-menu.mjs';

try {
  if (process.argv.length > 2) throw new Error('Build on the target OS without extra arguments; use Tauri configuration for signing.');
  if (process.env.CARGO_BUILD_TARGET || process.env.CARGO_TARGET_DIR) {
    throw new Error('Unset CARGO_BUILD_TARGET and CARGO_TARGET_DIR; the installer uses the native target and workspace target directory.');
  }
  const entry = await prepareRuntime();
  await run(process.execPath, [join(root, 'node_modules/typescript/bin/tsc'), '--noEmit']);
  const macOS = entry.target === 'darwin-arm64';
  const cli = join(root, 'node_modules/@tauri-apps/cli/tauri.js');
  const env = { ...process.env };
  if (macOS && !env.APPLE_SIGNING_IDENTITY) {
    if (env.APPLE_CERTIFICATE) throw new Error('Set APPLE_SIGNING_IDENTITY when supplying an Apple signing certificate.');
    env.APPLE_SIGNING_IDENTITY = '-';
    console.log('Using an ad-hoc macOS signature for local testing. Public distribution requires Developer ID and notarization.');
  }
  await prepareContextMenu(macOS, env);
  await run(process.execPath, [cli, 'build', '--ci', '--features', 'custom-protocol',
    '--config', 'src-tauri/tauri.installer.conf.json',
    '--config', `src-tauri/tauri.installer.${macOS ? 'macos' : 'windows'}.conf.json`,
    ...(macOS ? ['--config', JSON.stringify(macosSigningConfig(env.APPLE_SIGNING_IDENTITY))] : []),
    '--bundles', macOS ? 'app' : 'nsis'], { env });
  if (macOS) {
    // Run the signed app itself: an unsigned CLI cannot exercise library validation.
    await run(join(root, 'target/release/bundle/macos/ClearFrame Desktop.app/Contents/MacOS/clearframe-desktop'),
      ['--verify-runtime'], { env });
    await buildDmg(env);
  } else {
    // Exercise app-local DLL loading in an isolated directory, without developer PATH fallbacks.
    const smoke = await mkdtemp(join(root, 'target/windows-runtime-smoke-'));
    try {
      await copyFile(join(root, 'target/release/clearframe-desktop.exe'), join(smoke, 'clearframe-desktop.exe'));
      for (const name of (await readdir(join(root, 'resources/runtime'))).filter(name => name.endsWith('.dll'))) {
        await copyFile(join(root, 'resources/runtime', name), join(smoke, name));
      }
      await run(join(smoke, 'clearframe-desktop.exe'), ['--verify-runtime'], {
        env: { ...env, PATH: join(process.env.SystemRoot ?? 'C:\\Windows', 'System32') }, cwd: smoke,
      });
    } finally {
      await rm(smoke, { recursive: true, force: true });
    }
  }
  const bundleDir = join(root, 'target/release/bundle', macOS ? 'dmg' : 'nsis');
  const extension = macOS ? '.dmg' : '.exe';
  const config = JSON.parse(await readFile(join(root, 'src-tauri/tauri.conf.json'), 'utf8'));
  const artifacts = (await readdir(bundleDir)).filter(name =>
    name.startsWith(`${config.productName}_${config.version}_`) && name.endsWith(extension));
  if (!artifacts.length) throw new Error('Tauri completed without an installer artifact');
  for (const name of artifacts) {
    const path = join(bundleDir, name);
    await writeFile(`${path}.sha256`, `${await sha256(path)}  ${name}\n`);
    console.log(`Installer: ${path}\nChecksum: ${path}.sha256`);
  }
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
