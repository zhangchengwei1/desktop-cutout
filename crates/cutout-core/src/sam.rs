//! MobileSAM's image/point and mask contracts, independent of ONNX Runtime.
use image::RgbaImage;

const SIDE: usize = 256;

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub label: u8,
}

pub fn resized_size(width: u32, height: u32) -> Result<(u32, u32), &'static str> {
    if width == 0 || height == 0 {
        return Err("sam-size");
    }
    let scale = 1024.0 / width.max(height) as f64;
    let resized = (
        (width as f64 * scale + 0.5).floor() as u32,
        (height as f64 * scale + 0.5).floor() as u32,
    );
    if resized.0 == 0 || resized.1 == 0 {
        return Err("sam-size");
    }
    Ok(resized)
}

/// Raw HWC RGB [0,255]; the pinned encoder normalizes and pads internally.
pub fn image_tensor(image: &RgbaImage) -> Result<(Vec<f32>, u32, u32), &'static str> {
    let (width, height) = resized_size(image.width(), image.height())?;
    // Canvas drawImage blends in premultiplied-alpha space. Hidden RGB under
    // transparent pixels must not leak into the encoder's HWC input.
    let resized = crate::isnet::resize_premultiplied(image, width, height);
    let mut values = Vec::with_capacity(width as usize * height as usize * 3);
    for pixel in resized.pixels() {
        values.extend(pixel.0[..3].iter().map(|value| *value as f32));
    }
    Ok((values, width, height))
}

pub fn prompt(
    points: &[Point],
    width: u32,
    height: u32,
) -> Result<(Vec<f32>, Vec<f32>), &'static str> {
    if points.is_empty()
        || points.len() > 64
        || !points.iter().any(|p| p.label == 1)
        || points.iter().any(|p| {
            p.label > 1
                || !p.x.is_finite()
                || !p.y.is_finite()
                || !(0.0..=1.0).contains(&p.x)
                || !(0.0..=1.0).contains(&p.y)
        })
    {
        return Err("sam-points");
    }
    let (width, height) = resized_size(width, height)?;
    let mut coords = vec![0.0; (points.len() + 1) * 2];
    let mut labels = vec![0.0; points.len() + 1];
    for (i, point) in points.iter().enumerate() {
        coords[i * 2] = point.x * width as f32;
        coords[i * 2 + 1] = point.y * height as f32;
        labels[i] = point.label as f32;
    }
    labels[points.len()] = -1.0;
    Ok((coords, labels))
}

fn sample(values: &[f32], offset: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, 255.0);
    let y = y.clamp(0.0, 255.0);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(255);
    let y1 = (y0 + 1).min(255);
    let dx = x - x0 as f32;
    let dy = y - y0 as f32;
    (values[offset + y0 * SIDE + x0] * (1.0 - dx) + values[offset + y0 * SIDE + x1] * dx)
        * (1.0 - dy)
        + (values[offset + y1 * SIDE + x0] * (1.0 - dx) + values[offset + y1 * SIDE + x1] * dx) * dy
}

pub fn select_mask(
    masks: &[f32],
    scores: &[f32],
    points: &[Point],
    original: (u32, u32),
    output: (u32, u32),
) -> Result<Vec<u8>, &'static str> {
    if scores.is_empty()
        || masks.len() != scores.len() * SIDE * SIDE
        || scores.iter().any(|v| !v.is_finite())
        || masks.iter().any(|v| !v.is_finite())
        || output.0 == 0
        || output.1 == 0
    {
        return Err("sam-output");
    }
    prompt(points, original.0, original.1)?;
    let scaled = resized_size(original.0, original.1)?;
    let mut best = None;
    let mut best_score = f32::NEG_INFINITY;
    for (candidate, score) in scores.iter().enumerate() {
        let offset = candidate * SIDE * SIDE;
        let consistent = points.iter().all(|point| {
            (sample(
                masks,
                offset,
                point.x * scaled.0 as f32 / 4.0 - 0.5,
                point.y * scaled.1 as f32 / 4.0 - 0.5,
            ) > 0.0)
                == (point.label == 1)
        });
        if consistent && *score > best_score {
            best = Some(candidate);
            best_score = *score;
        }
    }
    let offset = best.ok_or("sam-conflict")? * SIDE * SIDE;
    let mut result = vec![0; output.0 as usize * output.1 as usize];
    for y in 0..output.1 as usize {
        for x in 0..output.0 as usize {
            let sx = (x as f32 + 0.5) * scaled.0 as f32 / output.0 as f32 / 4.0 - 0.5;
            let sy = (y as f32 + 0.5) * scaled.1 as f32 / output.1 as f32 / 4.0 - 0.5;
            result[y * output.0 as usize + x] = if sample(masks, offset, sx, sy) > 0.0 {
                255
            } else {
                0
            };
        }
    }
    Ok(result)
}

pub fn union(a: &[u8], b: &[u8]) -> Result<Vec<u8>, &'static str> {
    if a.len() != b.len() {
        return Err("sam-size");
    }
    Ok(a.iter().zip(b).map(|(x, y)| (*x).max(*y)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prompt_and_candidate_match_browser_contract() {
        assert_eq!(resized_size(1600, 900).unwrap(), (1024, 576));
        assert!(resized_size(1, 20_000).is_err());
        let points = [
            Point {
                x: 0.2,
                y: 0.5,
                label: 1,
            },
            Point {
                x: 0.8,
                y: 0.5,
                label: 0,
            },
        ];
        let (coords, labels) = prompt(&points, 1600, 900).unwrap();
        assert_eq!(coords, [204.8, 288.0, 819.2, 288.0, 0.0, 0.0]);
        assert_eq!(labels, [1.0, 0.0, -1.0]);
        let mut masks = vec![-1.0; 4 * SIDE * SIDE];
        masks[..SIDE * SIDE].fill(1.0);
        for y in 0..SIDE {
            masks[SIDE * SIDE + y * SIDE..SIDE * SIDE + y * SIDE + 128].fill(1.0);
        }
        let mask = select_mask(
            &masks,
            &[0.99, 0.8, 0.7, 0.1],
            &points,
            (1600, 900),
            (16, 9),
        )
        .unwrap();
        assert_eq!(mask[4 * 16 + 3], 255);
        assert_eq!(mask[4 * 16 + 13], 0);
        assert_eq!(union(&[255, 0, 0], &[0, 0, 255]).unwrap(), [255, 0, 255]);
        assert!(select_mask(
            &vec![-1.0; SIDE * SIDE],
            &[0.8],
            &[Point {
                x: 0.5,
                y: 0.5,
                label: 1
            }],
            (100, 100),
            (4, 4)
        )
        .is_err());
    }

    #[test]
    fn transparent_hidden_rgb_does_not_enter_encoder_input() {
        let image = RgbaImage::from_pixel(1, 1, image::Rgba([240, 50, 190, 0]));
        let (input, width, height) = image_tensor(&image).unwrap();
        assert_eq!((width, height), (1024, 1024));
        assert!(input.iter().all(|value| *value == 0.0));
    }
}
