use image::{
    ColorType, DynamicImage, GrayImage, ImageDecoder, ImageFormat, ImageReader, Limits, RgbaImage,
};
use moxcms::{ColorProfile, Layout, TransformOptions};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::sync::LazyLock;

/// Shared with file pickers and installer adapters. Extension checks only
/// discover candidates; headers and decoder limits still validate each file.
pub fn supported_extension(value: &str) -> Option<&'static str> {
    static EXTENSIONS: LazyLock<Vec<String>> = LazyLock::new(|| {
        #[derive(serde::Deserialize)]
        struct Formats {
            extensions: Vec<String>,
        }
        serde_json::from_str::<Formats>(include_str!("../../../resources/image-formats.json"))
            .expect("invalid bundled image formats")
            .extensions
    });
    EXTENSIONS
        .iter()
        .find(|extension| extension.eq_ignore_ascii_case(value))
        .map(String::as_str)
}

pub fn supported_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .and_then(supported_extension)
        .is_some()
}

const MAX_SOURCE_BYTES: u64 = 200 * 1024 * 1024;
const MAX_SIDE: u32 = 20_000;
const MAX_PIXELS: u64 = 50_000_000;
const MAX_DECODE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ICC_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ImageIoError {
    UnsupportedFormat,
    UnsupportedBitDepth,
    UnsupportedMetadata(&'static str),
    InvalidMetadata(&'static str),
    SourceTooLarge,
    InvalidDimensions,
    DecodeBudgetExceeded,
    InvalidMask,
    InvalidDestination,
}

impl fmt::Display for ImageIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::UnsupportedFormat => "不支持此图片格式；请使用 PNG、JPEG、WebP、GIF、BMP、TIFF、ICO、TGA、DDS、PNM、QOI 或 Farbfeld",
            Self::UnsupportedBitDepth => "仅支持 8/16 位整数通道图片；请先将 HDR/浮点图片转换为 sRGB",
            Self::UnsupportedMetadata(value) | Self::InvalidMetadata(value) => value,
            Self::SourceTooLarge => "图片文件超过 200 MiB 输入预算",
            Self::InvalidDimensions => "图片尺寸无效或超过边长/像素预算",
            Self::DecodeBudgetExceeded => "图片预计解码占用超过 256 MiB 预算",
            Self::InvalidMask => "蒙版必须是 8 位灰度 PNG",
            Self::InvalidDestination => "输出文件必须是 PNG，且不能覆盖输入",
        };
        write!(f, "{message}")
    }
}
impl Error for ImageIoError {}
type CoreResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Decode to straight-alpha 8-bit sRGB. Check the header and projected
/// allocation before decoding; convert embedded ICC in Rust.
pub fn decode_image(path: &Path) -> CoreResult<RgbaImage> {
    Ok(read_image(path, false)?.0.into_rgba8())
}

pub fn decode_png(path: &Path) -> CoreResult<RgbaImage> {
    if image_format(path)? != ImageFormat::Png {
        return Err(ImageIoError::UnsupportedFormat.into());
    }
    decode_image(path)
}

pub fn decode_gray_png(path: &Path) -> CoreResult<GrayImage> {
    if image_format(path)? != ImageFormat::Png {
        return Err(ImageIoError::InvalidMask.into());
    }
    let (image, color) = read_image(path, true)?;
    if color != ColorType::L8 {
        return Err(ImageIoError::InvalidMask.into());
    }
    Ok(image.into_luma8())
}

pub fn encode_png(image: &RgbaImage, writer: &mut impl Write) -> image::ImageResult<()> {
    encode_png_with_compression(image, writer, png::Compression::Balanced)
}

/// Previews are bounded to 1024 px and kept in a memory-limited cache. Favor
/// response time here; exported PNGs keep the normal compression setting.
pub(crate) fn encode_preview_png(
    image: &RgbaImage,
    writer: &mut impl Write,
) -> image::ImageResult<()> {
    encode_png_with_compression(image, writer, png::Compression::Fastest)
}

fn encode_png_with_compression(
    image: &RgbaImage,
    writer: &mut impl Write,
    compression: png::Compression,
) -> image::ImageResult<()> {
    let mut encoder = png::Encoder::new(writer, image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    encoder.set_compression(compression);
    let mut png_writer = encoder
        .write_header()
        .map_err(|error| image::ImageError::IoError(error.into()))?;
    png_writer
        .write_image_data(image.as_raw())
        .map_err(|error| image::ImageError::IoError(error.into()))
}

/// Same-directory temporary write and no-clobber commit for CLI and Tauri.
pub fn save_png_noclobber(image: &RgbaImage, destination: &Path) -> CoreResult<()> {
    save_png_noclobber_checked(image, destination, || Ok(()))
}

/// Call `before_commit` after encoding and syncing, immediately before the
/// no-clobber rename. The workspace uses it to reject a stale result.
pub fn save_png_noclobber_checked<T>(
    image: &RgbaImage,
    destination: &Path,
    before_commit: impl FnOnce() -> CoreResult<T>,
) -> CoreResult<()> {
    save_png_with(
        image,
        destination,
        |image, file| encode_png(image, file),
        before_commit,
    )
}

/// Hash the exact encoded bytes before publishing a batch result, without a
/// second read of the PNG. The digest is committed with the item state.
pub(crate) fn save_png_noclobber_checked_hashed<T>(
    image: &RgbaImage,
    destination: &Path,
    before_commit: impl FnOnce() -> CoreResult<T>,
) -> CoreResult<(String, u64)> {
    save_png_with(
        image,
        destination,
        |image, file| {
            let mut writer = HashingWriter {
                file,
                hasher: Sha256::new(),
                bytes: 0,
            };
            encode_png(image, &mut writer)?;
            Ok((format!("{:x}", writer.hasher.finalize()), writer.bytes))
        },
        before_commit,
    )
}

struct HashingWriter<'a> {
    file: &'a mut std::fs::File,
    hasher: Sha256,
    bytes: u64,
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.file.write(buffer)?;
        self.hasher.update(&buffer[..written]);
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

pub(crate) fn file_digest(path: &Path) -> CoreResult<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes.checked_add(read as u64).ok_or("文件过大")?;
        hasher.update(&buffer[..read]);
    }
    Ok((format!("{:x}", hasher.finalize()), bytes))
}

pub fn save_gray_png_noclobber(image: &GrayImage, destination: &Path) -> CoreResult<()> {
    if destination
        .extension()
        .and_then(|x| x.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        != Some("png")
    {
        return Err(ImageIoError::InvalidDestination.into());
    }
    let parent = destination
        .parent()
        .filter(|x| !x.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut encoder = png::Encoder::new(temporary.as_file_mut(), image.width(), image.height());
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(image.as_raw())?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist_noclobber(destination)?;
    Ok(())
}

fn save_png_with<T, R>(
    image: &RgbaImage,
    destination: &Path,
    encode: impl FnOnce(&RgbaImage, &mut std::fs::File) -> image::ImageResult<R>,
    before_commit: impl FnOnce() -> CoreResult<T>,
) -> CoreResult<R> {
    if destination
        .extension()
        .and_then(|x| x.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        != Some("png")
    {
        return Err(ImageIoError::InvalidDestination.into());
    }
    let parent = destination
        .parent()
        .filter(|x| !x.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let result = encode(image, temporary.as_file_mut())?;
    temporary.as_file_mut().sync_all()?;
    let _commit_guard = before_commit()?;
    temporary.persist_noclobber(destination)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgba};

    #[test]
    fn fast_preview_png_preserves_exact_rgba_pixels() {
        let source = RgbaImage::from_fn(37, 23, |x, y| {
            Rgba([
                (x * 7 + y) as u8,
                (x + y * 11) as u8,
                (x * 3 + y * 5) as u8,
                (x * 13 + y * 17) as u8,
            ])
        });
        let mut bytes = Vec::new();
        encode_preview_png(&source, &mut bytes).unwrap();
        assert_eq!(
            image::load_from_memory_with_format(&bytes, ImageFormat::Png)
                .unwrap()
                .into_rgba8(),
            source
        );
    }

    #[test]
    fn export_never_overwrites_and_cleans_failed_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("result.png");
        let image = RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 128]));
        save_png_noclobber(&image, &destination).unwrap();
        let before = std::fs::read(&destination).unwrap();
        assert!(before.windows(4).any(|chunk| chunk == b"sRGB"));
        assert!(save_png_noclobber(&image, &destination).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), before);
        let missing = directory.path().join("failed.png");
        assert!(save_png_with(
            &image,
            &missing,
            |_, writer| {
                writer.write_all(b"partial")?;
                Err::<(), _>(image::ImageError::IoError(std::io::Error::other(
                    "disk full",
                )))
            },
            || Ok(())
        )
        .is_err());
        assert!(!missing.exists());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(save_png_noclobber(&image, &directory.path().join("missing/result.png")).is_err());
        assert!(save_png_noclobber(&image, &directory.path().join("result.jpg")).is_err());
    }

    #[test]
    fn batch_png_digest_matches_the_committed_file() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("result.png");
        let image = RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 128]));
        let committed = save_png_noclobber_checked_hashed(&image, &destination, || Ok(())).unwrap();
        assert_eq!(committed, file_digest(&destination).unwrap());
    }

    #[test]
    fn fixture_headers_and_orientation_are_validated() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/images");
        let jpeg = decode_image(&root.join("exif-orient6.jpg")).unwrap();
        assert_eq!(jpeg.dimensions(), (64, 96));
        for name in ["gamma-tagged.png", "icc-tagged.png"] {
            assert_eq!(
                decode_image(&root.join(name)).unwrap().dimensions(),
                (96, 64)
            );
        }
        assert!(decode_image(&root.join("corrupt-truncated.png")).is_err());
        assert!(decode_image(&root.join("oversize-header.png")).is_err());
    }

    #[test]
    fn lossless_webp_decodes_with_alpha() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path();
        let source = RgbaImage::from_pixel(2, 1, Rgba([19, 78, 125, 128]));
        image::codecs::webp::WebPEncoder::new_lossless(file.reopen().unwrap())
            .write_image(source.as_raw(), 2, 1, ColorType::Rgba8.into())
            .unwrap();
        assert_eq!(decode_image(path).unwrap(), source);
    }
}

fn image_format(path: &Path) -> CoreResult<ImageFormat> {
    if std::fs::metadata(path)?.len() > MAX_SOURCE_BYTES {
        return Err(ImageIoError::SourceTooLarge.into());
    }
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    match reader.format() {
        Some(
            format @ (ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::WebP
            | ImageFormat::Gif
            | ImageFormat::Bmp
            | ImageFormat::Tiff
            | ImageFormat::Ico
            | ImageFormat::Tga
            | ImageFormat::Dds
            | ImageFormat::Pnm
            | ImageFormat::Qoi
            | ImageFormat::Farbfeld),
        ) => Ok(format),
        _ => Err(ImageIoError::UnsupportedFormat.into()),
    }
}

pub fn detected_format(path: &Path) -> CoreResult<&'static str> {
    Ok(match image_format(path)? {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpeg",
        ImageFormat::WebP => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
        ImageFormat::Tga => "tga",
        ImageFormat::Dds => "dds",
        ImageFormat::Pnm => "pnm",
        ImageFormat::Qoi => "qoi",
        ImageFormat::Farbfeld => "farbfeld",
        _ => unreachable!(),
    })
}

/// Conservative reservation for one decoded source, decoder/transform buffers,
/// full-size mask and composed output. This reads headers, not image pixels.
pub fn batch_inflight_bytes(path: &Path) -> CoreResult<u64> {
    image_format(path)?;
    let (width, height) = ImageReader::open(path)?
        .with_guessed_format()?
        .into_dimensions()?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ImageIoError::InvalidDimensions)?;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || pixels > MAX_PIXELS {
        return Err(ImageIoError::InvalidDimensions.into());
    }
    pixels
        .checked_mul(24)
        .ok_or(ImageIoError::DecodeBudgetExceeded.into())
}

fn read_image(path: &Path, mask: bool) -> CoreResult<(DynamicImage, ColorType)> {
    let format = image_format(path)?;
    if format == ImageFormat::WebP {
        let decoder = image::codecs::webp::WebPDecoder::new(std::io::BufReader::new(
            std::fs::File::open(path)?,
        ))?;
        if decoder.has_animation() {
            return Err(ImageIoError::UnsupportedMetadata("动画 WebP 暂未支持").into());
        }
    }
    let png_meta = if format == ImageFormat::Png {
        Some(scan_png_metadata(path)?)
    } else {
        None
    };
    if let Some(meta) = png_meta {
        if mask && meta.bit_depth != 8 {
            return Err(ImageIoError::UnsupportedBitDepth.into());
        }
        if meta.unsupported_color {
            return Err(
                ImageIoError::UnsupportedMetadata("PNG 的 cHRM/cICP 色彩元数据暂未支持").into(),
            );
        }
        if meta.animated {
            return Err(ImageIoError::UnsupportedMetadata("动画 PNG 暂未支持").into());
        }
    }
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or(ImageIoError::InvalidDimensions)?;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || pixels > MAX_PIXELS {
        return Err(ImageIoError::InvalidDimensions.into());
    }
    let color = decoder.color_type();
    if !matches!(
        color,
        ColorType::L8
            | ColorType::La8
            | ColorType::Rgb8
            | ColorType::Rgba8
            | ColorType::L16
            | ColorType::La16
            | ColorType::Rgb16
            | ColorType::Rgba16
    ) {
        return Err(ImageIoError::UnsupportedBitDepth.into());
    }
    // Decoder buffer, RGBA conversion and one rotation/transform buffer.
    let estimate = decoder
        .total_bytes()
        .checked_add(
            pixels
                .checked_mul(8)
                .ok_or(ImageIoError::DecodeBudgetExceeded)?,
        )
        .ok_or(ImageIoError::DecodeBudgetExceeded)?;
    if estimate > MAX_DECODE_BYTES {
        return Err(ImageIoError::DecodeBudgetExceeded.into());
    }
    if mask && color != ColorType::L8 {
        return Err(ImageIoError::InvalidMask.into());
    }
    let orientation = decoder.orientation()?;
    let icc = decoder.icc_profile()?;
    if mask && (icc.is_some() || png_meta.is_some_and(|meta| meta.gamma.is_some() || meta.srgb)) {
        return Err(ImageIoError::UnsupportedMetadata("带色彩配置的灰度蒙版暂未支持").into());
    }
    if icc.as_ref().is_some_and(|p| p.len() > MAX_ICC_BYTES) {
        return Err(ImageIoError::InvalidMetadata("ICC 配置文件超过 4 MiB 预算").into());
    }
    let mut image = DynamicImage::from_decoder(decoder)?;
    if !mask {
        let mut rgba = image.into_rgba8();
        if let Some(icc) = icc {
            convert_icc(&mut rgba, &icc)?;
        } else if let Some(meta) = png_meta {
            if let Some(gamma) = meta.gamma.filter(|_| !meta.srgb) {
                convert_gamma(&mut rgba, gamma)?;
            }
        }
        image = DynamicImage::ImageRgba8(rgba);
        image.apply_orientation(orientation);
    }
    Ok((image, color))
}

fn convert_icc(image: &mut RgbaImage, bytes: &[u8]) -> CoreResult<()> {
    let profile = ColorProfile::new_from_slice(bytes)
        .map_err(|_| ImageIoError::InvalidMetadata("ICC 配置文件无效或不受支持"))?;
    let transform = profile
        .create_transform_8bit(
            Layout::Rgba,
            &ColorProfile::new_srgb(),
            Layout::Rgba,
            TransformOptions::default(),
        )
        .map_err(|_| ImageIoError::UnsupportedMetadata("无法将 ICC 配置文件转换到 sRGB"))?;
    let stride = image.width() as usize * 4;
    let mut result = vec![0_u8; image.as_raw().len()];
    for (src, dst) in image.as_raw().chunks(stride).zip(result.chunks_mut(stride)) {
        transform
            .transform(src, dst)
            .map_err(|_| ImageIoError::InvalidMetadata("ICC 转换失败"))?;
    }
    image.as_mut().copy_from_slice(&result);
    Ok(())
}

fn convert_gamma(image: &mut RgbaImage, gamma: f64) -> CoreResult<()> {
    if !gamma.is_finite() || !(0.1..=10.0).contains(&gamma) {
        return Err(ImageIoError::InvalidMetadata("PNG gAMA 值无效").into());
    }
    let mut table = [0_u8; 256];
    for (index, value) in table.iter_mut().enumerate() {
        let linear = (index as f64 / 255.0).powf(1.0 / gamma);
        let encoded = if linear <= 0.0031308 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        };
        *value = (encoded * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    for pixel in image.pixels_mut() {
        for channel in 0..3 {
            pixel[channel] = table[pixel[channel] as usize];
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct PngMetadata {
    bit_depth: u8,
    gamma: Option<f64>,
    srgb: bool,
    unsupported_color: bool,
    animated: bool,
}

fn scan_png_metadata(path: &Path) -> CoreResult<PngMetadata> {
    let mut file = std::fs::File::open(path)?;
    let mut signature = [0; 8];
    file.read_exact(&mut signature)?;
    if signature != [137, 80, 78, 71, 13, 10, 26, 10] {
        return Err(ImageIoError::UnsupportedFormat.into());
    }
    let mut meta = PngMetadata {
        bit_depth: 0,
        gamma: None,
        srgb: false,
        unsupported_color: false,
        animated: false,
    };
    loop {
        let mut header = [0; 8];
        file.read_exact(&mut header)?;
        let size = u32::from_be_bytes(header[..4].try_into().unwrap());
        let kind = &header[4..8];
        if kind == b"IDAT" || kind == b"IEND" {
            break;
        }
        if u64::from(size) > MAX_SOURCE_BYTES {
            return Err(ImageIoError::InvalidMetadata("PNG 块长度无效").into());
        }
        match kind {
            b"IHDR" => {
                if size != 13 {
                    return Err(ImageIoError::InvalidMetadata("PNG IHDR 长度无效").into());
                }
                let mut bytes = [0; 13];
                file.read_exact(&mut bytes)?;
                meta.bit_depth = bytes[8];
            }
            b"gAMA" => {
                if size != 4 {
                    return Err(ImageIoError::InvalidMetadata("PNG gAMA 长度无效").into());
                }
                let mut bytes = [0; 4];
                file.read_exact(&mut bytes)?;
                meta.gamma = Some(f64::from(u32::from_be_bytes(bytes)) / 100_000.0);
            }
            b"sRGB" => {
                meta.srgb = true;
                file.seek(std::io::SeekFrom::Current(i64::from(size)))?;
            }
            b"cHRM" | b"cICP" => {
                meta.unsupported_color = true;
                file.seek(std::io::SeekFrom::Current(i64::from(size)))?;
            }
            b"acTL" => {
                meta.animated = true;
                file.seek(std::io::SeekFrom::Current(i64::from(size)))?;
            }
            _ => {
                file.seek(std::io::SeekFrom::Current(i64::from(size)))?;
            }
        }
        file.seek(std::io::SeekFrom::Current(4))?;
    }
    if meta.bit_depth == 0 {
        return Err(ImageIoError::InvalidMetadata("PNG 缺少 IHDR").into());
    }
    Ok(meta)
}
