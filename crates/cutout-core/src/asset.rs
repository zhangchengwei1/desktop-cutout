//! Bounded, encoded previews. Source files stay on disk; RGBA never crosses IPC.
use crate::{image_io, isnet, mask};
use image::{imageops, GrayImage, RgbaImage};
use std::{error::Error, path::Path, sync::Arc};

const PREVIEW_SIDE: u32 = 1024;
pub(crate) const SOURCE_CACHE_BUDGET: usize = 64 * 1024 * 1024;

pub(crate) struct Preview {
    pub source_size: (u32, u32),
    pub png: Vec<u8>,
    pub source: Option<Arc<RgbaImage>>,
}

pub(crate) fn build_preview(path: &Path) -> Result<Preview, Box<dyn Error + Send + Sync>> {
    let source = image_io::decode_image(path)?;
    let (width, height) = source.dimensions();
    let longest = width.max(height);
    let (preview, cached_source) = if longest > PREVIEW_SIDE {
        let scale = f64::from(PREVIEW_SIDE) / f64::from(longest);
        let target_width = (f64::from(width) * scale).round().max(1.0) as u32;
        let target_height = (f64::from(height) * scale).round().max(1.0) as u32;
        let preview = isnet::resize_premultiplied(&source, target_width, target_height);
        let cached = (source.as_raw().len() <= SOURCE_CACHE_BUDGET).then(|| Arc::new(source));
        (preview, cached)
    } else {
        (source, None)
    };
    let mut png = Vec::new();
    image_io::encode_preview_png(&preview, &mut png)?;
    Ok(Preview {
        source_size: (width, height),
        png,
        source: cached_source,
    })
}

pub(crate) fn build_result_preview(
    original_png: &[u8],
    full_mask: &GrayImage,
    background: Option<mask::Background<'_>>,
) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let original =
        image::load_from_memory_with_format(original_png, image::ImageFormat::Png)?.into_rgba8();
    let cutout = if full_mask.dimensions() == original.dimensions() {
        mask::apply(&original, full_mask)?
    } else {
        let preview_mask = imageops::resize(
            full_mask,
            original.width(),
            original.height(),
            imageops::FilterType::CatmullRom,
        );
        mask::apply(&original, &preview_mask)?
    };
    let result = if let Some(background) = background {
        mask::with_background(&cutout, background)?
    } else {
        cutout
    };
    let mut png = Vec::new();
    image_io::encode_preview_png(&result, &mut png)?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, Rgba, RgbaImage};

    #[test]
    fn matching_mask_size_keeps_the_previous_preview_pixels() {
        let original = RgbaImage::from_fn(37, 23, |x, y| {
            Rgba([x as u8, y as u8, (x + y) as u8, (x * 3 + y * 5) as u8])
        });
        let full_mask = GrayImage::from_fn(37, 23, |x, y| Luma([(x * 17 + y * 7) as u8]));
        let mut original_png = Vec::new();
        image_io::encode_preview_png(&original, &mut original_png).unwrap();
        let actual = build_result_preview(&original_png, &full_mask, None).unwrap();
        let resized = imageops::resize(&full_mask, 37, 23, imageops::FilterType::CatmullRom);
        let expected = mask::apply(&original, &resized).unwrap();
        let actual = image::load_from_memory_with_format(&actual, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        assert_eq!(actual, expected);
    }
}
