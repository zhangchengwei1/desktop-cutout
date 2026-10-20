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

[`package.json`](package.json) の全 19 スクリプトを以下に示します。リポジトリのルートで実行してください。`build` と `preview` はフロントエンド用です。ネイティブ画像処理にはデスクトップ用コマンドを使用します。

### フロントエンド開発

| コマンド | 用途 |
| --- | --- |
| `npm run dev` | `http://localhost:1420` で Vite 開発プレビューを起動。このポートを空けてください |
| `npm run typecheck` | `tsc --noEmit` を実行。ビルド成果物は生成しません |
| `npm run build` | フロントエンドを `dist/` にビルド。型検査は実行しません |
| `npm run preview` | 既存のビルドをローカル配信。先に `build` を実行し、Vite が表示する URL を開きます |
| `npm run check` | TypeScript 検査と Vite 本番ビルド |

### ネイティブ開発とパッケージ作成

| コマンド | 用途 |
| --- | --- |
| `npm run tauri` | ローカルの Tauri CLI を呼び出します。引数例: `npm run tauri -- --help` |
| `npm run desktop:dev` | ネイティブアプリと Vite を起動。各 OS のツールが必要で、推論にはランタイムも必要です |
| `npm run desktop:build` | 型検査後、インストーラーなしでネイティブアプリをビルド |
| `npm run installer:prepare` | ランタイム、ライセンス通知、Windows CRT をダウンロード・検証し `resources/runtime/` に配置 |
| `npm run desktop:installer` | リソース準備、アプリ構築、同梱ランタイムの読み込み検査後、DMG または NSIS と SHA-256 を生成 |

### テストと画像参照データ

| コマンド | 用途 |
| --- | --- |
| `npm run test:contracts` | ネイティブコマンド登録と権限の検査 |
| `npm run test:i18n` | 翻訳リソースの検査 |
| `npm run test:shared` | フロントエンド共通ロジックのテスト |
| `npm run test:installer` | インストーラーツールのテスト |
| `npm run test:gpu:windows` | Windows 専用。DirectML の画質、実行プロバイダー、エラー復旧、CPU 切り替えを検査。`-ModelDirectory` 必須 |
| `npm run baseline:verify` | 固定画像と参照ハッシュの検証。Python 3 が必要 |
| `npm run baseline:isnet` | 固定 Canvas テンソルから ORT Web/WASM CPU で IS-Net 参照を実行。ローカルの重みが必要 |
| `npm run baseline:s04:model` | S04 固定サンプルの MobileSAM / ViTMatte 参照を検証または採取。対応する 3 つの重みが必要 |
| `npm run baseline:s08:sam` | 横長・縦長画像の MobileSAM 参照を検証または採取。エンコーダーとデコーダーの重みが必要 |

以下の Rust ワークスペース用コマンドは npm スクリプトとは別です。

| コマンド | 用途 |
| --- | --- |
| `cargo check --workspace` | Rust ワークスペースの検査 |
| `cargo test --workspace` | Rust テスト。モデル/GPU 検証には追加準備が必要 |
| `cargo fmt --all -- --check` | Rust の書式検査 |

## 検証の進め方

フロントエンドやネイティブ API 境界を変更した場合は、関連する検査を実行します。

```bash
npm run check
npm run test:contracts
npm run test:i18n
npm run test:shared
npm run test:installer
```

Rust の変更では、上記の 3 つのワークスペースコマンドも実行してください。画像処理パイプラインの変更には固定サンプルとの比較が必要です。ネイティブ IPC、ファイルダイアログ、メニュー、権限は Tauri ウィンドウで検証します。ブラウザープレビューや単体テストの成功だけでは、ネイティブ動作や GPU の準備完了を示せません。

### モデル参照スクリプトの準備

3 つのモデル参照スクリプトは Node 上の ORT Web/WASM CPU を使用し、本番のネイティブ推論とは別です。[`model-snapshot.json`](tests/fixtures/model-snapshot.json) に指定された重みを取得し、バイト数と SHA-256 を確認して Git 対象外のディレクトリに配置します。

```text
test-output/models/
├─ isnet-auto-high.onnx
├─ mobilesam-image-encoder.onnx
├─ mobilesam-mask-decoder.onnx
└─ vitmatte-high.onnx
```

`tests/fixtures/reference/` の固定 Canvas 入力を再利用し、重みのダウンロードは行いません。既存の参照と一致しない場合は既定で失敗します。`-- --refresh` を渡すと参照メタデータやマスクを置き換えるため、変更を確認・記録してから使用し、その後 `npm run baseline:verify` を実行してください。ブラウザー入力の採取、画像生成、数値基準は[固定サンプルガイド](tests/fixtures/README.md)を参照してください。

### Windows GPU 検証

DirectX 12 GPU を搭載した Windows x64 でランタイムを準備し、アプリから必要なモデルをダウンロードします。その後 PowerShell で実行してください。

```powershell
npm run installer:prepare
npm run test:gpu:windows -- -ModelDirectory "$env:LOCALAPPDATA\dev.silta.clearframe.desktop\models"
```

任意で `-Image "C:\Images\example.png"` を追加できます。省略時は固定の合成サンプルを使用します。ログ、画質レポート、プロバイダーのプロファイルは `test-output/acceleration-windows/` に出力されます。プロファイルに実際の DirectML 実行が必要で、CPU への切り替えだけでは GPU 検証を通過できません。必要条件と未検証項目は [Windows 検証ガイド](docs/29-Windows-GPU开发与真机验证.md)を参照してください。

## ヘッドレス CLI

`cutout-cli` はウィンドウを開かずに同じ Rust コアを呼び出します。

```bash
cargo run -p cutout-cli -- --help
cargo run -p cutout-cli -- info
cargo run -p cutout-cli -- inspect tests/fixtures/images/hard-edge-opaque.png
cargo run -p cutout-cli -- cutout --help
```

ほかに `apply-mask`、`verify-model`、`infer-tensor`、`select`、`refine`、`batch-bench` があります。推論コマンドにはランタイムとモデルのパスを明示します。位置引数は各サブコマンドの `--help` で確認してください。既定の `info` はコアのメタデータであり、デスクトップのモデルセッション準備完了を示すものではありません。

## 設定とローカルデータ

コマンドを起動するシェルで環境変数を設定してください。ネイティブツールは `.env` ファイルを自動で読み込みません。

| 変数 | 用途 |
| --- | --- |
| `CLEARFRAME_ORT_LIBRARY` | debug アプリで使用するネイティブランタイムの絶対パス |
| `CLEARFRAME_MODEL_DIR` | debug アプリのモデルキャッシュディレクトリを指定 |
| `CLEARFRAME_ORT_ARCHIVE` | リソース準備時に固定 ORT アーカイブをローカルから使用。サイズとハッシュは検証されます |
| `CLEARFRAME_DIRECTML_ARCHIVE` | Windows の準備で固定 DirectML 依存アーカイブをローカルから使用 |
| `CLEARFRAME_MSVC_REDIST_DIR` | Windows x64 MSVC CRT の取得元ディレクトリを指定 |
| `CLEARFRAME_PYTHON` | macOS DMG 作成用の Python 3.10+ インタープリターを指定 |
| `APPLE_SIGNING_IDENTITY` | macOS インストーラービルドの署名 ID を指定 |

release アプリは同梱ランタイムとアプリデータディレクトリを使用し、debug 専用のランタイム/モデルの指定は使用しません。既定のデータ保存先:

- macOS: `~/Library/Application Support/dev.silta.clearframe.desktop/`
- Windows: `%LOCALAPPDATA%\dev.silta.clearframe.desktop\`

モデル、設定、任意の復元データは端末内で管理され、アンインストール時に自動削除されません。削除にはアプリのリソース/設定画面を使用してください。ユーザーの書き出し先は別管理です。モデルの出典、revision、ハッシュ、ライセンス URL は [Rust モデル定義](crates/cutout-core/src/model.rs)、ランタイムアーカイブは[ランタイム定義](tools/runtime-manifest.json)に固定されています。

## インストーラーのビルド

対象は macOS 14+ Apple Silicon と Windows 11 x64 です。Intel Mac と Windows ARM64 は現在の対象外です。対象 OS 上で実行します。

```bash
npm ci
npm run desktop:installer
```

リソースは自動で準備されます。macOS の DMG 作成には Python 3.10+ も必要で、分離したツール環境は `target/` に作成されます。Windows は x64 MSVC 再頒布可能ファイルが必要です。パッケージャーは NSIS と WebView2 オフラインインストーラーを取得します。必要なダウンロードが準備済みでなければ、初回ビルドにはネットワーク接続が必要です。

| OS | 成果物。同じ場所に `.sha256` も生成されます |
| --- | --- |
| macOS | `target/release/bundle/dmg/ClearFrame Desktop_<version>_aarch64.dmg` |
| Windows | `target/release/bundle/nsis/ClearFrame Desktop_<version>_x64-setup.exe` |

パッケージ作成前に `CARGO_BUILD_TARGET` と `CARGO_TARGET_DIR` を解除してください。スクリプトはターゲットの上書きと追加引数を拒否します。macOS のローカルビルドは既定で ad-hoc 署名を使用します。一般配布には Developer ID/公証または Windows Authenticode の設定と配布前検証が必要です。同梱ランタイムの読み込み検査は、モデル画質や GPU 検証の代わりにはなりません。[配布ガイド](docs/21-安装器与分发.md)を参照してください。

## トラブルシューティング

| 症状 | 対処 |
| --- | --- |
| Vite がポート 1420 で起動できない | 使用中のプロセスを停止してください。開発サーバーは固定ポートを使用します |
| UI は開くが処理できない | `desktop:dev` を使用し、ランタイムのパスとモデルのダウンロード/セッションエラーを確認 |
| モデル参照スクリプトでファイルが見つからない | 上記の正確なファイル名で重みを配置。アプリのキャッシュと参照スクリプトのディレクトリは別です |
| 基準ハッシュが変わった | 入力・モデル・アルゴリズムの差を調査してから明示的に参照を更新 |
| ランタイム準備に失敗する | アーカイブの整合性、OS、CRT の条件を確認し、ハッシュ検証は維持 |

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

[AGENTS.md](AGENTS.md) のモジュール境界、画質要件、検証規則を確認してください。英語・中国語・日本語の README は同期して更新します。本リポジトリは隣接する `browser-cutout` とは独立しています。ロックファイルと出典を記載した合成テスト画像をコミットし、モデル重み、ユーザー画像、書き出し結果、認証情報、生成バイナリは含めないでください。コミットメッセージは簡潔な英語とし、PR に検証結果と未検証の OS 制約を記載してください。
