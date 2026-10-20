// Playwright CLI script: render the fixed ORT Web/WASM IS-Net masks using
// the browser-cutout u2net.ts Canvas alpha-composition behavior.
async (page) => {
  const files = [
    'hard-edge-opaque.png', 'wide-alpha.png', 'portrait-strands.png',
    'low-contrast.png', 'gamma-tagged.png', 'icc-tagged.png', 'exif-orient6.jpg',
  ];
  const samples = [];
  for (const filename of files) {
    const sample = await page.evaluate(async (name) => {
      const [sourceResponse, maskResponse] = await Promise.all([
        fetch(`images/${name}`),
        fetch(`reference/model-isnet/${name}.mask.u8.gz`),
      ]);
      if (!sourceResponse.ok || !maskResponse.ok) throw new Error(`Missing input or mask: ${name}`);
      const bitmap = await createImageBitmap(await sourceResponse.blob());
      const mask = new Uint8Array(await new Response(maskResponse.body.pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
      if (mask.length !== 1024 * 1024) throw new Error(`Mask length mismatch: ${name}`);
      const maskCanvas = document.createElement('canvas');
      maskCanvas.width = 1024;
      maskCanvas.height = 1024;
      const maskContext = maskCanvas.getContext('2d', { willReadFrequently: true });
      const maskImage = maskContext.createImageData(1024, 1024);
      for (let index = 0; index < mask.length; index += 1) {
        const offset = index * 4;
        maskImage.data[offset] = mask[index];
        maskImage.data[offset + 1] = mask[index];
        maskImage.data[offset + 2] = mask[index];
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
      const scaledMask = scaledMaskContext.getImageData(0, 0, bitmap.width, bitmap.height).data;
      const output = document.createElement('canvas');
      output.width = bitmap.width;
      output.height = bitmap.height;
      const context = output.getContext('2d', { willReadFrequently: true });
      context.drawImage(bitmap, 0, 0);
      bitmap.close();
      const pixels = context.getImageData(0, 0, output.width, output.height);
      for (let index = 0; index < output.width * output.height; index += 1) {
        pixels.data[index * 4 + 3] = Math.round(pixels.data[index * 4 + 3] * (scaledMask[index * 4] / 255));
      }
      context.clearRect(0, 0, output.width, output.height);
      context.putImageData(pixels, 0, 0);
      const digest = await crypto.subtle.digest('SHA-256', pixels.data.buffer);
      const rgbaSha256 = Array.from(new Uint8Array(digest)).map((byte) => byte.toString(16).padStart(2, '0')).join('');
      const blob = await new Promise((resolve, reject) => output.toBlob((value) => value ? resolve(value) : reject(new Error('PNG encoding failed')), 'image/png'));
      window.__modelRenderDownload = blob;
      return { input: name, width: output.width, height: output.height, rgbaSha256 };
    }, filename);
    samples.push(sample);
    const downloadPromise = page.waitForEvent('download');
    await page.evaluate((name) => {
      const anchor = document.createElement('a');
      anchor.href = URL.createObjectURL(window.__modelRenderDownload);
      anchor.download = `${name}.model.png`;
      anchor.click();
      setTimeout(() => URL.revokeObjectURL(anchor.href), 1000);
    }, filename);
    const download = await downloadPromise;
    await download.saveAs(`tests/fixtures/reference/model-isnet/${filename}.model.png`);
  }
  return { schemaVersion: 1, kind: 'Chromium Canvas render of ORT Web/WASM IS-Net masks',
    userAgent: await page.evaluate(() => navigator.userAgent), samples };
}
