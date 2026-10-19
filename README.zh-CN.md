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

| 命令 | 用途 |
| --- | --- |
| `npm run check` | TypeScript 检查与 Vite 生产构建 |
| `npm run test:contracts` | 核对原生命令注册与权限 |
| `npm run test:i18n` | 检查翻译资源 |
| `npm run test:shared` | 前端共享逻辑测试 |
| `npm run test:installer` | 安装器工具测试 |
| `npm run baseline:verify` | 校验固定图片与参考哈希，需要 Python 3 |
| `cargo check --workspace` | 检查 Rust 工作区 |
| `cargo test --workspace` | Rust 测试，模型/GPU 探针需额外准备 |
| `cargo fmt --all -- --check` | 检查 Rust 格式 |
| `npm run desktop:build` | 构建原生应用，不生成安装器 |
| `npm run desktop:installer` | 在目标系统构建安装器 |

安装器目标为 macOS 14+ Apple Silicon 和 Windows 11 x64。macOS DMG 打包另需 Python 3.10+；产物和 SHA-256 文件位于 `target/release/bundle/`。macOS 本地构建默认使用 ad-hoc 签名，公开分发仍需正式凭据与发布验收，见[安装器指南](docs/21-安装器与分发.md)。

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

阅读 [AGENTS.md](AGENTS.md)，遵循模块边界、质量契约和验证规则。同步维护英、中、日三份 README。本仓库与同级 `browser-cutout` 独立维护。提交锁文件和有来源说明的合成测试样本；不要提交模型权重、用户图片、导出结果、凭据或生成的二进制。模型与运行库许可说明见[资源指南](resources/README.md)。
