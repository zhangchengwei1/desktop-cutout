// Run with playwright-cli on the directory served at /tests/fixtures/.
// Canvas operations and numeric rules mirror browser-cutout/src/features/inference/u2net.ts
// at repository revision 3c8a1d00ee01b355f65d7c584eb8c4d2101c32eb.
// These are synthetic-mask references; no ONNX inference runs in this script.
async (page) => {
  const allFiles = [
    'hard-edge-opaque.png',
    'wide-alpha.png',
    'portrait-strands.png',
    'low-contrast.png',
    'gamma-tagged.png',
    'icc-tagged.png',
    'exif-orient6.jpg',
  ];
  // Chrome can block a burst of automatic downloads from one origin. Capture
  // at most three samples (nine downloads) per fresh browser session.
  const start = Number(page.url().match(/[?&]start=(\d+)/)?.[1] ?? 0);
  const end = Number(page.url().match(/[?&]end=(\d+)/)?.[1] ?? 3);
  if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end > allFiles.length || end <= start || end - start > 3) {
    throw new Error('Use ?start=N&end=M with 1–3 samples per fresh session');
  }
  const files = allFiles.slice(start, end);
  const samples = [];
  for (const name of files) {
    const sample = await page.evaluate(async (filename) => {
      const response = await fetch(`images/${filename}`);
      if (!response.ok) throw new Error(`${filename}: HTTP ${response.status}`);
      const bitmap = await createImageBitmap(await response.blob());
      const side = 1024;
      const canvas = document.createElement('canvas');
      canvas.width = side;
      canvas.height = side;
      const context = canvas.getContext('2d', { willReadFrequently: true });
      if (!context) throw new Error('2D Canvas unavailable');
      const scale = Math.min(side / bitmap.width, side / bitmap.height);
      const fittedWidth = Math.max(1, Math.round(bitmap.width * scale));
      const fittedHeight = Math.max(1, Math.round(bitmap.height * scale));
      context.drawImage(bitmap, 0, 0, fittedWidth, fittedHeight);
      const pixels = context.getImageData(0, 0, side, side).data;
      const plane = side * side;
      const tensor = new Float32Array(plane * 3);
      const snapshotResponse = await fetch('model-snapshot.json');
      if (!snapshotResponse.ok) throw new Error('Fixed model snapshot unavailable');
      const snapshot = await snapshotResponse.json();
      const preprocessing = snapshot.models.find((model) => model.id === 'isnet-auto-high')?.preprocessing;
      if (preprocessing?.fit !== 'contain' || preprocessing?.std?.length !== 3) {
        throw new Error('Fixed IS-Net preprocessing contract unavailable');
      }
      const mean = preprocessing.mean;
      const deviation = preprocessing.std;
      for (let index = 0; index < plane; index += 1) {
        for (let channel = 0; channel < 3; channel += 1) {
          tensor[channel * plane + index] = (pixels[index * 4 + channel] / 255 - mean[channel]) / deviation[channel];
        }
      }
      for (let channel = 0; channel < 3; channel += 1) {
        for (let y = 0; y < side; y += 1) {
          tensor.fill(0, channel * plane + y * side + (y < fittedHeight ? fittedWidth : 0), channel * plane + (y + 1) * side);
        }
      }

      // A deterministic synthetic model output exercises min/max, contain
      // padding removal, image-space resize, and source-alpha multiplication.
      const values = new Float32Array(plane);
      for (let y = 0; y < side; y += 1) {
        for (let x = 0; x < side; x += 1) {
          values[y * side + x] = x / 1023 * 0.75 + y / 1023 * 0.25;
        }
      }
      let minimum = Number.POSITIVE_INFINITY;
      let maximum = Number.NEGATIVE_INFINITY;
      for (const value of values) {
        minimum = Math.min(minimum, value);
        maximum = Math.max(maximum, value);
      }
      const normalized = Uint8ClampedArray.from(values, (value) => Math.round(((value - minimum) / (maximum - minimum)) * 255 + 1e-4));
      const croppedWidth = Math.max(1, Math.round(fittedWidth * side / side));
      const croppedHeight = Math.max(1, Math.round(fittedHeight * side / side));
      const restored = new Uint8ClampedArray(plane);
      for (let y = 0; y < side; y += 1) {
        for (let x = 0; x < side; x += 1) {
          restored[y * side + x] = normalized[Math.min(croppedHeight - 1, Math.floor(y * croppedHeight / side)) * side
            + Math.min(croppedWidth - 1, Math.floor(x * croppedWidth / side))];
        }
      }

      const maskCanvas = document.createElement('canvas');
      maskCanvas.width = side;
      maskCanvas.height = side;
      const maskContext = maskCanvas.getContext('2d', { willReadFrequently: true });
      const maskImage = maskContext.createImageData(side, side);
      for (let index = 0; index < plane; index += 1) {
        const offset = index * 4;
        maskImage.data[offset] = restored[index];
        maskImage.data[offset + 1] = restored[index];
        maskImage.data[offset + 2] = restored[index];
        maskImage.data[offset + 3] = 255;
      }
      maskContext.putImageData(maskImage, 0, 0);
      const scaledMaskCanvas = document.createElement('canvas');
      scaledMaskCanvas.width = bitmap.width;
      scaledMaskCanvas.height = bitmap.height;
      const scaledMaskContext = scaledMaskCanvas.getContext('2d', { willReadFrequently: true });
      scaledMaskContext.imageSmoothingEnabled = true;
      scaledMaskContext.imageSmoothingQuality = 'high';
      scaledMaskContext.drawImage(maskCanvas, 0, 0, bitmap.width, bitmap.height);
      const maskPixels = scaledMaskContext.getImageData(0, 0, bitmap.width, bitmap.height).data;
      const output = document.createElement('canvas');
      output.width = bitmap.width;
      output.height = bitmap.height;
      const outputContext = output.getContext('2d', { willReadFrequently: true });
      outputContext.drawImage(bitmap, 0, 0);
      bitmap.close();
      const outputPixels = outputContext.getImageData(0, 0, output.width, output.height);
      for (let index = 0; index < output.width * output.height; index += 1) {
        outputPixels.data[index * 4 + 3] = Math.round(outputPixels.data[index * 4 + 3] * (maskPixels[index * 4] / 255));
      }
      outputContext.clearRect(0, 0, output.width, output.height);
      outputContext.putImageData(outputPixels, 0, 0);

      const hex = async (data) => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', data)))
        .map((byte) => byte.toString(16).padStart(2, '0')).join('');
      const compress = async (bytes) => new Response(new Blob([bytes]).stream().pipeThrough(new CompressionStream('gzip'))).blob();
      const tensorBlob = await compress(tensor.buffer);
      const maskBlob = await compress(restored.buffer);
      const pngBlob = await new Promise((resolve, reject) => output.toBlob((blob) => blob ? resolve(blob) : reject(new Error('PNG encoding failed')), 'image/png'));
      window.__referenceDownloads ??= {};
      window.__referenceDownloads[`${filename}.tensor.f32le.gz`] = tensorBlob;
      window.__referenceDownloads[`${filename}.mask.u8.gz`] = maskBlob;
      window.__referenceDownloads[`${filename}.synthetic.png`] = pngBlob;
      return {
        input: filename,
        width: output.width,
        height: output.height,
        fittedWidth,
        fittedHeight,
        tensorShape: [1, 3, side, side],
        tensorSha256: await hex(tensor.buffer),
        restoredMaskShape: [side, side],
        restoredMaskSha256: await hex(restored.buffer),
        outputRgbaSha256: await hex(outputPixels.data.buffer),
        outputAlphaAtCorners: [outputPixels.data[3], outputPixels.data[(output.width - 1) * 4 + 3],
          outputPixels.data[(output.height - 1) * output.width * 4 + 3], outputPixels.data[outputPixels.data.length - 1]],
      };
    }, name);
    samples.push(sample);
    for (const suffix of ['tensor.f32le.gz', 'mask.u8.gz', 'synthetic.png']) {
      const filename = `${name}.${suffix}`;
      const downloadPromise = page.waitForEvent('download');
      await page.evaluate((file) => {
        const anchor = document.createElement('a');
        anchor.href = URL.createObjectURL(window.__referenceDownloads[file]);
        anchor.download = file;
        anchor.click();
        setTimeout(() => URL.revokeObjectURL(anchor.href), 1000);
      }, filename);
      const download = await downloadPromise;
      await download.saveAs(`tests/fixtures/reference/${filename}`);
    }
  }
  return {
    schemaVersion: 1,
    kind: 'browser-canvas-synthetic-mask',
    sourceRevision: '3c8a1d00ee01b355f65d7c584eb8c4d2101c32eb',
    start,
    end,
    userAgent: await page.evaluate(() => navigator.userAgent),
    samples,
  };
}
