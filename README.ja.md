# ClearFrame Desktop

[English](README.md) · [简体中文](README.zh-CN.md) · **日本語**

macOS と Windows 向けのローカル背景除去アプリです。React、TypeScript、Tauri 2 と独立した Rust 処理コアで構成されています。画像、マスク、処理結果は端末内に保持されます。UI は英語、簡体字中国語、日本語に対応しています。

## 開発状況

開発版には、自動背景除去、ポイント選択、マットの精密調整、マスクブラシ、背景変更、元に戻す/やり直し、PNG 書き出し、フォルダー/ZIP の一括処理、モデル管理、任意のセッション復元、システムのコンテキストメニュー連携が実装されています。

現在も開発中です。macOS Apple Silicon ではネイティブ動作とローカル DMG の検証記録があります。Windows 11 x64 のビルド、インストール、DirectML 高速化は実機検証が必要です。画質の一致、大規模な処理負荷、配布用署名、公証、クリーン環境での受け入れ検証は未完了です。[開発チェックリスト](docs/07-桌面端迁移开发指南.md)を参照してください。

## 開発を始める

Node.js **22.12+**、npm、Rust、Tauri の各 OS 用ビルド依存関係を用意してください。macOS は Xcode Command Line Tools、Windows は MSVC C++ Build Tools、Windows SDK、CMake、WebView2 が必要です。

リポジトリのルートで実行します。

```bash
npm ci
npm run installer:prepare
```

準備コマンドは対象 OS の ONNX Runtime をダウンロードし、検証後に `resources/runtime/` に配置します。開発時はライブラリの絶対パスを指定してください。

macOS Apple Silicon:

```bash
CLEARFRAME_ORT_LIBRARY="$PWD/resources/runtime/libonnxruntime.dylib" npm run desktop:dev
```

Windows x64（PowerShell）:

```powershell
$env:CLEARFRAME_ORT_LIBRARY = (Resolve-Path resources/runtime/onnxruntime.dll).Path
npm run desktop:dev
```

アプリは不足するモデルをダウンロードし、使用前にバイト数と SHA-256 を検証します。必要なモデルのキャッシュ完了後はオフラインで処理できます。モデルはインストーラーにも Git にも含めません。固定 FP32 パイプラインは IS-Net、MobileSAM、ViTMatte を使用し、CPU への切り替えでも同じ重みと入力サイズを維持します。

UI のプレビューには `npm run dev` を実行し、`http://localhost:1420` を開きます。ブラウザーではネイティブ IPC や画像処理を利用できません。

## コマンド

| コマンド | 用途 |
| --- | --- |
| `npm run check` | TypeScript 検査と Vite 本番ビルド |
| `npm run test:contracts` | ネイティブコマンド登録と権限の検査 |
| `npm run test:i18n` | 翻訳リソースの検査 |
| `npm run test:shared` | フロントエンド共通ロジックのテスト |
| `npm run test:installer` | インストーラーツールのテスト |
| `npm run baseline:verify` | 固定画像と参照ハッシュの検証。Python 3 が必要 |
| `cargo check --workspace` | Rust ワークスペースの検査 |
| `cargo test --workspace` | Rust テスト。モデル/GPU 検証には追加準備が必要 |
| `cargo fmt --all -- --check` | Rust の書式検査 |
| `npm run desktop:build` | インストーラーを含まないアプリのビルド |
| `npm run desktop:installer` | 対象 OS 上でインストーラーをビルド |

インストーラーの対象は macOS 14+ Apple Silicon と Windows 11 x64 です。macOS の DMG 作成には Python 3.10+ も必要です。成果物と SHA-256 ファイルは `target/release/bundle/` に出力されます。macOS のローカルビルドは既定で ad-hoc 署名を使用します。一般配布には正式な認証情報と受け入れ検証が必要です。[配布ガイド](docs/21-安装器与分发.md)を参照してください。

## リポジトリ構成

```text
src/                 React UI、翻訳、共通ユーティリティ、ネイティブ API 境界
src-tauri/           Tauri コマンド、ウィンドウ、メニュー、権限、配布用ソース
crates/cutout-core/  Tauri に依存しない画像・モデル・編集・タスク処理
crates/cutout-cli/   ヘッドレス処理と検証用 CLI
resources/           リソース定義。生成したランタイムは Git 対象外
tests/               契約、翻訳、共通ロジック、配布ツール、画像の基準データ
tools/               ランタイム準備、パッケージ作成、各 OS の検証ツール
docs/                設計、移行、検証記録（中国語）
```

## コントリビューション

[AGENTS.md](AGENTS.md) のモジュール境界、画質要件、検証規則を確認してください。英語・中国語・日本語の README は同期して更新します。本リポジトリは隣接する `browser-cutout` とは独立しています。ロックファイルと出典を記載した合成テスト画像をコミットし、モデル重み、ユーザー画像、書き出し結果、認証情報、生成バイナリは含めないでください。モデルとランタイムのライセンスは[リソースガイド](resources/README.md)に記載しています。
