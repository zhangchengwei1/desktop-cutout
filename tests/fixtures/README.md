# M0 固定样本与参考输出

2026-09-24 建立首版 S01 基线；2026-09-25 先补上遗漏的标准差，随后发现固定 IS-Net 清单显式使用 `[1,1,1]`，再次修正并重采集全部七组张量、真实 IS-Net 蒙版与成品。十张输入均由本项目的 [`generate.py`](generate.py) 创建并以 CC0-1.0 发布，没有用户图片。七张可解码样本已有 Chromium Canvas 输入张量、确定性合成蒙版和 PNG 输出；同七张还有固定 IS-Net 权重经 ORT Web 1.29.0 **WASM CPU** 推理的输出哈希、恢复蒙版，以及 Chromium Canvas 导出的透明 PNG。损坏文件和超限尺寸用于错误路径，不应产生成功结果。[S02 对照记录](../../docs/08-S02-图像质量对照.md)列出尚未通过的 Rust 缩放质量门槛。

## 样本与来源

| 样本 | 用途 |
| --- | --- |
| `hard-edge-opaque.png` | 彩色纹理、硬边主体、JPEG 原始画面 |
| `wide-alpha.png` | 极宽比例、透明像素中隐藏的 RGB、半透明边界 |
| `portrait-strands.png` | 极高比例、细线和半透明边界；是几何代理样本，不等于真人发丝 |
| `low-contrast.png` | 低对比度主体与背景 |
| `gamma-tagged.png` | 非标准 `gAMA` 色彩元数据 |
| `icc-tagged.png` | 自生成的简化 RGB ICC 配置文件，用于格式路径，不作为标准色卡 |
| `exif-orient6.jpg` | 非对称画面和 EXIF 方向 6；浏览器解码尺寸为 64×96 |
| `mask-edges.png` | CLI 手工蒙版和 Alpha 合成输入 |
| `corrupt-truncated.png` | 截断文件拒绝路径 |
| `oversize-header.png` | 宽 20,001 像素，超过当前单边预算 |

尺寸、格式、用途、文件 SHA-256 和许可在 [`manifest.json`](manifest.json) 中冻结。PNG 由 Python 标准库编码；JPEG 使用 FFmpeg 9.0.1 生成后由脚本写入 EXIF APP1，重新生成时 FFmpeg 版本可能改变压缩字节，因此以提交的文件哈希为权威。模型 revision、大小、SHA-256 和许可线索在 [`model-snapshot.json`](model-snapshot.json) 中；四个权重均不提交。

参考的浏览器源为仓库提交 `3c8a1d00ee01b355f65d7c584eb8c4d2101c32eb`：`u2net.ts` 文件 SHA-256 为 `0accacc96c1c700dbb4a9b827e28cd8e3979d9a62c3543c09744d395615b544f`，`model-manifest.json` 为 `0aab35d9e4f8ba9bd81ce4b5bbf8cf0fb1a41a18191592ef64a3cb4df98c6d76`。采集脚本在桌面项目内复现这份源代码的 Canvas 与数值规则；浏览器版项目未作修改，也没有运行时源码依赖。

## 参考文件的含义

- [`reference/browser-canvas.json`](reference/browser-canvas.json) 记录 Chromium 153、1024² CHW 输入张量、contain 尺寸、合成蒙版及输出像素哈希。完整张量是 `*.tensor.f32le.gz`，恢复蒙版是 `*.mask.u8.gz`，`*.synthetic.png` 用固定梯度模拟模型输出，仅用于预/后处理对照。
- [`reference/model-isnet.json`](reference/model-isnet.json) 记录固定 IS-Net 权重在 ORT Web 1.29.0 WASM CPU 上的原始模型输出、归一化蒙版和恢复蒙版 SHA-256；恢复蒙版保存在 `reference/model-isnet/*.mask.u8.gz`。原始 32 位浮点输出约 11 MiB 压缩后存于忽略的 `test-output/fixtures/isnet-output/`，可由脚本和已校验模型重建。
- [`reference/model-isnet/browser-render.json`](reference/model-isnet/browser-render.json) 与同目录 `*.model.png` 是将真实模型蒙版送入 Chromium Canvas 后得到的输出。**模型执行发生在 Node/WASM CPU；浏览器负责解码、预处理和最终 Canvas 渲染。** 这不代表网站 WebGPU/WebGL 的输出或桌面原生 ORT 已验证。
- [`reference/model-s04/`](reference/model-s04/) 固定 96×64 合成图的 Chromium Canvas MobileSAM/ViTMatte 输入、trimap 和 ORT Web 1.29.0 WASM CPU 蒙版。原始权重仍只在忽略的 `test-output/models/`；`reference_s04.rs` 可测量 Rust 输入差异，显式设置 `S04_NATIVE_RUNTIME` 时做本机原生 CPU 输出对照。当前点选差异和适用范围见 [S04 验证记录](../../docs/10-S04-点选与精修原生验证.md)。
- [`reference/model-s08/`](reference/model-s08/) 从已冻结的 Chromium Canvas 张量还原 160×48 与 48×160 图片的 MobileSAM RGB 输入，固定同一权重在 ORT Web 1.29.0 WASM CPU 的主体点蒙版。`npm run baseline:s08:sam` 可复核参考；设置 `S04_NATIVE_RUNTIME` 后，`reference_s08.rs` 比较原生 ORT 对同一张量的逐像素输出，并测量 Rust 实际图片预处理带来的最终蒙版差异。它复用 S01 的浏览器像素，不是新的浏览器窗口采集。
- 浏览器内存 RGBA 与 `canvas.toBlob()` 编码 PNG 再解码后的 RGBA 哈希可能不同，尤其在透明像素处。导出质量对照使用参考 JSON 中的 `exportedPngRgbaSha256` 及解码后的像素，而不是比较 PNG 文件字节。

首版采集主机为 macOS 27.0 arm64，Node 24.10.0，Chromium Headless 153。首版 IS-Net WASM 单会话模型加载约 0.83 秒，七次单图推理分别约 26.2–27.2 秒；2026-09-25 重采集单图约 6.3–7.5 秒。耗时只记录本机参考，不能外推到桌面 ORT、其他模型或 Windows。性能报告另存忽略目录。

## 复现与验证

在 `desktop-cutout/` 执行：

```bash
python3 tests/fixtures/generate.py
npm run baseline:verify
cargo build -q -p cutout-cli
python3 tests/fixtures/check-cli.py
```

重新采集浏览器输入参考需要 Chromium、`playwright-cli` 和本机可用端口 8766：

```bash
bash tests/fixtures/capture-browser-baseline.sh
```

然后按 [`model-snapshot.json`](model-snapshot.json) 中固定的 `downloadUrl` 获取 IS-Net 权重，放到 `test-output/models/isnet-auto-high.onnx`，先检查其大小和 SHA-256；权重不加入版本控制。下列命令重新运行真实模型并渲染结果：

```bash
cargo run -p cutout-cli -- verify-model isnet-auto-high test-output/models/isnet-auto-high.onnx
npm run baseline:isnet -- --refresh  # 仅在明确修正参考后更新已有哈希
bash tests/fixtures/capture-model-render.sh
npm run baseline:verify
```

`capture-*.sh` 默认调用 PATH 中的 `playwright-cli`，也可设置 `PWCLI_BIN` 指向已安装的可执行文件。模型参考脚本对已有哈希不自动更新；若模型或算法变化，先记录原因和新的源提交，再审查差异、明确建立新基线。自然发丝、复杂背景商品和真实摄影图片仍需补充明确授权样本，作为 M0 画质验收的一部分。

## 验收规则与报告

[`acceptance.json`](acceptance.json) 冻结第一轮尺寸、padding、张量、模型输出、蒙版和透明 PNG 的数值门槛。坐标、尺寸和归一化零填充要求精确；张量、CPU 浮点、模型蒙版和导出像素分别统计均值、P99 与最大偏差。透明低 Alpha 区的 RGB 以预乘值比较，并结合边缘放大图人工检查，避免透明像素的隐藏 RGB 影响视觉判断。

每次原生对照按 [`report-template.json`](report-template.json) 记录源/桌面提交、样本与模型 SHA、平台、实际执行后端、输入形状、阶段耗时、峰值内存、误差及人工复核。失败后先定位解码、缩放、推理或编码阶段；阈值不因结果失败自动放宽。M0 后续还须在 macOS 原生 ORT 与 Windows 真机分别采集 CPU/候选 GPU 证据，不能把本页的 Chromium/WASM 参考写成双平台通过。

生成的中间大文件、临时报告、模型权重、用户私有图片和性能实验结果只放 `test-output/`；固定的小型输入与参考输出留在本目录。

S10 的 [`batch-100-synthetic.json`](batch-100-synthetic.json) 把七张固定图重复成 100 项，用 [`run-batch-benchmark.py`](run-batch-benchmark.py) 驱动生产 Rust 批量管线并记录 macOS/Linux 子进程峰值 RSS。复现步骤和结果见 [百项原生模型基线](../../docs/17-S10-百项原生模型基线.md)。它用于吞吐与重复结果一致性探针，不是 100 张独立摄影样本的验收。

S11 的 [`run-concurrency-probe.py`](run-concurrency-probe.py) 用同一计划比较一个进程处理 100 项与两个独立进程各处理 50 项，核对全部 PNG 字节并记录墙钟、各进程峰值 RSS 与 CPU 时间；可反转运行顺序。结果和局限见 [双进程并发探针](../../docs/18-S11-双进程并发探针.md)。它不等于桌面工作区双槽实现或双平台验收。

[`generate-large-batch.py`](generate-large-batch.py) 在指定的忽略目录生成 100 张互异、高熵的 RGB PNG、RGBA PNG 与 JPEG 负载样本，以及独立的计划/哈希清单。配合 `run-batch-benchmark.py --zip` 或 `--zip-during`，可在处理后或处理期间用生产 Rust 工作区流式导出 ZIP，并逐条对照已提交 PNG；数据与边界见 [百张大图与 ZIP 负载探针](../../docs/19-S10-百张大图与ZIP负载.md)。此生成集只测负载，不作为真实摄影画质样本。
