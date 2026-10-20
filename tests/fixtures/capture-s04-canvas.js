// One-time Chromium Canvas reference for the pinned S04 synthetic sample.
// Mirrors mobile-sam.ts and trimap.ts without importing the website at runtime.
async (page) => {
  const sample = await page.evaluate(async () => {
    const open = async (path) => {
      const response = await fetch(path);
      if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`);
      return createImageBitmap(await response.blob());
    };
    const bitmap = await open('images/hard-edge-opaque.png');
    const maskBitmap = await open('images/mask-edges.png');
    const canvasPixels = (image, width, height) => {
      const canvas = document.createElement('canvas');
      canvas.width = width; canvas.height = height;
      const context = canvas.getContext('2d', { willReadFrequently: true });
      if (!context) throw new Error('2D Canvas unavailable');
      context.drawImage(image, 0, 0, width, height);
      return context.getImageData(0, 0, width, height).data;
    };
    const original = { width: bitmap.width, height: bitmap.height };
    const scale = 1024 / Math.max(original.width, original.height);
    const samSize = { width: Math.floor(original.width * scale + 0.5), height: Math.floor(original.height * scale + 0.5) };
    const samPixels = canvasPixels(bitmap, samSize.width, samSize.height);
    const sam = new Float32Array(samSize.width * samSize.height * 3);
    for (let i = 0; i < sam.length / 3; i++) for (let c = 0; c < 3; c++) sam[i * 3 + c] = samPixels[i * 4 + c];

    const imagePixels = canvasPixels(bitmap, original.width, original.height);
    const coarsePixels = canvasPixels(maskBitmap, maskBitmap.width, maskBitmap.height);
    if (maskBitmap.width !== original.width || maskBitmap.height !== original.height) throw new Error('mask dimensions');
    const coarse = Uint8ClampedArray.from({ length: original.width * original.height }, (_, i) => coarsePixels[i * 4]);
    const stride = original.width + 1;
    const fg = new Uint32Array(stride * (original.height + 1));
    const bg = new Uint32Array(fg.length);
    for (let y = 0; y < original.height; y++) {
      let rowFg = 0, rowBg = 0;
      for (let x = 0; x < original.width; x++) {
        const v = coarse[y * original.width + x];
        rowFg += Number(v >= 243); rowBg += Number(v <= 12);
        fg[(y + 1) * stride + x + 1] = fg[y * stride + x + 1] + rowFg;
        bg[(y + 1) * stride + x + 1] = bg[y * stride + x + 1] + rowBg;
      }
    }
    const radius = Math.max(1, Math.round(12 * Math.max(original.width, original.height) / 1024));
    const count = (table, left, top, right, bottom) => table[bottom * stride + right] - table[top * stride + right]
      - table[bottom * stride + left] + table[top * stride + left];
    const trimap = new Uint8ClampedArray(coarse.length);
    for (let y = 0; y < original.height; y++) for (let x = 0; x < original.width; x++) {
      const left = Math.max(0, x - radius), right = Math.min(original.width, x + radius + 1);
      const top = Math.max(0, y - radius), bottom = Math.min(original.height, y + radius + 1);
      const area = (right - left) * (bottom - top);
      trimap[y * original.width + x] = count(fg, left, top, right, bottom) === area ? 255
        : count(bg, left, top, right, bottom) === area ? 0 : 128;
    }
    const paddedWidth = Math.ceil(original.width / 32) * 32;
    const paddedHeight = Math.ceil(original.height / 32) * 32;
    const plane = paddedWidth * paddedHeight;
    const matte = new Float32Array(plane * 4);
    for (let y = 0; y < original.height; y++) for (let x = 0; x < original.width; x++) {
      const i = y * original.width + x, j = y * paddedWidth + x;
      for (let c = 0; c < 3; c++) matte[c * plane + j] = imagePixels[i * 4 + c] / 127.5 - 1;
      matte[3 * plane + j] = trimap[i] / 255;
    }
    const hash = async (bytes) => Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', bytes)))
      .map((byte) => byte.toString(16).padStart(2, '0')).join('');
    const gzip = async (bytes) => new Response(new Blob([bytes]).stream().pipeThrough(new CompressionStream('gzip'))).blob();
    window.__s04 = {
      'sam-input.f32le.gz': await gzip(sam.buffer),
      'matte-input.f32le.gz': await gzip(matte.buffer),
      'trimap.u8.gz': await gzip(trimap.buffer),
    };
    bitmap.close(); maskBitmap.close();
    return {
      original, samShape: [samSize.height, samSize.width, 3], matteShape: [1, 4, paddedHeight, paddedWidth],
      samInputSha256: await hash(sam.buffer), matteInputSha256: await hash(matte.buffer),
      trimapSha256: await hash(trimap.buffer),
    };
  });
  for (const filename of ['sam-input.f32le.gz', 'matte-input.f32le.gz', 'trimap.u8.gz']) {
    const pending = page.waitForEvent('download');
    await page.evaluate((name) => {
      const anchor = document.createElement('a');
      anchor.href = URL.createObjectURL(window.__s04[name]);
      anchor.download = name;
      anchor.click();
      setTimeout(() => URL.revokeObjectURL(anchor.href), 1000);
    }, filename);
    await (await pending).saveAs(`tests/fixtures/reference/model-s04/${filename}`);
  }
  return { schemaVersion: 1, kind: 'Chromium Canvas S04 model inputs',
    sourceRevision: 'b49702c7aeb84d69cf2ef4612bfd7dfe34b3b070',
    userAgent: await page.evaluate(() => navigator.userAgent), sample };
}
