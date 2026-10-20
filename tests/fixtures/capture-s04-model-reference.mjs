// Fixed MobileSAM/ViTMatte output reference: ORT Web 1.29 WASM CPU on
// separately frozen Chromium Canvas inputs. Never imports the website runtime.
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { gunzipSync, gzipSync } from 'node:zlib';
import * as ort from 'onnxruntime-web/wasm';

const root = new URL('./reference/model-s04/', import.meta.url);
const modelRoot = new URL('../../test-output/models/', import.meta.url);
const snapshot = JSON.parse(readFileSync(new URL('./model-snapshot.json', import.meta.url), 'utf8'));
const canvas = JSON.parse(readFileSync(new URL('canvas.json', root), 'utf8'));
const points = JSON.parse(readFileSync(new URL('./sam-points.json', import.meta.url), 'utf8'));
const hash = (value) => createHash('sha256').update(value).digest('hex');
const model = (id, filename) => {
  const spec = snapshot.models.find((entry) => entry.id === id);
  if (!spec) throw new Error(`Unknown model ${id}`);
  const bytes = readFileSync(new URL(filename, modelRoot));
  if (bytes.length !== spec.bytes || hash(bytes) !== spec.sha256) throw new Error(`${id}: size/SHA mismatch`);
  return bytes;
};
const tensor = (filename, expectedHash) => {
  const bytes = gunzipSync(readFileSync(new URL(filename, root)));
  if (hash(bytes) !== expectedHash || bytes.length % 4) throw new Error(`${filename}: frozen tensor mismatch`);
  return new Float32Array(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength));
};
const pending = new Map();
const save = (name, data) => pending.set(name, gzipSync(Buffer.from(data.buffer, data.byteOffset, data.byteLength), { level: 9 }));

ort.env.wasm.numThreads = 1;
ort.env.wasm.wasmBinary = readFileSync(new URL('../../node_modules/onnxruntime-web/dist/ort-wasm-simd-threaded.wasm', import.meta.url));
const samInput = tensor('sam-input.f32le.gz', canvas.sample.samInputSha256);
const samEncoder = await ort.InferenceSession.create(model('mobilesam-image-encoder', 'mobilesam-image-encoder.onnx'), { executionProviders: ['wasm'] });
const embedding = (await samEncoder.run({ input_image: new ort.Tensor('float32', samInput, canvas.sample.samShape) }, ['image_embeddings'])).image_embeddings;
if (embedding.dims.join(',') !== '1,256,64,64' || !embedding.data.every(Number.isFinite)) throw new Error('Invalid SAM embedding');
const embeddingHash = hash(Buffer.from(embedding.data.buffer));
await samEncoder.release();

const samDecoder = await ort.InferenceSession.create(model('mobilesam-mask-decoder', 'mobilesam-mask-decoder.onnx'), { executionProviders: ['wasm'] });
const original = canvas.sample.original;
const scaled = { width: canvas.sample.samShape[1], height: canvas.sample.samShape[0] };
const sample = (values, offset, x, y) => {
  x = Math.max(0, Math.min(255, x)); y = Math.max(0, Math.min(255, y));
  const x0 = Math.floor(x), y0 = Math.floor(y), x1 = Math.min(255, x0 + 1), y1 = Math.min(255, y0 + 1);
  const dx = x - x0, dy = y - y0;
  return (values[offset + y0 * 256 + x0] * (1 - dx) + values[offset + y0 * 256 + x1] * dx) * (1 - dy)
    + (values[offset + y1 * 256 + x0] * (1 - dx) + values[offset + y1 * 256 + x1] * dx) * dy;
};
const selected = new Uint8ClampedArray(original.width * original.height);
const groupReports = [];
for (const group of points) {
  const coords = new Float32Array((group.length + 1) * 2);
  const labels = new Float32Array(group.length + 1);
  group.forEach((point, index) => {
    coords[index * 2] = point.x * scaled.width; coords[index * 2 + 1] = point.y * scaled.height;
    labels[index] = point.label;
  });
  labels[group.length] = -1;
  const outputs = await samDecoder.run({
    image_embeddings: embedding,
    point_coords: new ort.Tensor('float32', coords, [1, labels.length, 2]),
    point_labels: new ort.Tensor('float32', labels, [1, labels.length]),
    mask_input: new ort.Tensor('float32', new Float32Array(256 * 256), [1, 1, 256, 256]),
    has_mask_input: new ort.Tensor('float32', new Float32Array(1), [1]),
    orig_im_size: new ort.Tensor('float32', new Float32Array([original.height, original.width]), [2]),
  }, ['low_res_masks', 'iou_predictions']);
  const masks = outputs.low_res_masks, scores = outputs.iou_predictions;
  if (masks.dims.join(',') !== '1,4,256,256' || scores.dims.join(',') !== '1,4') throw new Error('Invalid SAM decoder shape');
  let best = -1, bestScore = -Infinity;
  for (let candidate = 0; candidate < 4; candidate++) {
    const offset = candidate * 256 * 256;
    const consistent = group.every((point) => (sample(masks.data, offset,
      point.x * scaled.width / 4 - 0.5, point.y * scaled.height / 4 - 0.5) > 0) === (point.label === 1));
    if (consistent && scores.data[candidate] > bestScore) { best = candidate; bestScore = scores.data[candidate]; }
  }
  if (best < 0) throw new Error('SAM point conflict');
  for (let y = 0; y < original.height; y++) for (let x = 0; x < original.width; x++) {
    const logit = sample(masks.data, best * 256 * 256,
      (x + 0.5) * scaled.width / original.width / 4 - 0.5,
      (y + 0.5) * scaled.height / original.height / 4 - 0.5);
    selected[y * original.width + x] = Math.max(selected[y * original.width + x], logit > 0 ? 255 : 0);
  }
  groupReports.push({ selectedCandidate: best, scores: Array.from(scores.data),
    lowResMasksSha256: hash(Buffer.from(masks.data.buffer)) });
  masks.dispose(); scores.dispose();
}
await samDecoder.release(); embedding.dispose();
save('sam-mask.u8.gz', selected);

const originalModel = model('vitmatte-high', 'vitmatte-high.onnx');
const matteModel = Uint8Array.from(originalModel);
const prefix = new Uint8Array([10, 6, ...new TextEncoder().encode('alphas'), 18, 55, 10, 53, 8, 1, 18, 49, 10, 12, 18, 10,
  ...new TextEncoder().encode('batch_size'), 10, 14, 18, 12]);
let marker = -1;
for (let i = matteModel.indexOf(prefix[0]); i >= 0; i = matteModel.indexOf(prefix[0], i + 1)) {
  if (!prefix.every((value, offset) => matteModel[i + offset] === value)) continue;
  if (new TextDecoder().decode(matteModel.slice(i + prefix.length, i + prefix.length + 12)) !== 'num_channels') continue;
  if (marker >= 0) throw new Error('Multiple ViTMatte output symbols');
  marker = i + prefix.length;
}
if (marker < 0) throw new Error('Missing ViTMatte output symbol');
matteModel.set(new TextEncoder().encode('alpha_planes'), marker);
const matteInput = tensor('matte-input.f32le.gz', canvas.sample.matteInputSha256);
const matteSession = await ort.InferenceSession.create(matteModel, { executionProviders: ['wasm'] });
const matteOutput = (await matteSession.run({ pixel_values: new ort.Tensor('float32', matteInput, canvas.sample.matteShape) }, ['alphas'])).alphas;
if (matteOutput.dims.join(',') !== '1,1,64,96' || !matteOutput.data.every(Number.isFinite)) throw new Error('Invalid ViTMatte output');
const trimap = gunzipSync(readFileSync(new URL('trimap.u8.gz', root)));
if (hash(trimap) !== canvas.sample.trimapSha256) throw new Error('Trimap mismatch');
const matteMask = new Uint8ClampedArray(trimap.length);
for (let i = 0; i < trimap.length; i++) {
  const value = matteOutput.data[i];
  if (value < -0.001 || value > 1.001) throw new Error('Invalid alpha');
  matteMask[i] = trimap[i] === 128 ? Math.max(0, Math.min(1, value)) * 255 : trimap[i];
}
save('matte-mask.u8.gz', matteMask);
const reference = {
  schemaVersion: 1, kind: 'ORT Web WASM CPU on Chromium Canvas S04 inputs',
  browserSourceRevision: canvas.sourceRevision,
  modelSnapshotSha256: hash(readFileSync(new URL('./model-snapshot.json', import.meta.url))),
  ortWebVersion: ort.env.versions.web, backend: 'wasm',
  sample: 'hard-edge-opaque.png', points: 'sam-points.json',
  samInputSha256: canvas.sample.samInputSha256, embeddingSha256: embeddingHash,
  samGroups: groupReports, samMaskSha256: hash(selected),
  matteInputSha256: canvas.sample.matteInputSha256, trimapSha256: canvas.sample.trimapSha256,
  derivedMatteSha256: hash(matteModel), matteOutputSha256: hash(Buffer.from(matteOutput.data.buffer)),
  matteMaskSha256: hash(matteMask),
};
await matteSession.release(); matteOutput.dispose();
const path = new URL('model.json', root);
if (existsSync(path) && !process.argv.includes('--refresh')) {
  if (JSON.stringify(JSON.parse(readFileSync(path, 'utf8'))) !== JSON.stringify(reference)) throw new Error('Frozen S04 model reference changed');
  for (const [name, bytes] of pending) {
    if (hash(gunzipSync(readFileSync(new URL(name, root)))) !== hash(gunzipSync(bytes))) throw new Error(`${name}: frozen mask changed`);
  }
} else {
  writeFileSync(path, `${JSON.stringify(reference, null, 2)}\n`);
  for (const [name, bytes] of pending) writeFileSync(new URL(name, root), bytes);
}
console.log(JSON.stringify(reference, null, 2));
