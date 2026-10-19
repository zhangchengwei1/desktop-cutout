//! Full-resolution mask editing. The webview sends only normalized points.
use image::GrayImage;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    pub points: Vec<Point>,
    pub radius: u32,
    pub value: u8,
}

pub fn paint(mask: &mut GrayImage, stroke: &Stroke) -> Result<(), &'static str> {
    if stroke.points.is_empty()
        || stroke.points.len() > 4096
        || !(1..=128).contains(&stroke.radius)
        || !matches!(stroke.value, 0 | 255)
        || stroke.points.iter().any(|point| {
            !point.x.is_finite()
                || !point.y.is_finite()
                || !(0.0..=1.0).contains(&point.x)
                || !(0.0..=1.0).contains(&point.y)
        })
    {
        return Err("画笔参数无效");
    }
    let width = mask.width();
    let height = mask.height();
    if width == 0 || height == 0 {
        return Err("蒙版尺寸无效");
    }
    let radius = stroke.radius as f32;
    let coords: Vec<(f32, f32)> = stroke
        .points
        .iter()
        .map(|point| (point.x * (width - 1) as f32, point.y * (height - 1) as f32))
        .collect();
    let estimated_stamps = coords
        .windows(2)
        .map(|segment| {
            (((segment[1].0 - segment[0].0).hypot(segment[1].1 - segment[0].1) / radius) * 2.0)
                .ceil()
                .max(1.0) as u64
                + 1
        })
        .sum::<u64>()
        .max(1);
    if estimated_stamps.saturating_mul(u64::from(stroke.radius).pow(2)) > 20_000_000 {
        return Err("单次笔触超过处理预算，请分段绘制");
    }
    for segment in coords.windows(2) {
        let (from, to) = (segment[0], segment[1]);
        let steps = (((to.0 - from.0).hypot(to.1 - from.1) / radius) * 2.0)
            .ceil()
            .max(1.0) as u32;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            stamp(
                mask,
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
                radius,
                stroke.value,
            );
        }
    }
    if coords.len() == 1 {
        stamp(mask, coords[0].0, coords[0].1, radius, stroke.value);
    }
    Ok(())
}

fn stamp(mask: &mut GrayImage, cx: f32, cy: f32, radius: f32, value: u8) {
    let max_x = mask.width() - 1;
    let max_y = mask.height() - 1;
    let left = (cx - radius).floor().max(0.0) as u32;
    let right = (cx + radius).ceil().min(max_x as f32) as u32;
    let top = (cy - radius).floor().max(0.0) as u32;
    let bottom = (cy + radius).ceil().min(max_y as f32) as u32;
    for y in top..=bottom {
        for x in left..=right {
            if (x as f32 - cx).hypot(y as f32 - cy) <= radius {
                mask.put_pixel(x, y, image::Luma([value]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    #[test]
    fn stroke_connects_points_and_rejects_bad_inputs() {
        let mut mask = GrayImage::from_pixel(16, 8, Luma([0]));
        paint(
            &mut mask,
            &Stroke {
                points: vec![Point { x: 0.0, y: 0.5 }, Point { x: 1.0, y: 0.5 }],
                radius: 1,
                value: 255,
            },
        )
        .unwrap();
        assert_eq!(mask.get_pixel(8, 4)[0], 255);
        assert_eq!(mask.get_pixel(8, 0)[0], 0);
        assert!(paint(
            &mut mask,
            &Stroke {
                points: vec![Point {
                    x: f32::NAN,
                    y: 0.5
                }],
                radius: 1,
                value: 0
            }
        )
        .is_err());
    }
}
