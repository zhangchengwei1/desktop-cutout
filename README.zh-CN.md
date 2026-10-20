# ClearFrame Desktop

[English](README.md) · **简体中文** · [日本語](README.ja.md)

面向 macOS 与 Windows 的本地抠图桌面应用，使用 React、TypeScript、Tauri 2 和独立 Rust 处理核心。原图、蒙版和结果保留在本机，界面支持英语、简体中文和日语。

## 开发状态

开发版已包含自动抠图、点选、精修、画笔修补、背景、撤销/重做、PNG 导出、文件夹/ZIP 批量处理、模型管理、可选会话恢复和系统右键菜单集成。

项目仍在开发中。macOS Apple Silicon 已有原生运行和本地 DMG 验证记录；Windows 11 x64 的构建、安装和 DirectML 加速仍需真机验证。图像质量对齐、大规模负载、发布签名、公证和干净系统验收尚未全部完成，参见[开发清单](docs/07-桌面端迁移开发指南.md)。

## 开始开发

安装 Node.js **22.12+**、npm、Rust 和 Tauri 平台构建依赖。macOS 需要 Xcode Command Line Tools；Windows 需要 MSVC C++ Build Tools、Windows SDK、CMake 和 WebView2。

在仓库根目录执行：

```bash
npm ci
npm run installer:prepare
```

资源准备命令下载并校验当前平台的 ONNX Runtime，写入 `resources/runtime/`。开发启动时指定运行库的绝对路径：

macOS Apple Silicon：

```bash
CLEARFRAME_ORT_LIBRARY="$PWD/resources/runtime/libonnxruntime.dylib" npm run desktop:dev
```

Windows x64（PowerShell）：

```powershell
$env:CLEARFRAME_ORT_LIBRARY = (Resolve-Path resources/runtime/onnxruntime.dll).Path
npm run desktop:dev
```

应用下载缺失模型，并在使用前校验字节数与 SHA-256。所需模型缓存完成后可以离线处理；模型不包含在安装包中，也不提交 Git。固定 FP32 管线使用 IS-Net、MobileSAM 和 ViTMatte，CPU 回退保持相同权重和输入尺寸。

仅预览界面可运行 `npm run dev`，访问 `http://localhost:1420`。浏览器预览不具备原生 IPC 或图像处理能力。

## 常用命令

以下列出 [`package.json`](package.json) 的全部 19 个脚本，均在仓库根目录执行。`build` 与 `preview` 仅处理前端，原生图像处理使用桌面命令。

### 前端开发

| 命令 | 用途 |
| --- | --- |
| `npm run dev` | 启动 Vite 开发预览：`http://localhost:1420`，该端口必须可用 |
| `npm run typecheck` | 执行 `tsc --noEmit`，不生成构建产物 |
| `npm run build` | 构建前端到 `dist/`，不执行类型检查 |
| `npm run preview` | 本地预览已有前端产物；先执行 `build`，访问 Vite 输出的地址 |
| `npm run check` | TypeScript 检查与 Vite 生产构建 |

### 原生开发与打包

| 命令 | 用途 |
| --- | --- |
| `npm run tauri` | 调用本地 Tauri CLI；通过 `--` 传参，如 `npm run tauri -- --help` |
| `npm run desktop:dev` | 启动原生应用和 Vite 开发服务；需要平台工具链，推理另需运行库 |
| `npm run desktop:build` | 检查类型并构建原生应用，不生成安装器 |
| `npm run installer:prepare` | 下载、校验并准备原生运行库、许可通知及 Windows CRT 到 `resources/runtime/` |
| `npm run desktop:installer` | 准备资源、构建应用、检查随包运行库加载，生成 DMG 或 NSIS 安装器与 SHA-256 |

### 测试与图像参考

| 命令 | 用途 |
| --- | --- |
| `npm run test:contracts` | 核对原生命令注册与权限 |
| `npm run test:i18n` | 检查翻译资源 |
| `npm run test:shared` | 前端共享逻辑测试 |
| `npm run test:installer` | 安装器工具测试 |
| `npm run test:gpu:windows` | Windows 专用：DirectML 画质、执行后端、错误恢复与 CPU 回退验证；必须传入 `-ModelDirectory` |
| `npm run baseline:verify` | 校验固定图片与参考哈希，需要 Python 3 |
| `npm run baseline:isnet` | 基于冻结的 Canvas 张量执行 ORT Web/WASM CPU IS-Net 参考，需要本地 IS-Net 权重 |
| `npm run baseline:s04:model` | 校验或采集 S04 固定样本的 MobileSAM / ViTMatte 参考，需要对应三个权重 |
| `npm run baseline:s08:sam` | 校验或采集宽图/竖图的 MobileSAM 参考，需要编码器与解码器权重 |

以下 Rust 工作区命令不属于 npm 脚本：

| 命令 | 用途 |
| --- | --- |
| `cargo check --workspace` | 检查 Rust 工作区 |
| `cargo test --workspace` | Rust 测试，模型/GPU 探针需额外准备 |
| `cargo fmt --all -- --check` | 检查 Rust 格式 |

## 验证流程

前端或原生调用边界变更，按范围运行以下检查：

```bash
npm run check
npm run test:contracts
npm run test:i18n
npm run test:shared
npm run test:installer
```

Rust 变更另需执行上方三个工作区命令。图像管线变更需要固定样本对照，不能只看构建是否成功。原生 IPC、文件对话框、菜单和权限须在 Tauri 窗口验证；浏览器预览或单元测试通过不能证明原生能力或 GPU 就绪。

### 模型参考的准备条件

三个模型参考脚本在 Node 中使用 ORT Web/WASM CPU，与生产原生推理分开。按 [`model-snapshot.json`](tests/fixtures/model-snapshot.json) 获取精确权重，核对字节数与 SHA-256，再放入忽略目录：

```text
test-output/models/
├─ isnet-auto-high.onnx
├─ mobilesam-image-encoder.onnx
├─ mobilesam-mask-decoder.onnx
└─ vitmatte-high.onnx
```

脚本复用 `tests/fixtures/reference/` 已冻结的 Canvas 输入，不会下载权重。已有参考不匹配时默认失败；传入 `-- --refresh` 会明确替换参考元数据/蒙版，使用前须审查并记录基线变化，完成后执行 `npm run baseline:verify`。浏览器输入采集、样本生成和数值门槛见[固定样本指南](tests/fixtures/README.md)。

### Windows GPU 验证

在具有 DirectX 12 GPU 的 Windows x64 设备上，准备运行库并通过应用下载所需模型，然后在 PowerShell 执行：

```powershell
npm run installer:prepare
npm run test:gpu:windows -- -ModelDirectory "$env:LOCALAPPDATA\dev.silta.clearframe.desktop\models"
```

可追加 `-Image "C:\Images\example.png"`，否则使用固定合成样本。日志、画质报告和后端 profile 输出至 `test-output/acceleration-windows/`。profile 必须包含真实 DirectML 执行，仅 CPU 回退不能通过 GPU 验证。准备条件与待验项目见 [Windows 验证指南](docs/29-Windows-GPU开发与真机验证.md)。

## 无窗口 CLI

`cutout-cli` 复用同一 Rust 核心，不打开窗口：

```bash
cargo run -p cutout-cli -- --help
cargo run -p cutout-cli -- info
cargo run -p cutout-cli -- inspect tests/fixtures/images/hard-edge-opaque.png
cargo run -p cutout-cli -- cutout --help
```

其他子命令包括 `apply-mask`、`verify-model`、`infer-tensor`、`select`、`refine`、`batch-bench`。推理命令需显式传入运行库和模型路径，位置参数以各子命令的 `--help` 为准。默认 `info` 输出是核心元数据，不代表桌面模型会话已就绪。

## 配置与本地数据

在启动命令的终端中设置环境变量；原生工具不会自动加载 `.env` 文件。

| 变量 | 用途 |
| --- | --- |
| `CLEARFRAME_ORT_LIBRARY` | debug 应用使用的原生运行库绝对路径 |
| `CLEARFRAME_MODEL_DIR` | 覆盖 debug 应用的模型缓存目录 |
| `CLEARFRAME_ORT_ARCHIVE` | 准备资源时使用本地固定 ORT 归档，仍校验大小与哈希 |
| `CLEARFRAME_DIRECTML_ARCHIVE` | Windows 准备资源时使用本地固定 DirectML 依赖归档 |
| `CLEARFRAME_MSVC_REDIST_DIR` | 指定 Windows x64 MSVC CRT 来源目录 |
| `CLEARFRAME_PYTHON` | 指定 macOS DMG 打包使用的 Python 3.10+ 解释器 |
| `APPLE_SIGNING_IDENTITY` | 指定 macOS 安装器构建的签名身份 |

release 应用使用随包运行库与应用数据目录，不采用 debug 专用的运行库/模型覆盖项。默认数据位置：

- macOS：`~/Library/Application Support/dev.silta.clearframe.desktop/`
- Windows：`%LOCALAPPDATA%\dev.silta.clearframe.desktop\`

模型、偏好和可选恢复数据均由本机管理，卸载应用不会自动删除。清理时使用应用资源/设置入口，用户导出目录单独管理。模型来源、revision、哈希和许可链接定义在 [Rust 模型清单](crates/cutout-core/src/model.rs)，运行库归档固定在[运行库清单](tools/runtime-manifest.json)。

## 构建安装器

安装器目标为 macOS 14+ Apple Silicon 和 Windows 11 x64，暂不提供 Intel Mac 与 Windows ARM64 安装器。在目标系统执行：

```bash
npm ci
npm run desktop:installer
```

该命令自动准备资源。macOS DMG 打包另需 Python 3.10+，隔离工具环境位于 `target/`；Windows 需要 x64 MSVC 可再分发文件，打包器会获取 NSIS 与 WebView2 离线安装器。首次构建需要联网，除非所需下载已准备完成。

| 平台 | 安装器产物，同目录生成 `.sha256` |
| --- | --- |
| macOS | `target/release/bundle/dmg/ClearFrame Desktop_<version>_aarch64.dmg` |
| Windows | `target/release/bundle/nsis/ClearFrame Desktop_<version>_x64-setup.exe` |

打包前取消 `CARGO_BUILD_TARGET` 和 `CARGO_TARGET_DIR`，安装器脚本拒绝目标覆盖与额外参数。macOS 本地构建默认使用 ad-hoc 签名，公开分发仍需 Developer ID/公证或 Windows Authenticode 配置及发布验收。随包运行库加载检查不能代替模型画质或 GPU 验证，见[安装器指南](docs/21-安装器与分发.md)。

## 常见问题

| 现象 | 处理方式 |
| --- | --- |
| Vite 无法使用端口 1420 | 停止占用进程；开发服务使用固定端口 |
| 界面已打开但不能处理 | 使用 `desktop:dev`，核对运行库路径及模型下载/会话错误 |
| 模型参考脚本提示缺少文件 | 按上方文件名准备精确权重；应用缓存与参考脚本目录不同 |
| 基线哈希变化 | 先定位输入、模型或算法差异，再明确刷新参考 |
| 运行库准备失败 | 检查归档完整性、平台与 CRT 条件，保留哈希校验 |

## 仓库结构

```text
src/                 React 界面、翻译、共享工具与原生调用边界
src-tauri/           Tauri 命令、窗口、菜单、权限与安装器源文件
crates/cutout-core/  不依赖 Tauri 的图像、模型、编辑与任务逻辑
crates/cutout-cli/   无窗口处理与验证 CLI
resources/           资源清单；生成的运行库文件已忽略
tests/               契约、翻译、共享逻辑、安装器与图像基线
tools/               运行库准备、打包与平台验证工具
docs/                架构、迁移与验证记录（中文）
```

## 参与贡献

阅读 [AGENTS.md](AGENTS.md)，遵循模块边界、质量契约和验证规则。同步维护英、中、日三份 README。本仓库与同级 `browser-cutout` 独立维护。提交锁文件和有来源说明的合成测试样本；不要提交模型权重、用户图片、导出结果、凭据或生成的二进制。提交消息使用简洁英语，PR 写明验证结果与尚未验证的平台限制。
