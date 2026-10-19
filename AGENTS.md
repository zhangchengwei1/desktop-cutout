# Repository Guidelines

## Project Structure & Module Organization

ClearFrame Desktop uses React, TypeScript, Tauri 2, and Rust for local background removal.

- `src/`: UI in `app/`, `components/`, and `features/`; native calls in `platform/`; translations in `i18n/locales/{en,zh,ja}/`.
- `src-tauri/`: native commands, windows, menus, permissions, and installer configuration.
- `crates/cutout-core/`: Tauri-independent image processing, inference, editing, task scheduling, and recovery.
- `crates/cutout-cli/`: headless processing and validation tools.
- `tests/` and `crates/*/tests/`: frontend tests, Rust tests, and image baselines.
- `resources/`, `public/`, `tools/`, and `docs/`: manifests, assets, packaging tools, and Chinese development records.

## Build, Test, and Development Commands

Requires Node.js 22.12+, npm, Rust, and platform dependencies. Run from the root:

```bash
npm ci                       # Install locked dependencies
npm run dev                  # Browser preview at localhost:1420
npm run desktop:dev          # Launch the native app
npm run check                # TypeScript checks and Vite build
npm run installer:prepare    # Download and verify native runtime
npm run desktop:build        # Build without an installer
npm run desktop:installer    # Build a native DMG or NSIS installer
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
```

Development inference needs `CLEARFRAME_ORT_LIBRARY` pointing to the prepared runtime; see `README.md`. Browser preview lacks native processing.

## Coding Style & Naming Conventions

Use two-space indentation, single quotes, semicolons, and explicit type imports in TypeScript. Name React components in PascalCase and Rust functions/modules in snake_case; format Rust with rustfmt. Keep Tauri API access inside `src/platform/`. Synchronize English, Chinese, and Japanese UI strings and READMEs.

## Testing Guidelines

Use Rust's built-in tests and Node's test runner (`*.test.mjs`, `*.test.ts`). Run relevant `npm run test:contracts`, `test:i18n`, `test:shared`, and `test:installer` scripts. Verify image baselines with `npm run baseline:verify`. Add behavioral tests for core changes; model/GPU probes require additional resources. IPC, permissions, and native file changes need real Tauri validation. Report unexecuted checks explicitly.

## Commit & Pull Request Guidelines

History contains only `Initial commit`; no prefix convention exists. Use imperative commit subjects. PRs should explain behavior, link relevant issues, list validation and limitations, and include UI screenshots or image-quality comparisons.

## Architecture & Security

Keep this project independent of sibling `browser-cutout`. Rust owns task scheduling; exchange identifiers and previews over IPC rather than full images or weights. Synchronize command handlers, `build.rs`, permissions, and frontend types. Preserve fixed FP32 model quality, source alpha, orientation, and stale-result protection. Never upload user images; exclude weights, credentials, binaries, and test output from commits. Preserve lockfiles and user changes.
