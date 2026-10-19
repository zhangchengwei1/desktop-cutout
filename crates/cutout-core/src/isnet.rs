use image::RgbaImage;

pub const INPUT_SIDE: u32 = 1024;
const MEAN: [f64; 3] = [0.485, 0.456, 0.406];
// Pinned IS-Net manifest uses unit deviation; the browser fallback defaults
// are for models without an explicit preprocessing contract.
const STD: [f64; 3] = [1.0, 1.0, 1.0];

pub struct PreparedInput {
    /// NCHW tensor for the high-quality IS-Net model. Bottom/right padding is zero.
    pub tensor: Vec<f32>,
    pub fitted_width: u32,
    pub fitted_height: u32,
}

/// Port of the browser's contain placement and RGB normalization. Resampling
/// is deliberately isolated here so it can be compared against Canvas output.
pub fn prepare(source: &RgbaImage) -> Result<PreparedInput, &'static str> {
    let (source_width, source_height) = source.dimensions();
    if source_width == 0 || source_height == 0 {
        return Err("原图尺寸无效");
    }
    let scale =
        (INPUT_SIDE as f64 / source_width as f64).min(INPUT_SIDE as f64 / source_height as f64);
    let fitted_width = (source_width as f64 * scale).round().max(1.0) as u32;
    let fitted_height = (source_height as f64 * scale).round().max(1.0) as u32;
    let resized = resize_premultiplied(source, fitted_width, fitted_height);
    let plane = INPUT_SIDE as usize * INPUT_SIDE as usize;
    let mut tensor = vec![0.0; plane * 3];
    crate::parallel::chunks_mut(&mut tensor, INPUT_SIDE as usize, |row, values| {
        let channel = row / INPUT_SIDE as usize;
        let y = (row % INPUT_SIDE as usize) as u32;
        if y >= fitted_height {
            return;
        }
        for (x, output) in values.iter_mut().take(fitted_width as usize).enumerate() {
            let x = x as u32;
            let pixel = resized.get_pixel(x, y);
            // Drawing to a transparent Canvas yields zero RGB for fully
            // transparent pixels; normalized padding remains exactly zero.
            let value = if pixel[3] == 0 { 0 } else { pixel[channel] };
            // Canvas/JS computes in f64, then Float32Array rounds once.
            *output = ((value as f64 / 255.0 - MEAN[channel]) / STD[channel]) as f32;
        }
    });
    Ok(PreparedInput {
        tensor,
        fitted_width,
        fitted_height,
    })
}

/// Resize straight-alpha RGBA through premultiplied bilinear samples.
/// Callers must supply nonzero source and target dimensions.
pub fn resize_premultiplied(source: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let (sw, sh) = source.dimensions();
    let mut result = RgbaImage::new(width, height);
    crate::parallel::chunks_mut(result.as_mut(), width as usize * 4, |y, row| {
        for (x, output) in row.chunks_exact_mut(4).enumerate() {
            let sx = (x as f64 + 0.5) * f64::from(sw) / f64::from(width) - 0.5;
            let sy = (y as f64 + 0.5) * f64::from(sh) / f64::from(height) - 0.5;
            let x0 = sx.floor();
            let y0 = sy.floor();
            let tx = sx - x0;
            let ty = sy - y0;
            let sample = |dx: f64, dy: f64| {
                source.get_pixel(
                    (x0 + dx).clamp(0.0, f64::from(sw - 1)) as u32,
                    (y0 + dy).clamp(0.0, f64::from(sh - 1)) as u32,
                )
            };
            let samples = [
                (sample(0.0, 0.0), (1.0 - tx) * (1.0 - ty)),
                (sample(1.0, 0.0), tx * (1.0 - ty)),
                (sample(0.0, 1.0), (1.0 - tx) * ty),
                (sample(1.0, 1.0), tx * ty),
            ];
            let alpha: f64 = samples
                .iter()
                .map(|(pixel, weight)| f64::from(pixel[3]) * weight)
                .sum();
            let out_alpha = alpha.round() as u8;
            if out_alpha == 0 {
                output.fill(0);
                continue;
            }
            let mut pixel = [0_u8; 4];
            pixel[3] = out_alpha;
            for channel in 0..3 {
                let premult: f64 = samples
                    .iter()
                    .map(|(sample, weight)| {
                        f64::from(sample[channel]) * f64::from(sample[3]) * weight
                    })
                    .sum();
                pixel[channel] = (premult / alpha).round().clamp(0.0, 255.0) as u8;
            }
            output.copy_from_slice(&pixel);
        }
    });
    result
}

/// The browser removes contain padding by nearest-coordinate remapping after
/// normalizing the complete model output. This returns a square mask because
/// the final image-space resampling is a separate quality-sensitive stage.
pub fn restore_contain_mask(
    mask: &[u8],
    output_width: u32,
    output_height: u32,
    fitted_width: u32,
    fitted_height: u32,
) -> Result<Vec<u8>, &'static str> {
    if output_width != INPUT_SIDE
        || output_height != INPUT_SIDE
        || fitted_width == 0
        || fitted_height == 0
        || fitted_width > INPUT_SIDE
        || fitted_height > INPUT_SIDE
        || (fitted_width != INPUT_SIDE && fitted_height != INPUT_SIDE)
    {
        return Err("蒙版尺寸无效");
    }
    let count = (output_width as usize)
        .checked_mul(output_height as usize)
        .ok_or("蒙版尺寸溢出")?;
    if mask.len() != count {
        return Err("模型蒙版数据长度不匹配");
    }
    let cropped_width =
        ((fitted_width as f64 * output_width as f64 / INPUT_SIDE as f64).round() as u32).max(1);
    let cropped_height =
        ((fitted_height as f64 * output_height as f64 / INPUT_SIDE as f64).round() as u32).max(1);
    let mut restored = vec![0; count];
    crate::parallel::chunks_mut(&mut restored, output_width as usize, |y, row| {
        let source_y = (y as u64 * cropped_height as u64 / output_height as u64)
            .min((cropped_height - 1) as u64) as usize;
        for (x, output) in row.iter_mut().enumerate() {
            let source_x = (x as u64 * cropped_width as u64 / output_width as u64)
                .min((cropped_width - 1) as u64) as usize;
            *output = mask[source_y * output_width as usize + source_x];
        }
    });
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn wide_image_is_left_aligned_with_zero_normalized_padding() {
        let source = RgbaImage::from_pixel(2, 1, Rgba([255, 0, 128, 255]));
        let prepared = prepare(&source).unwrap();
        assert_eq!((prepared.fitted_width, prepared.fitted_height), (1024, 512));
        let plane = INPUT_SIDE as usize * INPUT_SIDE as usize;
        assert!((prepared.tensor[0] - 0.515).abs() < 1e-6);
        assert!((prepared.tensor[plane] + 0.456).abs() < 1e-6);
        assert!((prepared.tensor[2 * plane] - (128.0 / 255.0 - 0.406)).abs() < 1e-6);
        assert_eq!(prepared.tensor[512 * INPUT_SIDE as usize], 0.0);
        assert_eq!(prepared.tensor[plane + 512 * INPUT_SIDE as usize], 0.0);
    }

    #[test]
    fn contain_restore_discards_padded_rows() {
        let mut mask = vec![0_u8; (INPUT_SIDE * INPUT_SIDE) as usize];
        mask[..INPUT_SIDE as usize].fill(10);
        mask[INPUT_SIDE as usize..2 * INPUT_SIDE as usize].fill(50);
        mask[512 * INPUT_SIDE as usize..].fill(240);
        let restored = restore_contain_mask(&mask, INPUT_SIDE, INPUT_SIDE, 1024, 512).unwrap();
        assert_eq!(restored[0], 10);
        assert_eq!(restored[2 * INPUT_SIDE as usize], 50);
        assert!(!restored.contains(&240));
        assert!(restore_contain_mask(&mask, 512, 512, 1024, 512).is_err());
    }

    #[test]
    fn transparent_hidden_rgb_does_not_enter_tensor() {
        let source = RgbaImage::from_pixel(1, 1, Rgba([240, 50, 190, 0]));
        let prepared = prepare(&source).unwrap();
        let plane = INPUT_SIDE as usize * INPUT_SIDE as usize;
        for (channel, mean) in MEAN.iter().enumerate() {
            assert!(
                (prepared.tensor[channel * plane] + (*mean / STD[channel]) as f32).abs() < 1e-6
            );
        }
    }
}
