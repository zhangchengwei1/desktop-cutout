use cutout_core::{batch_folder, image_io};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use std::io::Cursor;

#[test]
fn expanded_formats_decode_and_enter_folder_zip_and_shell_discovery() {
    let directory = tempfile::tempdir().unwrap();
    let source = RgbaImage::from_pixel(8, 6, Rgba([32, 128, 224, 255]));
    let formats = [
        ("bmp", ImageFormat::Bmp),
        ("gif", ImageFormat::Gif),
        ("tiff", ImageFormat::Tiff),
        ("ico", ImageFormat::Ico),
        ("tga", ImageFormat::Tga),
        ("ppm", ImageFormat::Pnm),
        ("qoi", ImageFormat::Qoi),
        ("ff", ImageFormat::Farbfeld),
    ];
    for (extension, format) in formats {
        let mut bytes = Cursor::new(Vec::new());
        let input = if format == ImageFormat::Pnm {
            DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(source.clone()).into_rgb8())
        } else if format == ImageFormat::Farbfeld {
            DynamicImage::ImageRgba16(DynamicImage::ImageRgba8(source.clone()).into_rgba16())
        } else {
            DynamicImage::ImageRgba8(source.clone())
        };
        input.write_to(&mut bytes, format).unwrap();
        let path = directory
            .path()
            .join(format!("test.{}", extension.to_uppercase()));
        std::fs::write(&path, bytes.get_ref()).unwrap();
        let decoded =
            image_io::decode_image(&path).unwrap_or_else(|error| panic!("{extension}: {error}"));
        assert_eq!(decoded, source, "{extension}");
        assert_eq!(image_io::batch_inflight_bytes(&path).unwrap(), 8 * 6 * 24);
    }
    assert_eq!(
        batch_folder::scan(directory.path(), directory.path())
            .unwrap()
            .paths
            .len(),
        formats.len()
    );
    // Invalid content is still rejected despite an accepted extension.
    let invalid = directory.path().join("invalid.tiff");
    std::fs::write(&invalid, b"not a tiff").unwrap();
    assert!(image_io::decode_image(&invalid).is_err());
    assert!(!image_io::supported_path(std::path::Path::new(
        "unsupported.heic"
    )));
}

#[test]
fn sixteen_bit_png_and_tiff_preserve_alpha_when_normalized_to_eight_bit() {
    let directory = tempfile::tempdir().unwrap();
    let source = image::ImageBuffer::from_pixel(2, 3, image::Rgba([65535_u16, 32768, 0, 32768]));
    for (extension, format) in [("png", ImageFormat::Png), ("tiff", ImageFormat::Tiff)] {
        let path = directory.path().join(format!("high-depth.{extension}"));
        DynamicImage::ImageRgba16(source.clone())
            .save_with_format(&path, format)
            .unwrap();
        let decoded = image_io::decode_image(&path).unwrap();
        assert_eq!(decoded.dimensions(), (2, 3));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 128, 0, 128]);
    }
}

#[test]
fn palette_png_and_animated_gif_use_transparency_and_first_frame() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("palette.png");
    let mut encoder = png::Encoder::new(std::fs::File::create(&path).unwrap(), 2, 1);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::One);
    encoder.set_palette(&[255, 0, 0, 0, 255, 0]);
    encoder.set_trns(&[0, 255]);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0b01000000])
        .unwrap();
    let decoded = image_io::decode_image(&path).unwrap();
    assert_eq!(decoded.get_pixel(0, 0)[3], 0);
    assert_eq!(decoded.get_pixel(1, 0).0, [0, 255, 0, 255]);
    let path = directory.path().join("animated.gif");
    let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(&path).unwrap());
    for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
        encoder
            .encode_frame(image::Frame::new(RgbaImage::from_pixel(2, 2, Rgba(color))))
            .unwrap();
    }
    drop(encoder);
    assert_eq!(
        image_io::decode_image(&path).unwrap().get_pixel(0, 0).0,
        [255, 0, 0, 255]
    );
}

#[test]
fn dds_dxt1_decode_is_bounded_and_truncation_fails() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("texture.dds");
    let mut bytes = vec![0_u8; 136];
    bytes[..4].copy_from_slice(b"DDS ");
    for (offset, value) in [
        (4, 124_u32),
        (8, 0x81007),
        (12, 4),
        (16, 4),
        (20, 8),
        (76, 32),
        (80, 4),
        (108, 0x1000),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[84..88].copy_from_slice(b"DXT1");
    bytes[128..130].copy_from_slice(&0x07e0_u16.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();
    let decoded = image_io::decode_image(&path).unwrap();
    assert_eq!(decoded.dimensions(), (4, 4));
    assert_eq!(decoded.get_pixel(0, 0).0, [0, 255, 0, 255]);
    std::fs::write(&path, &bytes[..130]).unwrap();
    assert!(image_io::decode_image(&path).is_err());
}
