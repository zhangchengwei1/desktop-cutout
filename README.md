# ClearFrame Desktop

**English** · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

A local background removal desktop app for macOS and Windows, built with React, TypeScript, Tauri 2, and a standalone Rust processing core. Images, masks, and results stay on your device. The interface supports English, Simplified Chinese, and Japanese.

## Development status

The development build includes automatic background removal, point selection, matte refinement, mask painting, backgrounds, undo/redo, PNG export, folder/ZIP batch processing, model management, optional session recovery, and system context menu integration.

This project is still under development. macOS Apple Silicon has local native and DMG validation records. Windows 11 x64 builds, installation, and DirectML acceleration require real-device validation. Image quality parity, larger workloads, release signing, notarization, and clean-system acceptance remain incomplete. See the [development checklist](docs/07-桌面端迁移开发指南.md).

## Getting started

Install Node.js **22.12+**, npm, Rust, and the Tauri platform build dependencies. macOS requires Xcode Command Line Tools; Windows requires MSVC C++ Build Tools, Windows SDK, CMake, and WebView2.

Run from this repository root:

```bash
npm ci
npm run installer:prepare
```

The preparation command downloads and verifies the platform ONNX Runtime into `resources/runtime/`. Point development builds at its absolute library path:

macOS Apple Silicon:

```bash
CLEARFRAME_ORT_LIBRARY="$PWD/resources/runtime/libonnxruntime.dylib" npm run desktop:dev
```

Windows x64, in PowerShell:

```powershell
$env:CLEARFRAME_ORT_LIBRARY = (Resolve-Path resources/runtime/onnxruntime.dll).Path
npm run desktop:dev
```

The app downloads missing models and verifies their size and SHA-256 before use. Processing works offline once the required models are cached. Models are excluded from installers and Git. The fixed FP32 pipeline uses IS-Net, MobileSAM, and ViTMatte; CPU fallback retains the same weights and input sizes.

For interface preview only, run `npm run dev` and open `http://localhost:1420`. Browser preview has no native IPC or image processing.

## Commands

All 19 scripts from [`package.json`](package.json) are listed below. Run them from the repository root. `build` and `preview` operate on the frontend only; use the desktop commands for native processing.

### Frontend development

| Command | Purpose |
| --- | --- |
| `npm run dev` | Start Vite development preview at `http://localhost:1420`; the port must be available |
| `npm run typecheck` | Run `tsc --noEmit` without producing build output |
| `npm run build` | Build frontend assets into `dist/`; does not run type checking |
| `npm run preview` | Serve the existing frontend build locally; run `build` first and use the URL printed by Vite |
| `npm run check` | TypeScript checks and Vite production build |

### Native development and packaging

| Command | Purpose |
| --- | --- |
| `npm run tauri` | Invoke the local Tauri CLI; forward arguments with `--`, for example `npm run tauri -- --help` |
| `npm run desktop:dev` | Start the native app and Vite development server; requires platform tooling and a runtime for inference |
| `npm run desktop:build` | Check types and build the native application without bundling an installer |
| `npm run installer:prepare` | Download, verify, and stage the native runtime, notices, and Windows CRT in `resources/runtime/` |
| `npm run desktop:installer` | Prepare resources, build the native app, check packaged runtime loading, and create a DMG or NSIS installer with SHA-256 |

### Tests and image references

| Command | Purpose |
| --- | --- |
| `npm run test:contracts` | Native command registration and permission checks |
| `npm run test:i18n` | Translation resource checks |
| `npm run test:shared` | Shared frontend behavior tests |
| `npm run test:installer` | Installer tooling tests |
| `npm run test:gpu:windows` | Windows-only DirectML quality, provider, error recovery, and CPU fallback checks; requires `-ModelDirectory` |
| `npm run baseline:verify` | Verify fixed image/reference hashes; requires Python 3 |
| `npm run baseline:isnet` | Run fixed IS-Net references using ORT Web/WASM CPU and frozen Canvas tensors; requires the local IS-Net weight |
| `npm run baseline:s04:model` | Verify or capture MobileSAM and ViTMatte references on the frozen S04 sample; requires their three weights |
| `npm run baseline:s08:sam` | Verify or capture MobileSAM references for wide/tall images; requires encoder and decoder weights |

Rust workspace commands are separate from npm scripts:

| Command | Purpose |
| --- | --- |
| `cargo check --workspace` | Check Rust workspace |
| `cargo test --workspace` | Run Rust tests; model/GPU probes need extra setup |
| `cargo fmt --all -- --check` | Check Rust formatting |

## Validation workflows

For frontend or native boundary changes, run the relevant checks:

```bash
npm run check
npm run test:contracts
npm run test:i18n
npm run test:shared
npm run test:installer
```

For Rust changes, also run the three workspace commands above. Image pipeline changes need fixed-sample comparisons, not just a successful build. Native IPC, file dialogs, menus, and permissions require verification in a Tauri window. A browser preview or unit test does not establish native or GPU readiness.

### Model reference prerequisites

The three model reference scripts use ORT Web/WASM CPU under Node, separate from production native inference. Obtain the exact weights in [`model-snapshot.json`](tests/fixtures/model-snapshot.json), check their byte sizes and SHA-256, and place them under the ignored directory:

```text
test-output/models/
├─ isnet-auto-high.onnx
├─ mobilesam-image-encoder.onnx
├─ mobilesam-mask-decoder.onnx
└─ vitmatte-high.onnx
```

They use the existing frozen Canvas inputs under `tests/fixtures/reference/` and do not download weights. Existing reference mismatches fail by default. Passing `-- --refresh` explicitly replaces reference metadata/masks; review and document any baseline change before using it. Run `npm run baseline:verify` afterward. See the [fixture guide](tests/fixtures/README.md) for browser input capture, image generation, and numerical acceptance limits.

### Windows GPU verification

On Windows x64 with a DirectX 12 GPU, prepare the runtime and let the app download the required models. Then run in PowerShell:

```powershell
npm run installer:prepare
npm run test:gpu:windows -- -ModelDirectory "$env:LOCALAPPDATA\dev.silta.clearframe.desktop\models"
```

Optionally append `-Image "C:\Images\example.png"`; otherwise the script uses a fixed synthetic sample. Logs, quality reports, and provider profiles go to `test-output/acceleration-windows/`. Actual DirectML execution must be present in the profile; CPU fallback alone cannot pass GPU verification. See the [Windows validation guide](docs/29-Windows-GPU开发与真机验证.md) for prerequisites and outstanding device checks.

## Headless CLI

`cutout-cli` calls the same Rust core without opening a window:

```bash
cargo run -p cutout-cli -- --help
cargo run -p cutout-cli -- info
cargo run -p cutout-cli -- inspect tests/fixtures/images/hard-edge-opaque.png
cargo run -p cutout-cli -- cutout --help
```

Other subcommands include `apply-mask`, `verify-model`, `infer-tensor`, `select`, `refine`, and `batch-bench`. Inference commands take explicit runtime/model paths; use each subcommand's `--help` for positional arguments. The default `info` result is core metadata, not proof that a desktop model session is ready.

## Configuration and local data

Set environment variables in the shell that launches the command; the native tools do not automatically load a `.env` file.

| Variable | Use |
| --- | --- |
| `CLEARFRAME_ORT_LIBRARY` | Absolute native runtime library path for debug app builds |
| `CLEARFRAME_MODEL_DIR` | Override the model cache directory for debug app builds |
| `CLEARFRAME_ORT_ARCHIVE` | Use a local pinned ORT archive during resource preparation; size/hash verification still applies |
| `CLEARFRAME_DIRECTML_ARCHIVE` | Use the local pinned DirectML dependency archive for Windows preparation |
| `CLEARFRAME_MSVC_REDIST_DIR` | Override the Windows x64 MSVC CRT source directory |
| `CLEARFRAME_PYTHON` | Choose the Python 3.10+ interpreter for macOS DMG packaging |
| `APPLE_SIGNING_IDENTITY` | Select the macOS signing identity for installer builds |

Release builds use their packaged runtime and application data directory rather than the debug runtime/model overrides. Default data locations are:

- macOS: `~/Library/Application Support/dev.silta.clearframe.desktop/`
- Windows: `%LOCALAPPDATA%\dev.silta.clearframe.desktop\`

Models, preferences, and optional recovery data are managed locally. Uninstalling the app does not automatically delete them. Use the app's resource/settings controls for cleanup; user export directories are separate. Model sources, revisions, hashes, and license URLs are defined in the [Rust model manifest](crates/cutout-core/src/model.rs); runtime archives are pinned in the [runtime manifest](tools/runtime-manifest.json).

## Building installers

Installer targets are macOS 14+ Apple Silicon and Windows 11 x64; Intel Macs and Windows ARM64 are not current installer targets. Build on the target OS:

```bash
npm ci
npm run desktop:installer
```

The command prepares resources automatically. macOS DMG packaging also requires Python 3.10+ and creates its isolated tooling under `target/`. Windows needs x64 MSVC redistributables; the packager obtains NSIS and an offline WebView2 installer. First builds require network access unless the necessary downloads are already available.

| Platform | Installer output, with an adjacent `.sha256` file |
| --- | --- |
| macOS | `target/release/bundle/dmg/ClearFrame Desktop_<version>_aarch64.dmg` |
| Windows | `target/release/bundle/nsis/ClearFrame Desktop_<version>_x64-setup.exe` |

Unset `CARGO_BUILD_TARGET` and `CARGO_TARGET_DIR` before packaging; the installer script rejects target overrides and extra arguments. Local macOS builds default to ad-hoc signing. Public distribution still requires Developer ID/notarization or Windows Authenticode configuration and release acceptance. The packaged runtime load check does not establish model quality or GPU execution. See the [packaging guide](docs/21-安装器与分发.md).

## Troubleshooting

| Symptom | Action |
| --- | --- |
| Vite cannot start on port 1420 | Stop the process using that port; development uses a strict port |
| UI opens but processing is unavailable | Use `desktop:dev`, verify the runtime path, and inspect model download/session errors |
| Model reference script reports a missing file | Prepare the exact weight filenames above; the app cache and reference directory are separate |
| A baseline hash changes | Inspect the input/model/algorithm difference before explicitly refreshing the reference |
| Runtime preparation fails | Check archive integrity and platform/CRT requirements; retain hash checks |

## Repository structure

```text
src/                 React UI, translations, shared utilities, native API boundary
src-tauri/           Tauri commands, windows, menus, permissions, installer sources
crates/cutout-core/  Tauri-independent image, model, editing, and task logic
crates/cutout-cli/   Headless processing and validation CLI
resources/           Resource manifests; generated runtime files are ignored
tests/               Contract, translation, shared, installer, and image baselines
tools/               Runtime preparation, packaging, and platform verification
docs/                Architecture, migration, and validation records (Chinese)
```

## Contributing

Read [AGENTS.md](AGENTS.md) for module boundaries, quality requirements, and validation rules. Keep English, Chinese, and Japanese READMEs aligned. This repository is independent of the sibling `browser-cutout` project. Commit lockfiles and documented synthetic fixtures; keep model weights, user images, exports, credentials, and generated binaries out of Git. Use concise English commit messages and include validation results and remaining platform limitations in pull requests.
