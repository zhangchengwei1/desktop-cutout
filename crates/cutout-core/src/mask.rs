use image::{imageops, GrayImage, RgbaImage};

#[derive(Debug, PartialEq, Eq)]
pub enum MaskError {
    InvalidValues,
    DimensionMismatch,
    InvalidDimensions,
}

impl std::fmt::Display for MaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidValues => write!(f, "模型没有生成有效蒙版"),
            Self::DimensionMismatch => write!(f, "蒙版尺寸与原图不匹配"),
            Self::InvalidDimensions => write!(f, "蒙版尺寸或数据长度无效"),
        }
    }
}

impl std::error::Error for MaskError {}

/// Matches the browser pipeline's global min/max normalization. A constant
/// or non-finite output is a model failure, not an empty successful mask.
pub fn normalize(values: &[f32]) -> Result<Vec<u8>, MaskError> {
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        return Err(MaskError::InvalidValues);
    }
    let minimum = values.iter().copied().fold(f32::INFINITY, f32::min);
    let maximum = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let minimum = minimum as f64;
    let range = maximum as f64 - minimum;
    if !range.is_finite() || range < 1e-7 {
        return Err(MaskError::InvalidValues);
    }
    let mut normalized = vec![0; values.len()];
    crate::parallel::chunks_mut(&mut normalized, 4096, |index, chunk| {
        for (output, value) in chunk.iter_mut().zip(&values[index * 4096..]) {
            *output = (((*value as f64 - minimum) / range) * 255.0 + 1e-4).round() as u8;
        }
    });
    Ok(normalized)
}

/// Apply a full-resolution grayscale mask while retaining the source RGB and
/// multiplying, rather than replacing, its alpha channel.
pub fn apply(source: &RgbaImage, mask: &GrayImage) -> Result<RgbaImage, MaskError> {
    if source.dimensions() != mask.dimensions() {
        return Err(MaskError::DimensionMismatch);
    }
    let mut result = source.clone();
    crate::parallel::chunks_mut(result.as_mut(), 4096 * 4, |index, chunk| {
        for (pixel, coverage) in chunk
            .chunks_exact_mut(4)
            .zip(&mask.as_raw()[index * 4096..])
        {
            let alpha = pixel[3] as f64 * *coverage as f64 / 255.0;
            pixel[3] = alpha.round() as u8;
        }
    });
    Ok(result)
}

/// Upsample the restored model mask once, then multiply the original alpha.
pub fn apply_model_mask(
    source: &RgbaImage,
    mask: &[u8],
    width: u32,
    height: u32,
) -> Result<RgbaImage, MaskError> {
    let scaled = resize_model_mask(mask, width, height, source.width(), source.height())?;
    apply(source, &scaled)
}

pub fn resize_model_mask(
    mask: &[u8],
    width: u32,
    height: u32,
    target_width: u32,
    target_height: u32,
) -> Result<GrayImage, MaskError> {
    let count = (width as usize)
        .checked_mul(height as usize)
        .ok_or(MaskError::InvalidDimensions)?;
    if count == 0 || mask.len() != count || target_width == 0 || target_height == 0 {
        return Err(MaskError::InvalidDimensions);
    }
    let gray =
        GrayImage::from_raw(width, height, mask.to_vec()).ok_or(MaskError::InvalidDimensions)?;
    Ok(imageops::resize(
        &gray,
        target_width,
        target_height,
        imageops::FilterType::CatmullRom,
    ))
}

pub enum Background<'a> {
    Solid([u8; 3]),
    Image(&'a RgbaImage),
}

/// Source-over composite. Image backgrounds use center-cover geometry.
pub fn with_background(
    cutout: &RgbaImage,
    background: Background<'_>,
) -> Result<RgbaImage, MaskError> {
    if cutout.width() == 0 || cutout.height() == 0 {
        return Err(MaskError::InvalidDimensions);
    }
    let mut canvas = match background {
        Background::Solid(rgb) => RgbaImage::from_pixel(
            cutout.width(),
            cutout.height(),
            image::Rgba([rgb[0], rgb[1], rgb[2], 255]),
        ),
        Background::Image(image) => {
            if image.width() == 0 || image.height() == 0 {
                return Err(MaskError::InvalidDimensions);
            }
            image::DynamicImage::ImageRgba8(image.clone())
                .resize_to_fill(
                    cutout.width(),
                    cutout.height(),
                    imageops::FilterType::CatmullRom,
                )
                .to_rgba8()
        }
    };
    crate::parallel::chunks_mut(canvas.as_mut(), 4096 * 4, |index, chunk| {
        for (dst, src) in chunk
            .chunks_exact_mut(4)
            .zip(cutout.as_raw()[index * 4096 * 4..].chunks_exact(4))
        {
            let sa = u32::from(src[3]);
            let da = u32::from(dst[3]);
            let out_alpha = sa * 255 + da * (255 - sa);
            if out_alpha == 0 {
                dst.fill(0);
                continue;
            }
            for channel in 0..3 {
                let numerator =
                    u32::from(src[channel]) * sa * 255 + u32::from(dst[channel]) * da * (255 - sa);
                dst[channel] = (numerator as f64 / out_alpha as f64).round() as u8;
            }
            dst[3] = (out_alpha as f64 / 255.0).round() as u8;
        }
    });
    Ok(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, Rgba};

    fn parallel_parity(width: u32, height: u32, timings: bool) {
        let source = RgbaImage::from_fn(width, height, |x, y| {
            Rgba([
                (x * 71 + y * 37) as u8,
                (x * 11 + y * 13) as u8,
                (x * 23 + y * 29) as u8,
                (x * 17 + y * 31) as u8,
            ])
        });
        let coverage = GrayImage::from_fn(width, height, |x, y| Luma([(x * 19 + y * 43) as u8]));
        let work = || {
            let prepared = crate::isnet::prepare(&source).unwrap();
            let applied = apply(&source, &coverage).unwrap();
            let composite = with_background(&applied, Background::Solid([32, 64, 128])).unwrap();
            (prepared.tensor, applied, composite)
        };
        let expected = crate::parallel::serial(work);
        assert_eq!(
            work(),
            expected,
            "parallel processing must retain every RGBA byte and tensor element"
        );
        if timings {
            for parallel in [false, true] {
                let mut runs = Vec::new();
                for _ in 0..5 {
                    let start = std::time::Instant::now();
                    std::hint::black_box(if parallel {
                        work()
                    } else {
                        crate::parallel::serial(work)
                    });
                    runs.push(start.elapsed().as_millis());
                }
                println!("pixels {width}x{height} parallel={parallel}: {runs:?} ms");
            }
        }
    }

    #[test]
    fn parallel_pixels_preserve_transparency_and_rounding() {
        parallel_parity(641, 413, false);
    }

    #[test]
    fn parallel_normalization_and_contain_restore_match_serial() {
        let values: Vec<f32> = (0..1024 * 1024)
            .map(|i| ((i * 73 % 10001) as f32 - 5000.0) / 3000.0)
            .collect();
        let expected = crate::parallel::serial(|| normalize(&values)).unwrap();
        assert_eq!(normalize(&values).unwrap(), expected);
        let restore =
            || crate::isnet::restore_contain_mask(&expected, 1024, 1024, 1024, 413).unwrap();
        assert_eq!(restore(), crate::parallel::serial(restore));
    }

    #[test]
    #[ignore = "manual release-mode pixel benchmark"]
    fn parallel_pixel_latency_probe() {
        parallel_parity(1254, 1254, true);
        parallel_parity(3840, 2160, true);
    }

    #[test]
    fn normalization_rejects_constant_and_non_finite_outputs() {
        assert_eq!(normalize(&[0.5, 0.5]), Err(MaskError::InvalidValues));
        assert_eq!(normalize(&[0.0, f32::NAN]), Err(MaskError::InvalidValues));
        assert_eq!(normalize(&[]), Err(MaskError::InvalidValues));
    }

    #[test]
    fn normalization_uses_global_range() {
        assert_eq!(normalize(&[-2.0, -1.0, 0.0]).unwrap(), vec![0, 128, 255]);
    }

    #[test]
    fn composition_preserves_rgb_and_source_alpha() {
        let source = RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgba([12, 34, 56, 128])
            } else {
                Rgba([90, 80, 70, 200])
            }
        });
        let mask = GrayImage::from_fn(2, 1, |x, _| if x == 0 { Luma([128]) } else { Luma([0]) });
        let output = apply(&source, &mask).unwrap();
        assert_eq!(output.get_pixel(0, 0).0, [12, 34, 56, 64]);
        assert_eq!(output.get_pixel(1, 0).0, [90, 80, 70, 0]);
        assert_eq!(source.get_pixel(0, 0).0, [12, 34, 56, 128]);
    }

    #[test]
    fn composition_rejects_different_dimensions() {
        let source = RgbaImage::new(2, 1);
        let mask = GrayImage::new(1, 1);
        assert_eq!(apply(&source, &mask), Err(MaskError::DimensionMismatch));
    }

    #[test]
    fn model_mask_scales_and_keeps_source_alpha() {
        let source = RgbaImage::from_pixel(4, 2, Rgba([20, 40, 60, 128]));
        let output = apply_model_mask(&source, &[255], 1, 1).unwrap();
        assert!(output.pixels().all(|pixel| pixel[3] == 128));
        assert_eq!(
            apply_model_mask(&source, &[], 1, 1),
            Err(MaskError::InvalidDimensions)
        );
    }

    #[test]
    fn solid_background_uses_source_over() {
        let cutout = RgbaImage::from_pixel(1, 1, Rgba([200, 0, 0, 128]));
        let result = with_background(&cutout, Background::Solid([0, 0, 200])).unwrap();
        assert_eq!(result.get_pixel(0, 0).0, [100, 0, 100, 255]);
    }

    #[test]
    fn image_background_covers_target_dimensions() {
        let cutout = RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 0]));
        let backdrop = RgbaImage::from_pixel(4, 2, Rgba([10, 20, 30, 255]));
        let result = with_background(&cutout, Background::Image(&backdrop)).unwrap();
        assert_eq!(result.dimensions(), (2, 2));
        assert!(result.pixels().all(|pixel| pixel.0 == [10, 20, 30, 255]));
    }

    #[test]
    fn image_background_uses_center_cover_crop() {
        let cutout = RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 0]));
        let backdrop = RgbaImage::from_fn(4, 2, |x, _| Rgba([x as u8 * 40, 0, 0, 255]));
        let result = with_background(&cutout, Background::Image(&backdrop)).unwrap();
        assert_eq!(result.get_pixel(0, 0)[0], 40);
        assert_eq!(result.get_pixel(1, 0)[0], 80);
    }
}
