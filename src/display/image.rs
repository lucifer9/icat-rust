use image::GenericImageView;

use crate::imgutil;
use crate::kitty;
use crate::term::Size;

#[derive(Debug, Clone)]
pub struct PreparedImage {
    pub png_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn image(path: &str, size: Size, tmux: bool) -> Result<(), Box<dyn std::error::Error>> {
    let raw = imgutil::read_source(path).map_err(|err| {
        let label = if path.is_empty() { "<stdin>" } else { path };
        format!("failed to read image {label}: {err}")
    })?;
    encode_and_send(&raw, size, tmux)
}

pub fn image_from_bytes(
    data: &[u8],
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    encode_and_send(data, size, tmux)
}

fn encode_and_send(raw: &[u8], size: Size, tmux: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Fast path: unscaled PNG — send raw bytes directly, no copy needed
    if imgutil::is_png(raw)
        && let Some((width, height)) = imgutil::png_dimensions(raw)
        && imgutil::check_limits(width, height)
    {
        let (sw, sh) = imgutil::fit_to_width(width, height, size.pixel_width);
        if sw == width && sh == height {
            return kitty::send_static_image(raw, width, height, size, tmux);
        }
    }

    // Decode and optionally scale
    let mut image =
        imgutil::decode_with_limits(raw).map_err(|e| format!("failed to decode image: {e}"))?;
    let (width, height) = image.dimensions();
    let (sw, sh) = imgutil::fit_to_width(width, height, size.pixel_width);
    if sw != width || sh != height {
        image = imgutil::scale(&image, sw, sh);
    }

    // Send as RGBA + zlib (skips PNG filter selection + DEFLATE overhead)
    let zlib_data =
        imgutil::encode_rgba_zlib(&image).map_err(|e| format!("failed to encode RGBA: {e}"))?;
    kitty::send_static_image_rgba_zlib(&zlib_data, sw, sh, size, tmux)
}

pub fn prepare_image(
    raw: &[u8],
    max_pixel_width: u32,
) -> Result<PreparedImage, Box<dyn std::error::Error>> {
    if imgutil::is_png(raw)
        && let Some((width, height)) = imgutil::png_dimensions(raw)
    {
        if !imgutil::check_limits(width, height) {
            return Err(String::from("image too large").into());
        }
        let (scaled_width, scaled_height) = imgutil::fit_to_width(width, height, max_pixel_width);
        if scaled_width == width && scaled_height == height {
            return Ok(PreparedImage {
                png_data: raw.to_vec(),
                width,
                height,
            });
        }
    }

    let mut image =
        imgutil::decode_with_limits(raw).map_err(|err| format!("failed to decode image: {err}"))?;
    let (width, height) = image.dimensions();
    let (scaled_width, scaled_height) = imgutil::fit_to_width(width, height, max_pixel_width);
    if scaled_width != width || scaled_height != height {
        image = imgutil::scale(&image, scaled_width, scaled_height);
    }
    let png_data =
        imgutil::encode_png(&image).map_err(|err| format!("failed to encode PNG: {err}"))?;
    Ok(PreparedImage {
        png_data,
        width: scaled_width,
        height: scaled_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgba};

    fn make_png(width: u32, height: u32) -> Vec<u8> {
        let image =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(width, height, Rgba([0, 0, 0, 255])));
        imgutil::encode_png(&image).unwrap()
    }

    #[test]
    fn prepare_image_invalid_data() {
        assert!(prepare_image(b"not an image", 800).is_err());
    }

    #[test]
    fn prepare_image_png_passthrough() {
        let png = make_png(100, 100);
        let prepared = prepare_image(&png, 800).unwrap();
        assert_eq!((prepared.width, prepared.height), (100, 100));
        assert_eq!(prepared.png_data, png);
    }

    #[test]
    fn prepare_image_png_scaling() {
        let png = make_png(1600, 1200);
        let prepared = prepare_image(&png, 800).unwrap();
        assert_eq!((prepared.width, prepared.height), (800, 600));
        assert!(imgutil::is_png(&prepared.png_data));
        assert_eq!(
            imgutil::png_dimensions(&prepared.png_data),
            Some((800, 600))
        );
        assert_ne!(prepared.png_data, png);
    }
}
