#![allow(dead_code)]

pub(super) mod layout;
pub(super) mod math;
pub(super) mod mermaid;
pub(super) mod xml;

use std::path::Path;

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, Weight};
use image::{DynamicImage, ImageBuffer, Rgba};
use resvg::usvg;
use tiny_skia::{Pixmap, Transform};

pub(super) trait TextMeasure {
    fn measure_text(
        &mut self,
        text: &str,
        font_size: f32,
        is_code: bool,
        is_bold: bool,
        is_italic: bool,
        max_width: Option<f32>,
    ) -> (f32, f32);
}

pub(super) struct FontSystemMeasure<'a> {
    font_system: &'a mut FontSystem,
}

impl<'a> FontSystemMeasure<'a> {
    pub(super) fn new(font_system: &'a mut FontSystem) -> Self {
        Self { font_system }
    }
}

impl TextMeasure for FontSystemMeasure<'_> {
    fn measure_text(
        &mut self,
        text: &str,
        font_size: f32,
        is_code: bool,
        is_bold: bool,
        is_italic: bool,
        max_width: Option<f32>,
    ) -> (f32, f32) {
        let line_height = font_size * 1.2;
        let mut buffer = Buffer::new(self.font_system, Metrics::new(font_size, line_height));
        buffer.set_size(max_width, None);
        let attrs = Attrs::new()
            .family(if is_code {
                Family::Monospace
            } else {
                Family::SansSerif
            })
            .weight(if is_bold {
                Weight::BOLD
            } else {
                Weight::NORMAL
            })
            .style(if is_italic {
                Style::Italic
            } else {
                Style::Normal
            });
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(self.font_system, false);

        let mut width: f32 = 0.0;
        let mut height: f32 = 0.0;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            height += run.line_height;
        }
        (width, height.max(line_height))
    }
}

pub(super) fn svg_to_image(svg: &str) -> Result<DynamicImage, String> {
    let mut opts = usvg::Options::default();
    {
        let fontdb = opts.fontdb_mut();
        fontdb.load_system_fonts();
        let local_fonts = Path::new("fonts");
        if local_fonts.is_dir() {
            fontdb.load_fonts_dir(local_fonts);
        }
    }

    let tree =
        usvg::Tree::from_str(svg, &opts).map_err(|err| format!("failed to parse SVG: {err}"))?;
    let width = tree.size().width().ceil().max(1.0) as u32;
    let height = tree.size().height().ceil().max(1.0) as u32;
    let mut pixmap = Pixmap::new(width, height).ok_or_else(|| String::from("SVG too large"))?;
    resvg::render(&tree, Transform::identity(), &mut pixmap.as_mut());
    let image = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, pixmap.data().to_vec())
        .ok_or_else(|| String::from("failed to build SVG image"))?;
    Ok(DynamicImage::ImageRgba8(image))
}
