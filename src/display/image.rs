use image::GenericImageView;

use crate::imgutil;
use crate::kitty;
use crate::term::Size;

pub fn image(path: &str, size: Size, tmux: bool) -> Result<(), Box<dyn std::error::Error>> {
    let raw =
        imgutil::read_source(path).map_err(|err| format!("failed to read image {path}: {err}"))?;
    image_from_bytes(&raw, size, tmux)
}

pub fn image_from_bytes(
    raw: &[u8],
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Fast path: unscaled PNG — send raw bytes directly, no copy needed
    if imgutil::is_png(raw)
        && let Some((width, height)) = imgutil::png_dimensions(raw)
        && imgutil::check_limits(width, height)
    {
        let (sw, sh) =
            imgutil::fit_within(width, height, size.pixel_width, size.image_area_height());
        if sw == width && sh == height {
            return kitty::send_static_image(raw, width, height, size, tmux);
        }
    }

    // Decode and optionally scale
    let mut image =
        imgutil::decode_with_limits(raw).map_err(|e| format!("failed to decode image: {e}"))?;
    let (width, height) = image.dimensions();
    let (sw, sh) = imgutil::fit_within(width, height, size.pixel_width, size.image_area_height());
    if sw != width || sh != height {
        image = imgutil::scale(&image, sw, sh);
    }

    // Send as RGBA + zlib (skips PNG filter selection + DEFLATE overhead)
    let zlib_data =
        imgutil::encode_rgba_zlib(&image).map_err(|e| format!("failed to encode RGBA: {e}"))?;
    kitty::send_static_image_rgba_zlib(&zlib_data, sw, sh, size, tmux)
}
