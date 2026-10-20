// Test-only ORT Web/WASM CPU reference. Uses frozen Chromium input tensors;
// never imports browser-cutout source or changes production desktop dependencies.
import { createHash } from 'node:crypto';
import { readFileSync, mkdirSync, writeFileSync, existsSync } from 'node:fs';
import { gunzipSync, gzipSync } from 'node:zlib';
import { performance } from 'node:perf_hooks';
import * as ort from 'onnxruntime-web/wasm';

const referenceDirectory = new URL('./reference/', import.meta.url);
const outputDirectory = new URL('../../test-output/fixtures/isnet-output/', import.meta.url);
const modelPath = new URL('../../test-output/models/isnet-auto-high.onnx', import.meta.url);
const snapshot = JSON.parse(readFileSync(new URL('./model-snapshot.json', import.meta.url), 'utf8'));
const browser = JSON.parse(readFileSync(new URL('./reference/browser-canvas.json', import.meta.url), 'utf8'));
const frozenPath = new URL('./reference/model-isnet.json', import.meta.url);
const refresh = process.argv.includes('--refresh');
if (existsSync(frozenPath) && !refresh) {
  const previous = JSON.parse(readFileSync(frozenPath, 'utf8'));
  if (previous.samples.some((item, index) => item.inputTensorSha256 !== browser.samples[index]?.tensorSha256)) {
    throw new Error('Browser input tensors changed; review and rerun with --refresh');
  }
}
const spec = snapshot.models.find((item) => item.id === 'isnet-auto-high');
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const model = readFileSync(modelPath);
if (model.byteLength !== spec.bytes || hash(model) !== spec.sha256) {
  throw new Error('IS-Net weight size or SHA-256 mismatch');
}

ort.env.wasm.numThreads = 1;
ort.env.wasm.wasmBinary = readFileSync(new URL('../../node_modules/onnxruntime-web/dist/ort-wasm-simd-threaded.wasm', import.meta.url));
const started = performance.now();
const session = await ort.InferenceSession.create(model, { executionProviders: ['wasm'] });
const loadMs = performance.now() - started;
if (session.inputNames[0] !== 'input_image' || !session.outputNames.includes('output_image')) {
  throw new Error('IS-Net tensor names differ from the fixed manifest');
}
mkdirSync(outputDirectory, { recursive: true });
mkdirSync(new URL('./reference/model-isnet/', import.meta.url), { recursive: true });

const samples = [];
const masks = [];
for (const sample of browser.samples) {
  const bytes = gunzipSync(readFileSync(new URL(`./reference/${sample.input}.tensor.f32le.gz`, import.meta.url)));
  if (hash(bytes) !== sample.tensorSha256 || bytes.byteLength !== 3 * 1024 * 1024 * 4) {
    throw new Error(`${sample.input}: input tensor changed`);
  }
  const input = new Float32Array(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
  const runStarted = performance.now();
  const result = await session.run({ input_image: new ort.Tensor('float32', input, [1, 3, 1024, 1024]) }, ['output_image']);
  const runMs = performance.now() - runStarted;
  const output = result.output_image;
  if (!(output.data instanceof Float32Array) || output.data.length !== 1024 * 1024
      || output.dims.at(-1) !== 1024 || output.dims.at(-2) !== 1024
      || !output.data.every(Number.isFinite)) {
    throw new Error(`${sample.input}: invalid model output`);
  }
  const values = output.data;
  let min = Number.POSITIVE_INFINITY;
  let max = Number.NEGATIVE_INFINITY;
  for (const value of values) {
    min = Math.min(min, value);
    max = Math.max(max, value);
  }
  if (max - min < 1e-7) throw new Error(`${sample.input}: constant model output`);
  const normalized = Uint8ClampedArray.from(values, (value) => Math.round(((value - min) / (max - min)) * 255 + 1e-4));
  const restored = new Uint8ClampedArray(1024 * 1024);
  const croppedWidth = Math.max(1, Math.round(sample.fittedWidth));
  const croppedHeight = Math.max(1, Math.round(sample.fittedHeight));
  for (let y = 0; y < 1024; y += 1) {
    for (let x = 0; x < 1024; x += 1) {
      restored[y * 1024 + x] = normalized[Math.min(croppedHeight - 1, Math.floor(y * croppedHeight / 1024)) * 1024
        + Math.min(croppedWidth - 1, Math.floor(x * croppedWidth / 1024))];
    }
  }
  const outputBytes = Buffer.from(values.buffer, values.byteOffset, values.byteLength);
  writeFileSync(new URL(`${sample.input}.output.f32le.gz`, outputDirectory), gzipSync(outputBytes, { level: 9 }));
  masks.push([sample.input, gzipSync(restored, { level: 9 })]);
  samples.push({
    input: sample.input,
    inputTensorSha256: sample.tensorSha256,
    modelOutputShape: output.dims,
    modelOutputSha256: hash(outputBytes),
    modelOutputMinimum: min,
    modelOutputMaximum: max,
    normalizedMaskSha256: hash(normalized),
    restoredMaskSha256: hash(restored),
    runMs,
  });
  output.dispose();
  console.log(`${sample.input}: ${runMs.toFixed(1)} ms`);
}
await session.release();

const frozen = {
  schemaVersion: 1,
  kind: 'ORT Web WASM CPU inference on Chromium Canvas input tensors',
  browserSourceRevision: browser.sourceRevision,
  modelId: spec.id,
  modelRevision: spec.revision,
  modelSha256: spec.sha256,
  ortWebVersion: ort.env.versions.web,
  backend: 'wasm',
  samples: samples.map(({ runMs, ...sample }) => sample),
};
if (existsSync(frozenPath) && !refresh) {
  const previous = JSON.parse(readFileSync(frozenPath, 'utf8'));
  if (JSON.stringify(previous) !== JSON.stringify(frozen)) {
    throw new Error('Model reference changed; inspect before replacing frozen hashes');
  }
} else {
  writeFileSync(frozenPath, `${JSON.stringify(frozen, null, 2)}\n`);
}
for (const [name, bytes] of masks) {
  writeFileSync(new URL(`./reference/model-isnet/${name}.mask.u8.gz`, import.meta.url), bytes);
}
writeFileSync(new URL('../../test-output/fixtures/isnet-timings.json', import.meta.url),
  `${JSON.stringify({ loadMs, node: process.version, platform: process.platform, arch: process.arch, samples }, null, 2)}\n`);
