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

| Command | Purpose |
| --- | --- |
| `npm run check` | TypeScript checks and Vite production build |
| `npm run test:contracts` | Native command registration and permission checks |
| `npm run test:i18n` | Translation resource checks |
| `npm run test:shared` | Shared frontend behavior tests |
| `npm run test:installer` | Installer tooling tests |
| `npm run baseline:verify` | Verify fixed image/reference hashes; requires Python 3 |
| `cargo check --workspace` | Check Rust workspace |
| `cargo test --workspace` | Run Rust tests; model/GPU probes need extra setup |
| `cargo fmt --all -- --check` | Check Rust formatting |
| `npm run desktop:build` | Build the native application without an installer |
| `npm run desktop:installer` | Build an installer on the target OS |

Installer targets are macOS 14+ Apple Silicon and Windows 11 x64. macOS DMG packaging also requires Python 3.10+. Artifacts and SHA-256 files go to `target/release/bundle/`. Local macOS builds default to ad-hoc signing; public distribution requires release credentials and acceptance checks. See the [packaging guide](docs/21-安装器与分发.md).

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

Read [AGENTS.md](AGENTS.md) for module boundaries, quality requirements, and validation rules. Keep English, Chinese, and Japanese READMEs aligned. This repository is independent of the sibling `browser-cutout` project. Commit lockfiles and documented synthetic fixtures; keep model weights, user images, exports, credentials, and generated binaries out of Git. Model and runtime licenses are documented in the [resource guide](resources/README.md).
