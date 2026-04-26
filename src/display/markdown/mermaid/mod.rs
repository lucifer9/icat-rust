use cosmic_text::FontSystem;
use image::DynamicImage;

use super::markie::{self, FontSystemMeasure};

pub fn render_mermaid(
    source: &str,
    font_system: &mut FontSystem,
    max_width: u32,
    font_size: f32,
) -> Result<DynamicImage, String> {
    let mut measure = FontSystemMeasure::new(font_system);
    let style = markie::mermaid::DiagramStyle {
        font_size: (font_size * 0.82).max(10.0),
        ..markie::mermaid::DiagramStyle::default()
    };
    let (fragment, width, height) = markie::mermaid::render_diagram(source, &style, &mut measure)?;
    let canvas_pad = 4.0;
    let inner_max_width = (max_width as f32 - canvas_pad * 2.0).max(1.0);
    let scale = if width > inner_max_width {
        (inner_max_width / width).max(0.2)
    } else {
        1.0
    };
    let canvas_w = (width * scale + canvas_pad * 2.0).ceil().max(1.0);
    let canvas_h = (height * scale + canvas_pad * 2.0).ceil().max(1.0);
    let body = format!(
        r#"<g transform="translate({canvas_pad:.2},{canvas_pad:.2}) scale({scale:.4})">{fragment}</g>"#
    );
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{canvas_w:.0}" height="{canvas_h:.0}" viewBox="0 0 {canvas_w:.0} {canvas_h:.0}"><rect width="100%" height="100%" fill="white"/>{body}</svg>"#
    );
    markie::svg_to_image(&svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn non_white_bounds(image: &DynamicImage) -> (u32, u32, u32, u32) {
        let rgba = image.to_rgba8();
        let mut found = false;
        let mut left = image.width();
        let mut top = image.height();
        let mut right = 0;
        let mut bottom = 0;

        for (x, y, pixel) in rgba.enumerate_pixels() {
            if pixel[3] <= 8 || (pixel[0] > 250 && pixel[1] > 250 && pixel[2] > 250) {
                continue;
            }
            found = true;
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
        }

        assert!(
            found,
            "Mermaid diagram should contain visible non-white pixels"
        );
        (left, top, right, bottom)
    }

    fn assert_visible_content_is_not_clipped(name: &str, image: &DynamicImage) {
        let (left, top, right, bottom) = non_white_bounds(image);

        assert!(
            left > 0,
            "{name} Mermaid content should not touch the left edge: bounds=({left},{top},{right},{bottom}), size={}x{}",
            image.width(),
            image.height()
        );
        assert!(
            top > 0,
            "{name} Mermaid content should not touch the top edge: bounds=({left},{top},{right},{bottom}), size={}x{}",
            image.width(),
            image.height()
        );
        assert!(
            right + 1 < image.width(),
            "{name} Mermaid content should not touch the right edge: bounds=({left},{top},{right},{bottom}), size={}x{}",
            image.width(),
            image.height()
        );
        assert!(
            bottom + 1 < image.height(),
            "{name} Mermaid content should not touch the bottom edge: bounds=({left},{top},{right},{bottom}), size={}x{}",
            image.width(),
            image.height()
        );
    }

    #[test]
    fn render_mermaid_diagrams_have_visible_unclipped_content() {
        let mut font_system = FontSystem::new();
        let diagrams = [
            (
                "flowchart",
                "flowchart TD\n    A[Start] --> B{Decision}\n    B -->|Yes| C[Continue]\n    B -->|No| D[Retry]\n",
            ),
            (
                "sequence",
                "sequenceDiagram\n    participant User\n    participant System\n    User->>System: Login Request\n    System-->>User: Login Success\n",
            ),
            (
                "class",
                "classDiagram\n    class Animal {\n        +String name\n        +makeSound()\n    }\n    class Dog {\n        +bark()\n    }\n    Animal <|-- Dog\n",
            ),
            (
                "state",
                "stateDiagram\n    [*] --> Idle\n    Idle --> Loading: Load Data\n    Loading --> Success\n    Success --> [*]\n",
            ),
            (
                "er",
                "erDiagram\n    CUSTOMER ||--o{ ORDER : places\n    ORDER ||--|{ LINE_ITEM : contains\n    CUSTOMER {\n        int id\n        string name\n    }\n",
            ),
        ];

        for (name, source) in diagrams {
            let image = render_mermaid(source, &mut font_system, 800, 18.0).unwrap();
            assert!(
                image.width() <= 800,
                "{name} Mermaid should fit requested width"
            );
            assert_visible_content_is_not_clipped(name, &image);
        }
    }
}
