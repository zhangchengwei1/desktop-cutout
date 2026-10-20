// Diagnostic capture of Canvas stages; never rewrites the frozen references.
async (page) => {
  const files = [
    'hard-edge-opaque.png', 'wide-alpha.png', 'portrait-strands.png',
    'low-contrast.png', 'gamma-tagged.png', 'icc-tagged.png', 'exif-orient6.jpg',
  ];
  const query = new URL(page.url()).searchParams;
  const start = Number(query.get('start') ?? 0);
  const end = Number(query.get('end') ?? 2);
  if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end > files.length || end <= start || end - start > 2) {
    throw new Error('Use ?start=N&end=M with 1–2 samples');
  }
  const samples = [];
  for (const name of files.slice(start, end)) {
    const sample = await page.evaluate(async (filename) => {
      const [imageResponse, maskResponse] = await Promise.all([
        fetch(`images/${filename}`),
        fetch(`reference/model-isnet/${filename}.mask.u8.gz`),
      ]);
      if (!imageResponse.ok || !maskResponse.ok) throw new Error(`Missing fixture: ${filename}`);
      const bitmap = await createImageBitmap(await imageResponse.blob());
      const side = 1024;
      const scale = Math.min(side / bitmap.width, side / bitmap.height);
      const fittedWidth = Math.max(1, Math.round(bitmap.width * scale));
      const fittedHeight = Math.max(1, Math.round(bitmap.height * scale));
      const canvas = (width, height) => {
        const result = document.createElement('canvas');
        result.width = width;
        result.height = height;
        return result;
      };
      const decoded = canvas(bitmap.width, bitmap.height);
      const decodedContext = decoded.getContext('2d', { willReadFrequently: true });
      decodedContext.drawImage(bitmap, 0, 0);
      const decodedRgba = decodedContext.getImageData(0, 0, bitmap.width, bitmap.height).data;
      const resized = canvas(side, side);
      const resizedContext = resized.getContext('2d', { willReadFrequently: true });
      resizedContext.drawImage(bitmap, 0, 0, fittedWidth, fittedHeight);
      const resizedRgba = resizedContext.getImageData(0, 0, fittedWidth, fittedHeight).data;

      const mask = new Uint8Array(await new Response(maskResponse.body.pipeThrough(new DecompressionStream('gzip'))).arrayBuffer());
      if (mask.length !== side * side) throw new Error(`Invalid mask: ${filename}`);
      const maskCanvas = canvas(side, side);
      const maskContext = maskCanvas.getContext('2d', { willReadFrequently: true });
      const maskImage = maskContext.createImageData(side, side);
      for (let i = 0; i < mask.length; i += 1) {
        maskImage.data[i * 4] = mask[i];
        maskImage.data[i * 4 + 1] = mask[i];
        maskImage.data[i * 4 + 2] = mask[i];
        maskImage.data[i * 4 + 3] = 255;
      }
      maskContext.putImageData(maskImage, 0, 0);
      const scaledMaskCanvas = canvas(bitmap.width, bitmap.height);
      const scaledMaskContext = scaledMaskCanvas.getContext('2d', { willReadFrequently: true });
      scaledMaskContext.imageSmoothingEnabled = true;
      scaledMaskContext.imageSmoothingQuality = 'high';
      scaledMaskContext.drawImage(maskCanvas, 0, 0, bitmap.width, bitmap.height);
      const scaledMaskRgba = scaledMaskContext.getImageData(0, 0, bitmap.width, bitmap.height).data;
      const scaledMask = new Uint8Array(bitmap.width * bitmap.height);
      for (let i = 0; i < scaledMask.length; i += 1) scaledMask[i] = scaledMaskRgba[i * 4];
      bitmap.close();

      const compress = async (bytes) => new Response(new Blob([bytes]).stream().pipeThrough(new CompressionStream('gzip'))).blob();
      window.__s02Downloads ??= {};
      window.__s02Downloads[`${filename}.decoded.rgba.gz`] = await compress(decodedRgba);
      window.__s02Downloads[`${filename}.resized.rgba.gz`] = await compress(resizedRgba);
      window.__s02Downloads[`${filename}.scaled-mask.u8.gz`] = await compress(scaledMask);
      return { input: filename, width: decoded.width, height: decoded.height, fittedWidth, fittedHeight };
    }, name);
    samples.push(sample);
    for (const suffix of ['decoded.rgba.gz', 'resized.rgba.gz', 'scaled-mask.u8.gz']) {
      const filename = `${name}.${suffix}`;
      const downloadPromise = page.waitForEvent('download');
      await page.evaluate((file) => {
        const anchor = document.createElement('a');
        anchor.href = URL.createObjectURL(window.__s02Downloads[file]);
        anchor.download = file;
        anchor.click();
        setTimeout(() => URL.revokeObjectURL(anchor.href), 1000);
      }, filename);
      const download = await downloadPromise;
      await download.saveAs(`test-output/s02-canvas/${filename}`);
    }
  }
  return { userAgent: await page.evaluate(() => navigator.userAgent), samples };
}
