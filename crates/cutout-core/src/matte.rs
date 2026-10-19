//! ViTMatte trimap and tensor contract; all dimensions refer to the scaled image.
use image::RgbaImage;

pub fn size(width: u32, height: u32) -> Result<(u32, u32), &'static str> {
    if width == 0 || height == 0 {
        return Err("matte-size");
    }
    let scale = (1024.0 / width.max(height) as f64).min(1.0);
    Ok((
        (width as f64 * scale).round().max(1.0) as u32,
        (height as f64 * scale).round().max(1.0) as u32,
    ))
}

fn clamp_round(value: f64) -> u8 {
    // Uint8ClampedArray uses ties-to-even.
    value.clamp(0.0, 255.0).round_ties_even() as u8
}

pub fn resize_mask(
    source: &[u8],
    width: u32,
    height: u32,
    target: (u32, u32),
    nearest: bool,
) -> Result<Vec<u8>, &'static str> {
    if width == 0
        || height == 0
        || target.0 == 0
        || target.1 == 0
        || source.len() != width as usize * height as usize
    {
        return Err("trimap-shape");
    }
    let mut output = vec![0; target.0 as usize * target.1 as usize];
    for y in 0..target.1 {
        for x in 0..target.0 {
            let sx = ((x as f64 + 0.5) * width as f64 / target.0 as f64 - 0.5)
                .clamp(0.0, (width - 1) as f64);
            let sy = ((y as f64 + 0.5) * height as f64 / target.1 as f64 - 0.5)
                .clamp(0.0, (height - 1) as f64);
            let value = if nearest {
                source[sy.round() as usize * width as usize + sx.round() as usize] as f64
            } else {
                let x0 = sx.floor() as usize;
                let y0 = sy.floor() as usize;
                let x1 = (x0 + 1).min(width as usize - 1);
                let y1 = (y0 + 1).min(height as usize - 1);
                let dx = sx - x0 as f64;
                let dy = sy - y0 as f64;
                (source[y0 * width as usize + x0] as f64 * (1.0 - dx)
                    + source[y0 * width as usize + x1] as f64 * dx)
                    * (1.0 - dy)
                    + (source[y1 * width as usize + x0] as f64 * (1.0 - dx)
                        + source[y1 * width as usize + x1] as f64 * dx)
                        * dy
            };
            output[y as usize * target.0 as usize + x as usize] = clamp_round(value);
        }
    }
    Ok(output)
}

fn count(
    table: &[u32],
    stride: usize,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
) -> u32 {
    table[bottom * stride + right] + table[top * stride + left]
        - table[top * stride + right]
        - table[bottom * stride + left]
}

pub fn trimap(
    mask: &[u8],
    marks: Option<&[u8]>,
    source: (u32, u32),
    target: (u32, u32),
    edge: u32,
) -> Result<Vec<u8>, &'static str> {
    if marks.is_some_and(|m| m.len() != mask.len()) {
        return Err("trimap-shape");
    }
    let mask = resize_mask(mask, source.0, source.1, target, false)?;
    let marks = marks
        .map(|m| resize_mask(m, source.0, source.1, target, true))
        .transpose()?;
    let (width, height) = (target.0 as usize, target.1 as usize);
    let stride = width + 1;
    let mut fg = vec![0u32; stride * (height + 1)];
    let mut bg = fg.clone();
    for y in 0..height {
        let (mut row_fg, mut row_bg) = (0, 0);
        for x in 0..width {
            let value = mask[y * width + x];
            row_fg += u32::from(value >= 243);
            row_bg += u32::from(value <= 12);
            fg[(y + 1) * stride + x + 1] = fg[y * stride + x + 1] + row_fg;
            bg[(y + 1) * stride + x + 1] = bg[y * stride + x + 1] + row_bg;
        }
    }
    let radius = ((edge as f64 * target.0.max(target.1) as f64 / 1024.0).round() as usize).max(1);
    let mut result = vec![0; mask.len()];
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if let Some(mark) = marks.as_ref().map(|m| m[i]).filter(|m| *m != 0) {
                result[i] = match mark {
                    1 => 0,
                    3 => 255,
                    _ => 128,
                };
                continue;
            }
            let left = x.saturating_sub(radius);
            let top = y.saturating_sub(radius);
            let right = (x + radius + 1).min(width);
            let bottom = (y + radius + 1).min(height);
            let area = ((right - left) * (bottom - top)) as u32;
            result[i] = if count(&fg, stride, left, top, right, bottom) == area {
                255
            } else if count(&bg, stride, left, top, right, bottom) == area {
                0
            } else {
                128
            };
        }
    }
    Ok(result)
}

pub fn input(image: &RgbaImage, trimap: &[u8]) -> Result<(Vec<f32>, u32, u32), &'static str> {
    let (width, height) = image.dimensions();
    if trimap.len() != width as usize * height as usize {
        return Err("trimap-shape");
    }
    let padded_width = width.div_ceil(32) * 32;
    let padded_height = height.div_ceil(32) * 32;
    let plane = padded_width as usize * padded_height as usize;
    let mut tensor = vec![0.0; plane * 4];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let i = y * width as usize + x;
            let j = y * padded_width as usize + x;
            for c in 0..3 {
                tensor[c * plane + j] = image.as_raw()[i * 4 + c] as f32 / 127.5 - 1.0;
            }
            tensor[3 * plane + j] = trimap[i] as f32 / 255.0;
        }
    }
    Ok((tensor, padded_width, padded_height))
}

pub fn alpha(
    output: &[f32],
    trimap: &[u8],
    size: (u32, u32),
    padded: (u32, u32),
) -> Result<Vec<u8>, &'static str> {
    if output.len() != padded.0 as usize * padded.1 as usize
        || trimap.len() != size.0 as usize * size.1 as usize
    {
        return Err("matte-output");
    }
    let mut mask = vec![0; trimap.len()];
    for y in 0..size.1 as usize {
        for x in 0..size.0 as usize {
            let value = output[y * padded.0 as usize + x];
            let i = y * size.0 as usize + x;
            if !value.is_finite() || !(-0.001..=1.001).contains(&value) {
                return Err("matte-output");
            }
            mask[i] = if trimap[i] == 128 {
                clamp_round(value.clamp(0.0, 1.0) as f64 * 255.0)
            } else {
                trimap[i]
            };
        }
    }
    Ok(mask)
}

const OUTPUT_PREFIX: &[u8] =
    b"\x0a\x06alphas\x12\x37\x0a\x35\x08\x01\x12\x31\x0a\x0c\x12\x0abatch_size\x0a\x0e\x12\x0c";
pub fn fix_output_metadata(bytes: &mut [u8]) -> Result<(), &'static str> {
    let mut found = None;
    for (index, window) in bytes.windows(OUTPUT_PREFIX.len()).enumerate() {
        if window != OUTPUT_PREFIX {
            continue;
        }
        let start = index + OUTPUT_PREFIX.len();
        let Some(channel) = bytes.get(start..start + 12) else {
            continue;
        };
        if channel != b"num_channels" && channel != b"alpha_planes" {
            continue;
        }
        if found.replace(start).is_some() {
            return Err("matte-metadata");
        }
    }
    let start = found.ok_or("matte-metadata")?;
    bytes[start..start + 12].copy_from_slice(b"alpha_planes");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    #[test]
    fn trimap_and_alpha_match_browser_examples() {
        assert_eq!(size(3000, 1500).unwrap(), (1024, 512));
        assert_eq!(
            resize_mask(&[0, 3], 2, 1, (4, 1), true).unwrap(),
            [0, 0, 3, 3]
        );
        let mut mask = vec![0; 45];
        for y in 0..5 {
            mask[y * 9 + 4..y * 9 + 9].fill(255);
        }
        let mut marks = vec![0; 45];
        marks[2 * 9 + 7] = 2;
        marks[2 * 9 + 3] = 3;
        marks[2 * 9 + 8] = 1;
        assert_eq!(
            &trimap(&mask, Some(&marks), (9, 5), (9, 5), 12).unwrap()[18..27],
            &[0, 0, 0, 255, 128, 255, 255, 128, 0]
        );
        let img = RgbaImage::from_pixel(2, 1, Rgba([255, 0, 127, 255]));
        let (input, w, h) = input(&img, &[128, 128]).unwrap();
        assert_eq!((w, h), (32, 32));
        assert_eq!(input[0], 1.0);
        assert_eq!(input[1024], -1.0);
        assert_eq!(input[2], 0.0);
        assert_eq!(
            alpha(&vec![0.3; 1024], &[0, 128, 255], (3, 1), (32, 32)).unwrap(),
            [0, 77, 255]
        );
    }
    #[test]
    fn metadata_patch_is_exact_and_idempotent() {
        let mut bytes = [OUTPUT_PREFIX, b"num_channels"].concat();
        fix_output_metadata(&mut bytes).unwrap();
        assert_eq!(&bytes[OUTPUT_PREFIX.len()..], b"alpha_planes");
        fix_output_metadata(&mut bytes).unwrap();
    }
}
