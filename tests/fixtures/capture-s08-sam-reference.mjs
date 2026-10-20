// Freeze MobileSAM outputs for wide/tall images on the existing Chromium Canvas pixels.
// Only the reference capture uses ORT Web; desktop inference stays in Rust.
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { gunzipSync, gzipSync } from 'node:zlib';
import * as ort from 'onnxruntime-web/wasm';

const fixtureRoot = new URL('./', import.meta.url);
const outputRoot = new URL('./reference/model-s08/', import.meta.url);
const modelRoot = new URL('../../test-output/models/', import.meta.url);
const snapshotBytes = readFileSync(new URL('model-snapshot.json', fixtureRoot));
const snapshot = JSON.parse(snapshotBytes);
const canvasBytes = readFileSync(new URL('reference/browser-canvas.json', fixtureRoot));
const canvas = JSON.parse(canvasBytes);
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const model = (id, filename) => {
  const spec = snapshot.models.find((entry) => entry.id === id);
  const bytes = readFileSync(new URL(filename, modelRoot));
  if (!spec || bytes.length !== spec.bytes || sha256(bytes) !== spec.sha256) {
    throw new Error(`${id}: size/SHA mismatch`);
  }
  return bytes;
};
const sampleAt = (values, offset, x, y) => {
  const sx = Math.max(0, Math.min(255, x));
  const sy = Math.max(0, Math.min(255, y));
  const x0 = Math.floor(sx), y0 = Math.floor(sy);
  const x1 = Math.min(255, x0 + 1), y1 = Math.min(255, y0 + 1);
  const dx = sx - x0, dy = sy - y0;
  return (values[offset + y0 * 256 + x0] * (1 - dx) + values[offset + y0 * 256 + x1] * dx) * (1 - dy)
    + (values[offset + y1 * 256 + x0] * (1 - dx) + values[offset + y1 * 256 + x1] * dx) * dy;
};

ort.env.wasm.numThreads = 1;
ort.env.wasm.wasmBinary = readFileSync(new URL('../../node_modules/onnxruntime-web/dist/ort-wasm-simd-threaded.wasm', import.meta.url));
const encoder = await ort.InferenceSession.create(model('mobilesam-image-encoder', 'mobilesam-image-encoder.onnx'), { executionProviders: ['wasm'] });
const decoder = await ort.InferenceSession.create(model('mobilesam-mask-decoder', 'mobilesam-mask-decoder.onnx'), { executionProviders: ['wasm'] });
const results = [];
const pending = new Map();
for (const name of ['wide-alpha.png', 'portrait-strands.png']) {
  const sample = canvas.samples.find((entry) => entry.input === name);
  if (!sample) throw new Error(`Missing Chromium Canvas sample: ${name}`);
  const raw = gunzipSync(readFileSync(new URL(`reference/${name}.tensor.f32le.gz`, fixtureRoot)));
  if (sha256(raw) !== sample.tensorSha256) throw new Error(`${name}: Canvas tensor changed`);
  const source = new Float32Array(raw.buffer.slice(raw.byteOffset, raw.byteOffset + raw.byteLength));
  const width = sample.fittedWidth, height = sample.fittedHeight;
  const plane = 1024 * 1024;
  const input = new Float32Array(width * height * 3);
  const means = [0.485, 0.456, 0.406];
  for (let y = 0; y < height; y++) for (let x = 0; x < width; x++) {
    for (let c = 0; c < 3; c++) {
      input[(y * width + x) * 3 + c] = Math.round((source[c * plane + y * 1024 + x] + means[c]) * 255);
    }
  }
  const embedding = (await encoder.run({ input_image: new ort.Tensor('float32', input, [height, width, 3]) }, ['image_embeddings'])).image_embeddings;
  if (embedding.dims.join(',') !== '1,256,64,64' || !embedding.data.every(Number.isFinite)) {
    throw new Error(`${name}: invalid embedding`);
  }
  const points = [{ x: 0.5, y: 0.5, label: 1 }];
  if (name === 'portrait-strands.png') points.push({ x: 0.05, y: 0.05, label: 0 });
  const coords = new Float32Array((points.length + 1) * 2);
  const labels = new Float32Array(points.length + 1);
  points.forEach((point, index) => {
    coords[index * 2] = point.x * width;
    coords[index * 2 + 1] = point.y * height;
    labels[index] = point.label;
  });
  labels[points.length] = -1;
  const outputs = await decoder.run({
    image_embeddings: embedding,
    point_coords: new ort.Tensor('float32', coords, [1, labels.length, 2]),
    point_labels: new ort.Tensor('float32', labels, [1, labels.length]),
    mask_input: new ort.Tensor('float32', new Float32Array(256 * 256), [1, 1, 256, 256]),
    has_mask_input: new ort.Tensor('float32', new Float32Array(1), [1]),
    orig_im_size: new ort.Tensor('float32', new Float32Array([sample.height, sample.width]), [2]),
  }, ['low_res_masks', 'iou_predictions']);
  const masks = outputs.low_res_masks, scores = outputs.iou_predictions;
  if (masks.dims.join(',') !== '1,4,256,256' || scores.dims.join(',') !== '1,4') {
    throw new Error(`${name}: invalid decoder shape`);
  }
  let best = -1, bestScore = -Infinity;
  for (let candidate = 0; candidate < 4; candidate++) {
    const consistent = points.every((point) =>
      (sampleAt(masks.data, candidate * 256 * 256,
        point.x * width / 4 - 0.5, point.y * height / 4 - 0.5) > 0) === (point.label === 1));
    if (consistent && scores.data[candidate] > bestScore) {
      best = candidate;
      bestScore = scores.data[candidate];
    }
  }
  if (best < 0) throw new Error(`${name}: point conflict`);
  const mask = new Uint8Array(sample.width * sample.height);
  for (let y = 0; y < sample.height; y++) for (let x = 0; x < sample.width; x++) {
    mask[y * sample.width + x] = sampleAt(masks.data, best * 256 * 256,
      (x + 0.5) * width / sample.width / 4 - 0.5,
      (y + 0.5) * height / sample.height / 4 - 0.5) > 0 ? 255 : 0;
  }
  pending.set(`${name}.mask.u8.gz`, gzipSync(mask, { level: 9 }));
  results.push({ input: name, width: sample.width, height: sample.height,
    fittedWidth: width, fittedHeight: height, points,
    canvasTensorSha256: sample.tensorSha256, samInputSha256: sha256(Buffer.from(input.buffer)),
    selectedCandidate: best, scores: Array.from(scores.data), maskSha256: sha256(mask) });
  embedding.dispose(); masks.dispose(); scores.dispose();
}
await encoder.release();
await decoder.release();
const reference = { schemaVersion: 1, kind: 'ORT Web WASM CPU on frozen Chromium Canvas RGB',
  browserSourceRevision: canvas.sourceRevision, canvasManifestSha256: sha256(canvasBytes),
  modelSnapshotSha256: sha256(snapshotBytes), ortWebVersion: ort.env.versions.web,
  backend: 'wasm', samples: results };
const path = new URL('sam-cross-size.json', outputRoot);
if (existsSync(path) && !process.argv.includes('--refresh')) {
  if (JSON.stringify(JSON.parse(readFileSync(path))) !== JSON.stringify(reference)) {
    throw new Error('Frozen S08 reference changed');
  }
  for (const [name, bytes] of pending) {
    if (sha256(gunzipSync(readFileSync(new URL(name, outputRoot)))) !== sha256(gunzipSync(bytes))) {
      throw new Error(`${name}: frozen mask changed`);
    }
  }
} else {
  mkdirSync(outputRoot, { recursive: true });
  writeFileSync(path, `${JSON.stringify(reference, null, 2)}\n`);
  for (const [name, bytes] of pending) writeFileSync(new URL(name, outputRoot), bytes);
}
console.log(JSON.stringify(reference, null, 2));
