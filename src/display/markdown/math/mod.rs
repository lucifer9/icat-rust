use cosmic_text::FontSystem;
use image::DynamicImage;

use super::markie::{self, FontSystemMeasure};

pub struct RenderedMath {
    pub image: DynamicImage,
    pub baseline: u32,
}

pub fn render_math(
    latex: &str,
    font_system: &mut FontSystem,
    font_size: f32,
    display: bool,
) -> Result<RenderedMath, String> {
    let mut measure = FontSystemMeasure::new(font_system);
    let size = if display { font_size * 1.15 } else { font_size };
    let result = markie::math::render_math(latex, size, "#141414", &mut measure, display)?;
    let pad = if display { 12.0 } else { 4.0 };
    let width = (result.width + pad * 2.0).ceil().max(1.0);
    let height = (result.ascent + result.descent + pad * 2.0).ceil().max(1.0);
    let draw_baseline = (pad + result.ascent).ceil().max(1.0);
    let baseline = if display {
        draw_baseline
    } else {
        ((height * 0.5) + font_size * 0.2).ceil().clamp(1.0, height)
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}"><g transform="translate({pad:.2}, {draw_baseline:.2})">{}</g></svg>"#,
        result.svg_fragment
    );
    let image = markie::svg_to_image(&svg)?;
    Ok(RenderedMath {
        image,
        baseline: baseline as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_bounds(image: &DynamicImage) -> (u32, u32, u32, u32) {
        let rgba = image.to_rgba8();
        let mut found = false;
        let mut left = image.width();
        let mut top = image.height();
        let mut right = 0;
        let mut bottom = 0;

        for (x, y, pixel) in rgba.enumerate_pixels() {
            if pixel[3] <= 8 {
                continue;
            }
            found = true;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
        }

        assert!(found, "rendered math should contain visible ink");
        (left, top, right, bottom)
    }

    fn assert_ink_has_padding(name: &str, image: &DynamicImage) {
        let (left, top, right, bottom) = alpha_bounds(image);

        assert!(
            left > 0,
            "{name} math ink should not touch the left edge: bounds=({left},{top},{right},{bottom}), size={}x{}",
            image.width(),
            image.height()
        );
        assert!(
            top > 0,
            "{name} math ink should not touch the top edge: bounds=({left},{top},{right},{bottom}), size={}x{}",
            image.width(),
            image.height()
        );
        assert!(
            right + 1 < image.width(),
            "{name} math ink should not touch the right edge: bounds=({left},{top},{right},{bottom}), size={}x{}",
            image.width(),
            image.height()
        );
        assert!(
            bottom + 1 < image.height(),
            "{name} math ink should not touch the bottom edge: bounds=({left},{top},{right},{bottom}), size={}x{}",
            image.width(),
            image.height()
        );
    }

    #[test]
    fn renders_markie_matrix() {
        let mut font_system = FontSystem::new();
        let rendered = render_math(
            r"\begin{bmatrix}a & b \\ c & d\end{bmatrix}",
            &mut font_system,
            18.0,
            true,
        )
        .unwrap();
        assert!(rendered.image.width() > 20);
        assert!(rendered.image.height() > 20);
    }

    #[test]
    fn inline_math_baselines_stay_near_visual_midline() {
        let mut font_system = FontSystem::new();
        let font_size = 18.0;
        let cases = [
            ("square root", r"\sqrt{x^2 + y^2}"),
            ("fraction", r"\frac{a + b}{c + d}"),
            ("binomial", r"\binom{n}{k}"),
            ("sum with limits", r"\sum_{i=1}^{n} i"),
            ("subscript and superscript", r"x_i^2 + y_j^3"),
        ];

        for (name, latex) in cases {
            let rendered = render_math(latex, &mut font_system, font_size, false).unwrap();
            let midpoint = rendered.image.height() as f32 / 2.0;
            let min_baseline = midpoint - font_size * 0.15;
            let max_baseline = midpoint + font_size * 0.28;

            assert!(rendered.baseline > 0);
            assert!(rendered.baseline <= rendered.image.height());
            assert!(
                rendered.baseline as f32 >= min_baseline,
                "{name} inline baseline {} should not sit far above image midpoint {}",
                rendered.baseline,
                midpoint
            );
            assert!(
                rendered.baseline as f32 <= max_baseline,
                "{name} inline baseline {} should stay near image midpoint {}",
                rendered.baseline,
                midpoint
            );
        }
    }

    #[test]
    fn inline_sqrt_baseline_is_visually_centered() {
        let mut font_system = FontSystem::new();
        let font_size = 18.0;
        let rendered =
            render_math(r"\sqrt{x^2 + y^2}", &mut font_system, font_size, false).unwrap();
        let midpoint = rendered.image.height() as f32 / 2.0;
        let max_baseline = midpoint + font_size * 0.25;

        assert!(
            rendered.baseline as f32 <= max_baseline,
            "inline sqrt baseline {} should stay near image midpoint {}",
            rendered.baseline,
            midpoint
        );
    }

    #[test]
    fn rendered_math_keeps_visible_ink_inside_canvas() {
        let mut font_system = FontSystem::new();
        let cases = [
            ("inline sqrt", r"\sqrt{x^2 + y^2}", false),
            ("inline fraction", r"\frac{a + b}{c + d}", false),
            (
                "display matrix",
                r"\begin{bmatrix}a & b \\ c & d\end{bmatrix}",
                true,
            ),
            (
                "display cases",
                r"\begin{cases} x + 1 & x > 0 \\ x - 1 & x \leq 0 \end{cases}",
                true,
            ),
        ];

        for (name, latex, display) in cases {
            let rendered = render_math(latex, &mut font_system, 18.0, display).unwrap();
            assert_ink_has_padding(name, &rendered.image);
        }
    }
}
