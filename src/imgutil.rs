use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use image::AnimationDecoder;
use image::codecs::gif::GifDecoder;
use image::{DynamicImage, ImageFormat, ImageReader, imageops::FilterType};

pub const MAX_INPUT_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 100_000_000;

pub const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

#[derive(Debug)]
pub struct InputTooLarge;

impl std::fmt::Display for InputTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "input exceeds {} MiB limit",
            MAX_INPUT_BYTES / (1024 * 1024)
        )
    }
}

impl std::error::Error for InputTooLarge {}

#[derive(Debug)]
pub struct NotRegular;

impl std::fmt::Display for NotRegular {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("input path is not a regular file")
    }
}

impl std::error::Error for NotRegular {}

/// Lowercased extension of `path`, if it has a UTF-8 one.
pub fn lowercase_extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()?
        .to_str()
        .map(str::to_ascii_lowercase)
}

pub fn has_image_extension(path: &str) -> bool {
    lowercase_extension(path).is_some_and(|ext| is_image_extension(&ext))
}

pub fn is_image_extension(ext: &str) -> bool {
    matches!(
        ext,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tiff" | "tif"
    )
}

pub fn is_png(data: &[u8]) -> bool {
    data.starts_with(&PNG_MAGIC)
}

pub fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || &data[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(data[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(data[20..24].try_into().ok()?);
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

pub fn check_limits(width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    u64::from(width) * u64::from(height) <= MAX_PIXELS
}

pub fn read_source(path: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if path.is_empty() {
        return read_limited(io::stdin().lock());
    }
    read_file(Path::new(path))
}

pub fn read_file(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(Box::new(NotRegular));
    }
    if metadata.len() > MAX_INPUT_BYTES as u64 {
        return Err(Box::new(InputTooLarge));
    }
    read_limited_with_hint(file, metadata.len() as usize)
}

pub fn read_limited<R: Read>(reader: R) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    read_limited_with_hint(reader, 0)
}

pub fn read_limited_with_hint<R: Read>(
    reader: R,
    size_hint: usize,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut reader = io::BufReader::new(reader.take((MAX_INPUT_BYTES + 1) as u64));
    let mut buf = Vec::with_capacity(size_hint.min(MAX_INPUT_BYTES));
    reader.read_to_end(&mut buf)?;
    if buf.len() > MAX_INPUT_BYTES {
        return Err(Box::new(InputTooLarge));
    }
    Ok(buf)
}

pub fn decode_with_limits(data: &[u8]) -> Result<DynamicImage, Box<dyn std::error::Error>> {
    let format = image::guess_format(data)?;
    let (width, height) =
        ImageReader::with_format(io::Cursor::new(data), format).into_dimensions()?;
    if !check_limits(width, height) {
        return Err(format!("image dimensions {width}x{height} exceed limits").into());
    }
    let image = match format {
        ImageFormat::Gif => decode_gif_first_frame(data)?,
        _ => image::load_from_memory_with_format(data, format)?,
    };
    Ok(image)
}

fn decode_gif_first_frame(data: &[u8]) -> Result<DynamicImage, Box<dyn std::error::Error>> {
    let decoder = GifDecoder::new(std::io::Cursor::new(data))?;
    let frame = decoder
        .into_frames()
        .next()
        .transpose()?
        .ok_or_else(|| String::from("GIF has no frames"))?;
    Ok(DynamicImage::ImageRgba8(frame.into_buffer()))
}

pub fn scale(image: &DynamicImage, dst_width: u32, dst_height: u32) -> DynamicImage {
    image.resize_exact(dst_width, dst_height, FilterType::CatmullRom)
}

/// Scales down, preserving aspect ratio, until the image fits both bounds.
pub fn fit_within(
    image_width: u32,
    image_height: u32,
    max_width: u32,
    max_height: u32,
) -> (u32, u32) {
    if image_width <= max_width && image_height <= max_height {
        return (image_width, image_height);
    }
    let scale =
        (max_width as f64 / image_width as f64).min(max_height as f64 / image_height as f64);
    let width = (image_width as f64 * scale).round().max(1.0) as u32;
    let height = (image_height as f64 * scale).round().max(1.0) as u32;
    (width, height)
}

pub fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let rgba = image.to_rgba8();
    let mut out = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut out);
    use image::ImageEncoder;
    encoder.write_image(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(out)
}

pub fn encode_rgba_zlib(image: &DynamicImage) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write as _;
    let rgba = image.to_rgba8();
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(rgba.as_raw())?;
    Ok(enc.finish()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, ImageBuffer, Rgba};
    use std::fs;

    fn make_png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(width, height, Rgba([0, 0, 0, 255])));
        encode_png(&image).unwrap()
    }

    #[test]
    fn image_extension_detection_ignores_case() {
        assert!(has_image_extension("image.png"));
        assert!(has_image_extension("dir/photo.JPEG"));
        assert!(has_image_extension("scan.tif"));
        assert!(!has_image_extension("archive.tar"));
        assert!(!has_image_extension("png"));
    }

    #[test]
    fn png_dimensions_valid() {
        assert_eq!(png_dimensions(&make_png_bytes(100, 200)), Some((100, 200)));
    }

    #[test]
    fn png_dimensions_too_short() {
        let data = make_png_bytes(4, 4);
        assert_eq!(png_dimensions(&data[..20]), None);
        assert_eq!(png_dimensions(&data[..8]), None);
        assert_eq!(png_dimensions(&[]), None);
    }

    #[test]
    fn png_dimensions_bad_ihdr_marker() {
        let mut data = make_png_bytes(8, 8);
        data[12] = b'X';
        assert_eq!(png_dimensions(&data), None);
    }

    #[test]
    fn png_dimensions_zero_values_fail() {
        let mut data = make_png_bytes(4, 4);
        data[16..20].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(png_dimensions(&data), None);
        let mut data = make_png_bytes(4, 4);
        data[20..24].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(png_dimensions(&data), None);
    }

    #[test]
    fn check_limits_behaviour() {
        assert!(check_limits(1920, 1080));
        assert!(check_limits(3840, 2160));
        assert!(check_limits(1, 1));
        assert!(!check_limits(0, 0));
        assert!(!check_limits(0, 100));
        assert!(!check_limits(100, 0));
        assert!(!check_limits(10001, 10000));
        assert!(check_limits(10000, 10000));
    }

    #[test]
    fn fit_within_behaviour() {
        assert_eq!(fit_within(100, 100, 800, 600), (100, 100));
        assert_eq!(fit_within(1600, 400, 800, 600), (800, 200));
        assert_eq!(fit_within(200, 2000, 800, 600), (60, 600));
        assert_eq!(fit_within(800, 600, 800, 600), (800, 600));
        assert_eq!(fit_within(1600, 1600, 800, 600), (600, 600));
        assert_eq!(fit_within(200, 2000, 800, u32::MAX), (200, 2000));
    }

    #[test]
    fn encode_png_round_trip() {
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_fn(4, 4, |x, y| {
            Rgba([(x * 64) as u8, (y * 64) as u8, 128, 255])
        }));
        let encoded = encode_png(&image).unwrap();
        assert!(is_png(&encoded));
        let decoded = decode_with_limits(&encoded).unwrap();
        assert_eq!(decoded.dimensions(), (4, 4));
    }

    #[test]
    fn encode_rgba_zlib_round_trip() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([1, 2, 3, 255])));
        let compressed = encode_rgba_zlib(&img).unwrap();
        assert!(!compressed.is_empty());
        // Decompress and verify
        use flate2::read::ZlibDecoder;
        use std::io::Read;
        let mut dec = ZlibDecoder::new(compressed.as_slice());
        let mut out = Vec::new();
        dec.read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), 2 * 2 * 4);
        assert_eq!(&out[0..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn decode_with_limits_bad_data() {
        assert!(decode_with_limits(b"not an image at all").is_err());
    }

    #[test]
    fn read_source_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.png");
        let bytes = make_png_bytes(2, 2);
        fs::write(&path, &bytes).unwrap();
        assert_eq!(read_source(path.to_str().unwrap()).unwrap(), bytes);
    }

    #[test]
    fn read_source_missing_file() {
        assert!(read_source("/nonexistent/path/that/cannot/exist.png").is_err());
    }

    #[test]
    fn read_source_rejects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_source(dir.path().to_str().unwrap()).unwrap_err();
        assert!(err.downcast_ref::<NotRegular>().is_some());
    }

    #[test]
    fn read_source_file_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("too-large.bin");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_INPUT_BYTES as u64 + 1).unwrap();
        let err = read_source(path.to_str().unwrap()).unwrap_err();
        assert!(err.downcast_ref::<InputTooLarge>().is_some());
    }

    /// Returns a hardcoded valid 4x4 indexed-colour (paletted) PNG.
    /// Built with a 4-colour PLTE (red, green, blue, white) and 8-bit depth.
    fn paletted_png_4x4() -> &'static [u8] {
        // 96 bytes: 4x4 indexed-color PNG with 4-color palette (red,green,blue,white)
        &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x08, 0x03, 0x00, 0x00,
            0x00, 0x9e, 0x2f, 0x6e, 0x4c, 0x00, 0x00, 0x00, 0x0c, 0x50, 0x4c, 0x54, 0x45, 0xff,
            0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xfb, 0x00, 0x60,
            0xf6, 0x00, 0x00, 0x00, 0x0f, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x60,
            0x64, 0x62, 0x66, 0x40, 0x25, 0x00, 0x00, 0xf0, 0x00, 0x19, 0x8d, 0x68, 0xb3, 0x78,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn test_decode_paletted_png_via_decode_with_limits() {
        let decoded = decode_with_limits(paletted_png_4x4())
            .expect("decode_with_limits failed on paletted PNG");
        assert_eq!(decoded.width(), 4, "decoded width should be 4");
        assert_eq!(decoded.height(), 4, "decoded height should be 4");
    }
}
