import { execFile } from 'node:child_process';
import { chmod, copyFile, cp, mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { promisify } from 'node:util';
import { root, run } from './prepare-runtime.mjs';

export async function prepareContextMenu(macOS, env) {
  const output = join(root, 'resources/context-menu');
  await mkdir(output, { recursive: true });
  if (macOS) {
    const contents = join(output, 'Context Menu Setup.app/Contents');
    const resources = join(contents, 'Resources');
    await mkdir(join(contents, 'MacOS'), { recursive: true });
    await mkdir(resources, { recursive: true });
    await cp(join(root, 'src-tauri/installer/context/locales'), join(resources, 'locales'), { recursive: true });
    await run(env.CLEARFRAME_PYTHON ?? 'python3', [join(root, 'tools/generate-context-workflows.py'), join(resources, 'workflows')], { env });
    await writeFile(join(contents, 'Info.plist'), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.silta.clearframe.desktop.contextsetup</string>
<key>CFBundleName</key><string>ClearFrame Context Menu Setup</string>
<key>CFBundleExecutable</key><string>clearframe-context-setup</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>LSUIElement</key><false/>
<key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>\n`);
    const binary = join(contents, 'MacOS/clearframe-context-setup');
    await run('swiftc', ['-target', 'arm64-apple-macosx14.0', '-O', '-module-cache-path', join(root, 'target/swift-module-cache'),
      join(root, 'tools/context-setup.swift'), '-o', binary], { env });
    await chmod(binary, 0o755);
    const identity = env.APPLE_SIGNING_IDENTITY ?? '-';
    await run('codesign', ['--force', '--sign', identity,
      ...(identity === '-' ? [] : ['--options', 'runtime', '--timestamp']), join(output, 'Context Menu Setup.app')], { env });
  } else {
    const execFileAsync = promisify(execFile);
    const vswhere = join(env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)', 'Microsoft Visual Studio/Installer/vswhere.exe');
    const { stdout } = await execFileAsync(vswhere, ['-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath']);
    if (!stdout.trim()) throw new Error('The context menu adapter requires Visual Studio x64 C++ Build Tools.');
    const build = join(root, 'target/context-menu');
    await mkdir(build, { recursive: true });
    const source = join(root, 'src-tauri/installer/context');
    // Batch file paths come only from trusted local build directories, never selection data.
    for (const path of [root, stdout.trim()]) if (/[\r\n"%!]/.test(path)) throw new Error('Unsupported Windows build directory characters.');
    const script = join(build, 'build.cmd');
    await writeFile(script, `@echo off\r\ncall "${join(stdout.trim(), 'VC/Auxiliary/Build/vcvarsall.bat')}" x64\r\nif errorlevel 1 exit /b 1\r\nrc /nologo /fo "${join(build, 'shell.res')}" "${join(source, 'windows-shell.rc')}"\r\nif errorlevel 1 exit /b 1\r\ncl /nologo /std:c++17 /EHsc /MT /LD /W4 /utf-8 "${join(source, 'windows-shell.cpp')}" "${join(build, 'shell.res')}" /Fo"${join(build, 'shell.obj')}" /link /OUT:"${join(output, 'clearframe-shell.dll')}" /IMPLIB:"${join(build, 'shell.lib')}" shell32.lib ole32.lib uuid.lib user32.lib\r\nexit /b %errorlevel%\r\n`);
    await run('cmd.exe', ['/d', '/s', '/c', `"${script}"`], { env });
    // Keep build output outside installer source control.
    await copyFile(join(output, 'clearframe-shell.dll'), join(build, 'clearframe-shell.dll'));
  }
}
