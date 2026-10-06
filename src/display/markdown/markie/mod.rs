pub(super) mod layout;
pub(super) mod math;
pub(super) mod mermaid;
pub(super) mod xml;

use std::sync::{Arc, OnceLock};

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, Weight};
use image::{DynamicImage, ImageBuffer, Rgba};
use resvg::usvg;
use tiny_skia::{Pixmap, Transform};

pub(super) trait TextMeasure {
    /// Advance width of `text` laid out on a single line.
    fn measure_width(
        &mut self,
        text: &str,
        font_size: f32,
        is_code: bool,
        is_bold: bool,
        is_italic: bool,
    ) -> f32;
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
    fn measure_width(
        &mut self,
        text: &str,
        font_size: f32,
        is_code: bool,
        is_bold: bool,
        is_italic: bool,
    ) -> f32 {
        let mut buffer = Buffer::new(self.font_system, Metrics::new(font_size, font_size * 1.2));
        buffer.set_size(None, None);
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
        buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0, f32::max)
    }
}

/// Deterministic measure for layout tests: every char is 0.6 em wide.
#[cfg(test)]
pub(super) struct MockMeasure;

#[cfg(test)]
impl TextMeasure for MockMeasure {
    fn measure_width(
        &mut self,
        text: &str,
        font_size: f32,
        _is_code: bool,
        _is_bold: bool,
        _is_italic: bool,
    ) -> f32 {
        text.chars().count() as f32 * font_size * 0.6
    }
}

// Loading system fonts can take hundreds of milliseconds; do it once and share
// the database across every SVG render (formulas, Mermaid diagrams, ...).
static SVG_FONTDB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();

pub(super) fn svg_to_image(svg: &str) -> Result<DynamicImage, String> {
    let fontdb = SVG_FONTDB
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone();
    let opts = usvg::Options {
        fontdb,
        ..usvg::Options::default()
    };

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
