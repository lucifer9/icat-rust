use crate::display::markdown::markie::TextMeasure;
use crate::display::markdown::markie::xml::{escape_xml, sanitize_xml_text};
use latex2mathml::{DisplayStyle, latex_to_mathml};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event as XmlEvent};
use quick_xml::reader::Reader as XmlReader;

#[derive(Debug)]
enum MathNode {
    Row(Vec<MathNode>),
    Ident(String),
    Number(String),
    Operator(String),
    Text(String),
    Sup {
        base: Box<MathNode>,
        sup: Box<MathNode>,
    },
    Sub {
        base: Box<MathNode>,
        sub: Box<MathNode>,
    },
    SubSup {
        base: Box<MathNode>,
        sub: Box<MathNode>,
        sup: Box<MathNode>,
    },
    Frac {
        num: Box<MathNode>,
        den: Box<MathNode>,
        has_rule: bool,
    },
    Sqrt {
        radicand: Box<MathNode>,
    },
    Root {
        radicand: Box<MathNode>,
        index: Box<MathNode>,
    },
    UnderOver {
        base: Box<MathNode>,
        under: Option<Box<MathNode>>,
        over: Option<Box<MathNode>>,
    },
    Space(f32),
    /// Table for matrices, cases, aligned equations
    Table {
        rows: Vec<Vec<MathNode>>,
    },
    /// Stretchy operator (parentheses, brackets that scale)
    StretchyOp {
        op: String,
        form: String, // "prefix", "postfix", "infix"
    },
}

#[derive(Debug)]
pub struct MathResult {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub svg_fragment: String,
}

pub fn render_math<T: TextMeasure>(
    latex: &str,
    font_size: f32,
    text_color: &str,
    measure: &mut T,
    display: bool,
) -> Result<MathResult, String> {
    let latex = preprocess_latex(latex);
    let style = if display {
        DisplayStyle::Block
    } else {
        DisplayStyle::Inline
    };

    let mathml =
        latex_to_mathml(&latex, style).map_err(|e| format!("LaTeX parse error: {:?}", e))?;

    let root = parse_mathml(&mathml)?;
    let mbox = layout_node(&root, font_size, text_color, measure, 0.0, 0.0);

    Ok(MathResult {
        width: mbox.width,
        ascent: mbox.ascent,
        descent: mbox.descent,
        svg_fragment: mbox.svg,
    })
}

/// Map unsupported LaTeX environments to supported equivalents for latex2mathml.
///
/// These replacements are safe from false substring matches because `\begin{` and
/// `\end{` are LaTeX command sequences that won't appear as arbitrary substrings
/// in well-formed LaTeX input.
fn preprocess_latex(latex: &str) -> String {
    let mut result = String::with_capacity(latex.len());

    // aligned → align (supported by latex2mathml)
    // cases → \left\{ + matrix + \right. (preserves the semantic left curly brace)
    let latex = latex.replace("\\begin{aligned}", "\\begin{align}");
    let latex = latex.replace("\\end{aligned}", "\\end{align}");
    let latex = latex.replace("\\begin{cases}", "\\left\\{\\begin{matrix}");
    let latex = latex.replace("\\end{cases}", "\\end{matrix}\\right.");

    // Single-pass scan for \begin{array}{...} → \begin{matrix}
    let begin_array = "\\begin{array}";
    let mut rest = latex.as_str();

    while let Some(pos) = rest.find(begin_array) {
        result.push_str(&rest[..pos]);
        result.push_str("\\begin{matrix}");
        rest = &rest[pos + begin_array.len()..];
        // Skip the optional column-alignment spec: {cc}, {l|r}, etc.
        if rest.starts_with('{') {
            let mut depth = 0;
            let mut end = rest.len();
            for (i, ch) in rest.char_indices() {
                if ch == '{' {
                    depth += 1;
                } else if ch == '}' {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
            }
            rest = &rest[end..];
        }
    }
    result.push_str(rest);

    result = result.replace("\\end{array}", "\\end{matrix}");

    result
}

struct MathBox {
    width: f32,
    ascent: f32,
    descent: f32,
    svg: String,
}

type Attrs = Vec<(String, String)>;

fn parse_mathml_attrs(element: &BytesStart<'_>) -> Result<Attrs, String> {
    element
        .attributes()
        // latex2mathml emits a few legacy unquoted attributes such as
        // `columnalign=left`; ignore only those malformed attributes while
        // normalizing every valid attribute through quick-xml.
        .filter_map(Result::ok)
        .map(|attribute| {
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|err| format!("XML attribute decode error: {err}"))?;
            Ok((attribute.key.as_ref().to_string(), value.into_owned()))
        })
        .collect()
}

fn push_mathml_text(stack: &mut [(String, Vec<MathNode>, Attrs)], text: String) {
    if !text.is_empty()
        && let Some((_, children, _)) = stack.last_mut()
    {
        children.push(MathNode::Text(text));
    }
}

fn parse_mathml(mathml: &str) -> Result<MathNode, String> {
    let mut reader = XmlReader::from_str(mathml);
    reader.config_mut().trim_text(true);

    // Stack now stores: (tag_name, children, attributes)
    let mut stack: Vec<(String, Vec<MathNode>, Attrs)> = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(ref e)) => {
                let name = e.name().as_ref().to_string();
                let attrs = parse_mathml_attrs(e)?;
                stack.push((name, Vec::new(), attrs));
            }
            Ok(XmlEvent::Text(ref e)) => {
                let text = e.xml_content(XmlVersion::Implicit1_0);
                push_mathml_text(&mut stack, text.into_owned());
            }
            Ok(XmlEvent::CData(ref e)) => {
                let text = e.xml_content(XmlVersion::Implicit1_0);
                push_mathml_text(&mut stack, text.into_owned());
            }
            Ok(XmlEvent::GeneralRef(ref e)) => {
                let reference = e.xml_content(XmlVersion::Implicit1_0);
                let encoded = format!("&{reference};");
                let text = quick_xml::escape::unescape(&encoded)
                    .map_err(|err| format!("XML reference error: {err}"))?;
                push_mathml_text(&mut stack, text.into_owned());
            }
            Ok(XmlEvent::End(_)) => {
                if let Some((tag, children, attrs)) = stack.pop() {
                    let node = build_node(&tag, children, &attrs);
                    if let Some((_, parent_children, _)) = stack.last_mut() {
                        parent_children.push(node);
                    } else {
                        return Ok(node);
                    }
                }
            }
            Ok(XmlEvent::Empty(ref e)) => {
                let name = e.name().as_ref().to_string();
                let attrs = parse_mathml_attrs(e)?;

                if name == "mspace" {
                    let mut width_em = 0.0;
                    for (key, val) in &attrs {
                        if key == "width"
                            && let Some(stripped) = val.strip_suffix("em")
                        {
                            width_em = stripped.parse().unwrap_or(0.0);
                        }
                    }
                    let node = MathNode::Space(width_em);
                    if let Some((_, parent_children, _)) = stack.last_mut() {
                        parent_children.push(node);
                    }
                }
            }
            Ok(XmlEvent::Eof) => {
                if let Some((tag, _, _)) = stack.last() {
                    return Err(format!("XML parse error: unclosed <{tag}> element"));
                }
                break;
            }
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Row(Vec::new()))
}

fn get_attr(attrs: &[(String, String)], name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

fn build_node(tag: &str, children: Vec<MathNode>, attrs: &Attrs) -> MathNode {
    match tag {
        "mi" => MathNode::Ident(extract_text(&children)),
        "mn" => MathNode::Number(extract_text(&children)),
        "mo" => {
            let text = extract_text(&children);
            let stretchy = get_attr(attrs, "stretchy")
                .map(|v| v == "true")
                .unwrap_or(false);
            let form = get_attr(attrs, "form").unwrap_or_else(|| "infix".to_string());

            if stretchy && !text.is_empty() {
                MathNode::StretchyOp { op: text, form }
            } else {
                MathNode::Operator(text)
            }
        }
        "mtext" => MathNode::Text(extract_text(&children)),
        "msup" => with_children(children, |[base, sup]| MathNode::Sup {
            base: Box::new(base),
            sup: Box::new(sup),
        }),
        "msub" => with_children(children, |[base, sub]| MathNode::Sub {
            base: Box::new(base),
            sub: Box::new(sub),
        }),
        "msubsup" => with_children(children, |[base, sub, sup]| MathNode::SubSup {
            base: Box::new(base),
            sub: Box::new(sub),
            sup: Box::new(sup),
        }),
        "mfrac" => with_children(children, |[num, den]| MathNode::Frac {
            num: Box::new(num),
            den: Box::new(den),
            // Binomials are fractions with `linethickness="0"`.
            has_rule: get_attr(attrs, "linethickness").and_then(|v| v.parse::<f32>().ok())
                != Some(0.0),
        }),
        "msqrt" => MathNode::Sqrt {
            radicand: Box::new(row_or_single(children)),
        },
        // In MathML `mroot` the index comes after the radicand.
        "mroot" => with_children(children, |[radicand, index]| MathNode::Root {
            radicand: Box::new(radicand),
            index: Box::new(index),
        }),
        "mover" => with_children(children, |[base, over]| MathNode::UnderOver {
            base: Box::new(base),
            under: None,
            over: Some(Box::new(over)),
        }),
        "munder" => with_children(children, |[base, under]| MathNode::UnderOver {
            base: Box::new(base),
            under: Some(Box::new(under)),
            over: None,
        }),
        "munderover" => with_children(children, |[base, under, over]| MathNode::UnderOver {
            base: Box::new(base),
            under: Some(Box::new(under)),
            over: Some(Box::new(over)),
        }),
        // Keep every `mtd` cell as its own column, even in a one-cell row.
        "mtr" => MathNode::Row(children),
        "mtable" => MathNode::Table {
            // Each child is a built `mtr`; rows without cells add no content.
            rows: children
                .into_iter()
                .filter_map(|row| match row {
                    MathNode::Row(cells) if !cells.is_empty() => Some(cells),
                    _ => None,
                })
                .collect(),
        },
        _ => row_or_single(children),
    }
}

/// Builds a fixed-arity element such as `msup`; any other child count is laid
/// out as a plain row.
fn with_children<const N: usize>(
    children: Vec<MathNode>,
    build: impl FnOnce([MathNode; N]) -> MathNode,
) -> MathNode {
    match <[MathNode; N]>::try_from(children) {
        Ok(parts) => build(parts),
        Err(children) => row_or_single(children),
    }
}

fn row_or_single(children: Vec<MathNode>) -> MathNode {
    match <[MathNode; 1]>::try_from(children) {
        Ok([only]) => only,
        Err(children) => MathNode::Row(children),
    }
}

fn extract_text(children: &[MathNode]) -> String {
    let mut s = String::new();
    for child in children {
        match child {
            MathNode::Text(t)
            | MathNode::Ident(t)
            | MathNode::Number(t)
            | MathNode::Operator(t) => s.push_str(t),
            _ => {}
        }
    }
    s
}

/// Default ascent and descent of a text run, as fractions of its font size.
const ASCENT_RATIO: f32 = 0.75;
const DESCENT_RATIO: f32 = 0.25;

fn measure_token<T: TextMeasure>(text: &str, font_size: f32, italic: bool, measure: &mut T) -> f32 {
    let cleaned = sanitize_xml_text(text);
    measure.measure_width(&cleaned, font_size, false, false, italic)
}

#[derive(Clone, Copy)]
enum MathFont {
    Serif,
    SerifItalic,
    SansSerif,
}

/// Lays out one text run on the baseline with the default ascent and descent.
fn text_box<T: TextMeasure>(
    text: &str,
    font: MathFont,
    font_size: f32,
    color: &str,
    measure: &mut T,
    x: f32,
    baseline_y: f32,
) -> MathBox {
    let (family, style) = match font {
        MathFont::Serif => ("serif", ""),
        MathFont::SerifItalic => ("serif", " font-style=\"italic\""),
        MathFont::SansSerif => ("sans-serif", ""),
    };
    let italic = matches!(font, MathFont::SerifItalic);
    let width = measure_token(text, font_size, italic, measure);
    let svg = format!(
        r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.2}" fill="{}"{}>{}</text>"#,
        x,
        baseline_y,
        family,
        font_size,
        color,
        style,
        escape_xml(text)
    );
    MathBox {
        width,
        ascent: font_size * ASCENT_RATIO,
        descent: font_size * DESCENT_RATIO,
        svg,
    }
}

fn layout_node<T: TextMeasure>(
    node: &MathNode,
    font_size: f32,
    color: &str,
    measure: &mut T,
    x: f32,
    baseline_y: f32,
) -> MathBox {
    match node {
        MathNode::Ident(text) => {
            let italic = text.len() == 1 && text.chars().next().is_some_and(|c| c.is_alphabetic());
            let font = if italic {
                MathFont::SerifItalic
            } else {
                MathFont::Serif
            };
            text_box(text, font, font_size, color, measure, x, baseline_y)
        }
        MathNode::Number(text) => text_box(
            text,
            MathFont::Serif,
            font_size,
            color,
            measure,
            x,
            baseline_y,
        ),
        MathNode::Operator(text) => {
            let is_large = is_large_operator(text);
            let effective_size = if is_large { font_size * 1.4 } else { font_size };
            let spacing = font_size * 0.15;
            let y_offset = if is_large {
                baseline_y + (effective_size - font_size) * 0.2
            } else {
                baseline_y
            };
            let mut op_box = text_box(
                text,
                MathFont::Serif,
                effective_size,
                color,
                measure,
                x + spacing,
                y_offset,
            );
            op_box.width += spacing * 2.0;
            if is_large {
                op_box.ascent = effective_size * 0.8;
                op_box.descent = effective_size * 0.3;
            }
            op_box
        }
        MathNode::Text(text) => text_box(
            text,
            MathFont::SansSerif,
            font_size,
            color,
            measure,
            x,
            baseline_y,
        ),
        MathNode::Space(em) => MathBox {
            width: font_size * em,
            ascent: 0.0,
            descent: 0.0,
            svg: String::new(),
        },
        MathNode::Row(children) => layout_row(children, font_size, color, measure, x, baseline_y),
        MathNode::Sup { base, sup } => {
            let base_box = layout_node(base, font_size, color, measure, x, baseline_y);

            let sup_size = font_size * 0.7;
            let sup_y = baseline_y - base_box.ascent * 0.55;
            let sup_box = layout_node(sup, sup_size, color, measure, x + base_box.width, sup_y);

            let total_width = base_box.width + sup_box.width;
            let ascent = base_box.ascent.max(sup_box.ascent + base_box.ascent * 0.55);
            let descent = base_box.descent;

            MathBox {
                width: total_width,
                ascent,
                descent,
                svg: format!("{}{}", base_box.svg, sup_box.svg),
            }
        }
        MathNode::Sub { base, sub } => {
            let base_box = layout_node(base, font_size, color, measure, x, baseline_y);

            let sub_size = font_size * 0.7;
            let sub_y = baseline_y + base_box.descent + sub_size * 0.35;
            let sub_box = layout_node(sub, sub_size, color, measure, x + base_box.width, sub_y);

            let total_width = base_box.width + sub_box.width;
            let ascent = base_box.ascent;
            let descent =
                (base_box.descent + sub_size * 0.35 + sub_box.descent).max(base_box.descent);

            MathBox {
                width: total_width,
                ascent,
                descent,
                svg: format!("{}{}", base_box.svg, sub_box.svg),
            }
        }
        MathNode::SubSup { base, sub, sup } => {
            let base_box = layout_node(base, font_size, color, measure, x, baseline_y);

            let script_size = font_size * 0.7;

            let sup_y = baseline_y - base_box.ascent * 0.55;
            let sup_box = layout_node(sup, script_size, color, measure, x + base_box.width, sup_y);

            let sub_y = baseline_y + base_box.descent + script_size * 0.35;
            let sub_box = layout_node(sub, script_size, color, measure, x + base_box.width, sub_y);

            let script_width = sup_box.width.max(sub_box.width);
            let total_width = base_box.width + script_width;
            let ascent = base_box.ascent.max(sup_box.ascent + base_box.ascent * 0.55);
            let descent =
                (base_box.descent + script_size * 0.35 + sub_box.descent).max(base_box.descent);

            MathBox {
                width: total_width,
                ascent,
                descent,
                svg: format!("{}{}{}", base_box.svg, sup_box.svg, sub_box.svg),
            }
        }
        MathNode::UnderOver { base, under, over } => layout_underover(
            base,
            under.as_deref(),
            over.as_deref(),
            &mut UnderoverContext {
                font_size,
                color,
                measure,
                x,
                baseline_y,
            },
        ),
        MathNode::Frac { num, den, has_rule } => {
            let frac_size = font_size * 0.85;

            let num_box = layout_node(num, frac_size, color, measure, 0.0, 0.0);
            let den_box = layout_node(den, frac_size, color, measure, 0.0, 0.0);

            let max_width = num_box.width.max(den_box.width);
            let padding = font_size * 0.2;
            let frac_width = max_width + padding * 2.0;

            let rule_y = baseline_y - font_size * 0.3;
            let gap = font_size * 0.15;

            let num_baseline = rule_y - gap - num_box.descent;
            let den_baseline = rule_y + gap + den_box.ascent;

            let num_x = x + (frac_width - num_box.width) / 2.0;
            let den_x = x + (frac_width - den_box.width) / 2.0;

            let num_rendered = layout_node(num, frac_size, color, measure, num_x, num_baseline);
            let den_rendered = layout_node(den, frac_size, color, measure, den_x, den_baseline);

            let rule_svg = if *has_rule {
                format!(
                    r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1" />"#,
                    x,
                    rule_y,
                    x + frac_width,
                    rule_y,
                    color
                )
            } else {
                String::new()
            };

            let ascent =
                (baseline_y - num_baseline + num_rendered.ascent).max(font_size * ASCENT_RATIO);
            let descent =
                (den_baseline - baseline_y + den_rendered.descent).max(font_size * DESCENT_RATIO);

            MathBox {
                width: frac_width,
                ascent,
                descent,
                svg: format!("{}{}{}", num_rendered.svg, rule_svg, den_rendered.svg),
            }
        }
        MathNode::Sqrt { radicand } => {
            let inner = layout_node(radicand, font_size, color, measure, 0.0, 0.0);

            let radical_width = font_size * 0.6;
            let padding = font_size * 0.1;
            let overbar_gap = font_size * 0.15;
            let total_width = radical_width + inner.width + padding;

            let inner_box = layout_node(
                radicand,
                font_size,
                color,
                measure,
                x + radical_width,
                baseline_y,
            );

            let top_y = baseline_y - inner_box.ascent - overbar_gap;
            let bottom_y = baseline_y + inner_box.descent;

            let radical_svg = format!(
                r#"<path d="M {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2}" stroke="{}" stroke-width="1.2" fill="none" />"#,
                x,
                baseline_y - font_size * 0.15,
                x + radical_width * 0.35,
                baseline_y,
                x + radical_width * 0.6,
                top_y,
                x + radical_width + inner_box.width + padding,
                top_y,
                color
            );

            let ascent = (baseline_y - top_y).max(inner_box.ascent + overbar_gap);
            let descent = inner_box.descent.max(bottom_y - baseline_y);

            MathBox {
                width: total_width,
                ascent,
                descent,
                svg: format!("{}{}", radical_svg, inner_box.svg),
            }
        }
        MathNode::Root { radicand, index } => {
            let inner = layout_node(radicand, font_size, color, measure, 0.0, 0.0);
            let index_size = font_size * 0.6;

            let radical_width = font_size * 0.6;
            let index_width = font_size * 0.5;
            let padding = font_size * 0.1;
            let overbar_gap = font_size * 0.15;
            let total_width = index_width + radical_width + inner.width + padding;

            let inner_box = layout_node(
                radicand,
                font_size,
                color,
                measure,
                x + index_width + radical_width,
                baseline_y,
            );

            let top_y = baseline_y - inner_box.ascent - overbar_gap;
            let bottom_y = baseline_y + inner_box.descent;

            // Render the index (nth root degree) in the notch
            let index_baseline = baseline_y - inner_box.ascent * 0.3;
            let index_box = layout_node(index, index_size, color, measure, x, index_baseline);

            // Radical symbol with notch for index
            let radical_svg = format!(
                r#"<path d="M {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2}" stroke="{}" stroke-width="1.2" fill="none" />"#,
                x + index_width,
                baseline_y - font_size * 0.15,
                x + index_width + radical_width * 0.35,
                baseline_y,
                x + index_width + radical_width * 0.6,
                top_y,
                x + index_width + radical_width + inner_box.width + padding,
                top_y,
                x + index_width + radical_width + inner_box.width + padding - font_size * 0.1,
                top_y - font_size * 0.05,
                color
            );

            let ascent = (baseline_y - top_y).max(inner_box.ascent + overbar_gap);
            let descent = inner_box.descent.max(bottom_y - baseline_y);

            MathBox {
                width: total_width,
                ascent,
                descent,
                svg: format!("{}{}{}", index_box.svg, radical_svg, inner_box.svg),
            }
        }
        MathNode::Table { rows } => layout_table(rows, font_size, color, measure, x, baseline_y),
        MathNode::StretchyOp { op, form } => {
            layout_regular_stretchy_operator(op, form, font_size, color, measure, x, baseline_y)
        }
    }
}

fn layout_row<T: TextMeasure>(
    children: &[MathNode],
    font_size: f32,
    color: &str,
    measure: &mut T,
    start_x: f32,
    baseline_y: f32,
) -> MathBox {
    let mut target_ascent: f32 = font_size * ASCENT_RATIO;
    let mut target_descent: f32 = font_size * DESCENT_RATIO;

    for child in children {
        if matches!(child, MathNode::StretchyOp { .. }) {
            continue;
        }
        let child_box = layout_node(child, font_size, color, measure, 0.0, 0.0);
        target_ascent = target_ascent.max(child_box.ascent);
        target_descent = target_descent.max(child_box.descent);
    }

    let mut cx = start_x;
    let mut svg = String::new();
    let mut max_ascent: f32 = font_size * ASCENT_RATIO;
    let mut max_descent: f32 = font_size * DESCENT_RATIO;

    for child in children {
        let child_box = match child {
            MathNode::StretchyOp { op, form } => {
                let layout = StretchedDelimiterLayout {
                    font_size,
                    color,
                    x: cx,
                    baseline_y,
                    target_ascent,
                    target_descent,
                };
                layout_stretched_delimiter(op, form, layout, measure)
            }
            _ => layout_node(child, font_size, color, measure, cx, baseline_y),
        };
        max_ascent = max_ascent.max(child_box.ascent);
        max_descent = max_descent.max(child_box.descent);
        cx += child_box.width;
        svg.push_str(&child_box.svg);
    }

    MathBox {
        width: cx - start_x,
        ascent: max_ascent,
        descent: max_descent,
        svg,
    }
}

struct StretchedDelimiterLayout<'a> {
    font_size: f32,
    color: &'a str,
    x: f32,
    baseline_y: f32,
    target_ascent: f32,
    target_descent: f32,
}

fn layout_stretched_delimiter<T: TextMeasure>(
    op: &str,
    form: &str,
    layout: StretchedDelimiterLayout<'_>,
    measure: &mut T,
) -> MathBox {
    let StretchedDelimiterLayout {
        font_size,
        color,
        x,
        baseline_y,
        target_ascent,
        target_descent,
    } = layout;

    if op == "." {
        return MathBox {
            width: 0.0,
            ascent: 0.0,
            descent: 0.0,
            svg: String::new(),
        };
    }

    let height = target_ascent + target_descent;
    if height <= font_size * 1.35 || !is_supported_stretched_delimiter(op) {
        return layout_regular_stretchy_operator(
            op, form, font_size, color, measure, x, baseline_y,
        );
    }

    let width = stretched_delimiter_width(op, font_size);
    let stroke_width = (font_size * 0.075).clamp(1.0, 2.0);
    let top = baseline_y - target_ascent;
    let bottom = baseline_y + target_descent;
    let mid = baseline_y + (target_descent - target_ascent) * 0.08;
    let left = x + stroke_width;
    let right = x + width - stroke_width;
    let d = match op {
        "[" => format!(
            "M {right:.2} {top:.2} L {left:.2} {top:.2} L {left:.2} {bottom:.2} L {right:.2} {bottom:.2}"
        ),
        "]" => format!(
            "M {left:.2} {top:.2} L {right:.2} {top:.2} L {right:.2} {bottom:.2} L {left:.2} {bottom:.2}"
        ),
        "(" => format!(
            "M {right:.2} {top:.2} C {left:.2} {:.2} {left:.2} {:.2} {right:.2} {bottom:.2}",
            top + height * 0.24,
            bottom - height * 0.24
        ),
        ")" => format!(
            "M {left:.2} {top:.2} C {right:.2} {:.2} {right:.2} {:.2} {left:.2} {bottom:.2}",
            top + height * 0.24,
            bottom - height * 0.24
        ),
        "{" => format!(
            "M {right:.2} {top:.2} C {left:.2} {top:.2} {left:.2} {:.2} {:.2} {:.2} C {:.2} {:.2} {left:.2} {:.2} {right:.2} {bottom:.2}",
            mid - height * 0.16,
            x + width * 0.48,
            mid,
            x + width * 0.48,
            mid,
            mid + height * 0.16
        ),
        "}" => format!(
            "M {left:.2} {top:.2} C {right:.2} {top:.2} {right:.2} {:.2} {:.2} {:.2} C {:.2} {:.2} {right:.2} {:.2} {left:.2} {bottom:.2}",
            mid - height * 0.16,
            x + width * 0.52,
            mid,
            x + width * 0.52,
            mid,
            mid + height * 0.16
        ),
        "|" | "‖" => format!(
            "M {:.2} {top:.2} L {:.2} {bottom:.2}",
            x + width / 2.0,
            x + width / 2.0
        ),
        _ => {
            return layout_regular_stretchy_operator(
                op, form, font_size, color, measure, x, baseline_y,
            );
        }
    };

    MathBox {
        width,
        ascent: target_ascent,
        descent: target_descent,
        svg: format!(
            r#"<path d="{}" stroke="{}" stroke-width="{:.2}" stroke-linecap="round" stroke-linejoin="round" fill="none" />"#,
            d, color, stroke_width
        ),
    }
}

fn layout_regular_stretchy_operator<T: TextMeasure>(
    op: &str,
    form: &str,
    font_size: f32,
    color: &str,
    measure: &mut T,
    x: f32,
    baseline_y: f32,
) -> MathBox {
    let offset = match form {
        "prefix" => font_size * 0.08,
        "postfix" => -font_size * 0.08,
        _ => 0.0,
    };
    text_box(
        op,
        MathFont::Serif,
        font_size,
        color,
        measure,
        x + offset,
        baseline_y,
    )
}

fn is_supported_stretched_delimiter(op: &str) -> bool {
    matches!(op, "[" | "]" | "(" | ")" | "{" | "}" | "|" | "‖")
}

fn stretched_delimiter_width(op: &str, font_size: f32) -> f32 {
    match op {
        "{" | "}" => font_size * 0.55,
        "(" | ")" => font_size * 0.48,
        "[" | "]" | "|" | "‖" => font_size * 0.42,
        _ => font_size * 0.45,
    }
}

struct UnderoverContext<'a, T: TextMeasure> {
    font_size: f32,
    color: &'a str,
    measure: &'a mut T,
    x: f32,
    baseline_y: f32,
}

fn layout_underover<T: TextMeasure>(
    base: &MathNode,
    under: Option<&MathNode>,
    over: Option<&MathNode>,
    ctx: &mut UnderoverContext<'_, T>,
) -> MathBox {
    let font_size = ctx.font_size;
    let color = ctx.color;
    let measure = &mut *ctx.measure;
    let x = ctx.x;
    let baseline_y = ctx.baseline_y;

    let base_box = layout_node(base, font_size, color, measure, 0.0, 0.0);
    let script_size = font_size * 0.65;
    let gap = font_size * 0.15;

    let over_box = over.map(|o| layout_node(o, script_size, color, measure, 0.0, 0.0));
    let under_box = under.map(|u| layout_node(u, script_size, color, measure, 0.0, 0.0));

    let max_width = [
        base_box.width,
        over_box.as_ref().map_or(0.0, |b| b.width),
        under_box.as_ref().map_or(0.0, |b| b.width),
    ]
    .into_iter()
    .fold(0.0f32, f32::max);

    let mut svg = String::new();
    let mut total_ascent = base_box.ascent;
    let mut total_descent = base_box.descent;

    let base_x = x + (max_width - base_box.width) / 2.0;
    let base_rendered = layout_node(base, font_size, color, measure, base_x, baseline_y);
    svg.push_str(&base_rendered.svg);

    if let (Some(over_node), Some(ob)) = (over, &over_box) {
        let over_baseline = baseline_y - base_box.ascent - gap - ob.descent;
        let over_x = x + (max_width - ob.width) / 2.0;
        let over_rendered = layout_node(
            over_node,
            script_size,
            color,
            measure,
            over_x,
            over_baseline,
        );
        svg.push_str(&over_rendered.svg);
        total_ascent = base_box.ascent + gap + ob.ascent + ob.descent;
    }

    if let (Some(under_node), Some(ub)) = (under, &under_box) {
        let under_baseline = baseline_y + base_box.descent + gap + ub.ascent;
        let under_x = x + (max_width - ub.width) / 2.0;
        let under_rendered = layout_node(
            under_node,
            script_size,
            color,
            measure,
            under_x,
            under_baseline,
        );
        svg.push_str(&under_rendered.svg);
        total_descent = base_box.descent + gap + ub.ascent + ub.descent;
    }

    MathBox {
        width: max_width,
        ascent: total_ascent,
        descent: total_descent,
        svg,
    }
}

fn layout_table<T: TextMeasure>(
    rows: &[Vec<MathNode>],
    font_size: f32,
    color: &str,
    measure: &mut T,
    x: f32,
    baseline_y: f32,
) -> MathBox {
    if rows.is_empty() {
        return MathBox {
            width: 0.0,
            ascent: font_size * ASCENT_RATIO,
            descent: font_size * DESCENT_RATIO,
            svg: String::new(),
        };
    }

    let cell_size = font_size * 0.9;
    let row_gap = font_size * 0.3;
    let col_gap = font_size * 0.4;

    // First pass: measure all cells to determine column widths and row heights
    let mut col_widths: Vec<f32> = Vec::new();
    let mut row_heights: Vec<(f32, f32)> = Vec::new(); // (ascent, descent) per row

    for row in rows {
        let mut row_ascent = cell_size * ASCENT_RATIO;
        let mut row_descent = cell_size * DESCENT_RATIO;

        for (col_idx, cell) in row.iter().enumerate() {
            let cell_box = layout_node(cell, cell_size, color, measure, 0.0, 0.0);

            // Expand column width if needed
            while col_widths.len() <= col_idx {
                col_widths.push(0.0);
            }
            col_widths[col_idx] = col_widths[col_idx].max(cell_box.width);

            row_ascent = row_ascent.max(cell_box.ascent);
            row_descent = row_descent.max(cell_box.descent);
        }
        row_heights.push((row_ascent, row_descent));
    }

    // Calculate total table dimensions
    let total_width: f32 =
        col_widths.iter().sum::<f32>() + col_gap * (col_widths.len().max(1) - 1) as f32;
    let total_height: f32 =
        row_heights.iter().map(|(a, d)| a + d).sum::<f32>() + row_gap * (rows.len() - 1) as f32;

    // Center the table vertically around baseline
    let table_top = baseline_y - total_height / 2.0;

    // Second pass: render all cells
    let mut svg = String::new();
    let mut current_y = table_top;

    for (row_idx, row) in rows.iter().enumerate() {
        let (row_ascent, row_descent) = row_heights[row_idx];
        let row_baseline = current_y + row_ascent;
        let mut current_x = x;

        for (col_idx, cell) in row.iter().enumerate() {
            let col_width = col_widths.get(col_idx).copied().unwrap_or(0.0);
            let cell_box = layout_node(cell, cell_size, color, measure, 0.0, 0.0);

            // Center cell in column
            let cell_x = current_x + (col_width - cell_box.width) / 2.0;

            let rendered = layout_node(cell, cell_size, color, measure, cell_x, row_baseline);
            svg.push_str(&rendered.svg);

            current_x += col_width + col_gap;
        }

        current_y += row_ascent + row_descent + row_gap;
    }

    MathBox {
        width: total_width,
        ascent: total_height / 2.0,
        descent: total_height / 2.0,
        svg,
    }
}

fn is_large_operator(text: &str) -> bool {
    matches!(
        text,
        "∑" | "∏"
            | "∐"
            | "⋀"
            | "⋁"
            | "⋂"
            | "⋃"
            | "∫"
            | "∬"
            | "∭"
            | "∮"
            | "⨁"
            | "⨂"
            | "⨀"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::markdown::markie::MockMeasure;

    #[test]
    fn test_parse_mathml_decodes_references_and_empty_element_attributes() {
        let operator = parse_mathml("<math><mo>&lt;</mo></math>").unwrap();
        assert!(matches!(operator, MathNode::Operator(ref op) if op == "<"));

        let space = parse_mathml(r#"<math><mspace width="0.5&#x65;m"/></math>"#).unwrap();
        assert!(matches!(space, MathNode::Space(width) if (width - 0.5).abs() < f32::EPSILON));
    }

    #[test]
    fn test_parse_mathml_rejects_unclosed_elements() {
        let err = parse_mathml("<math><mi>x</mi>").unwrap_err();
        assert!(err.contains("unclosed <math>"), "unexpected error: {err}");
    }

    #[test]
    fn test_render_math_spacing_uses_mspace_width() {
        let mut measure = MockMeasure;
        let compact = render_math("xy", 16.0, "#000000", &mut measure, false).unwrap();
        let spaced = render_math(r"x\,y", 16.0, "#000000", &mut measure, false).unwrap();

        assert!(spaced.width > compact.width);
    }

    #[test]
    fn supported_expressions_render_visible_geometry() {
        let mut measure = MockMeasure;
        let expressions = [
            "x + 1",
            r"\frac{a}{b}",
            r"\sqrt{x}",
            r"\sqrt[3]{x}",
            r"\sum_{i=0}^n i",
            "x_{i}",
            "x^{2}",
            "x_{i}^{2}",
            r"\text{plain text}",
            r"\binom{n}{k}",
            r"\begin{matrix} a & b \\ c & d \end{matrix}",
            r"\begin{bmatrix} a & b \\ c & d \end{bmatrix}",
            r"\begin{aligned} a &= b + c \\ d &= e + f \end{aligned}",
            r"\begin{cases} x + y = 1 \\ x - y = 0 \end{cases}",
            r"\begin{array}{cc} 1 & 2 \\ 3 & 4 \end{array}",
        ];

        for display in [false, true] {
            for latex in expressions {
                let result = render_math(latex, 16.0, "#000000", &mut measure, display)
                    .unwrap_or_else(|err| panic!("{latex} failed to render: {err}"));
                assert!(
                    result.width > 0.0 && result.ascent + result.descent > 0.0,
                    "{latex} has empty geometry"
                );
                let image = rasterize(&result);
                assert!(
                    image.pixels().any(|pixel| pixel[3] > 0),
                    "{latex} rendered no ink"
                );
            }
        }
    }

    #[test]
    fn test_render_math_error() {
        let mut measure = MockMeasure;
        // Invalid LaTeX (missing closing brace)
        let result = render_math("\\frac{a}{", 16.0, "#000000", &mut measure, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("LaTeX parse error"));
    }

    const RASTER_PAD: f32 = 4.0;

    fn rasterize(result: &MathResult) -> image::RgbaImage {
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}"><g transform="translate({RASTER_PAD} {})">{}</g></svg>"#,
            result.width + RASTER_PAD * 2.0,
            result.ascent + result.descent + RASTER_PAD * 2.0,
            result.ascent + RASTER_PAD,
            result.svg_fragment
        );
        crate::display::markdown::markie::svg_to_image(&svg)
            .unwrap()
            .to_rgba8()
    }

    /// Height of the ink drawn between columns `x0..x1` of the formula box.
    fn ink_height(image: &image::RgbaImage, x0: f32, x1: f32) -> u32 {
        let x0 = (x0 + RASTER_PAD).floor() as u32;
        let x1 = ((x1 + RASTER_PAD).ceil() as u32).min(image.width());
        let rows: Vec<u32> = (0..image.height())
            .filter(|&y| (x0..x1).any(|x| image.get_pixel(x, y)[3] > 64))
            .collect();
        match (rows.first(), rows.last()) {
            (Some(top), Some(bottom)) => bottom - top + 1,
            _ => 0,
        }
    }

    #[test]
    fn test_multiline_delimiters_span_the_content_height() {
        let mut measure = MockMeasure;
        let font_size = 16.0;
        let cases = [
            (
                r"\begin{bmatrix} a & b & c \\ d & e & f \\ g & h & i \end{bmatrix}",
                Some("["),
                Some("]"),
            ),
            (
                r"\begin{pmatrix} a & b & c \\ d & e & f \\ g & h & i \end{pmatrix}",
                Some("("),
                Some(")"),
            ),
            (
                r"\begin{cases} x + 1 & x > 0 \\ x - 1 & x \leq 0 \end{cases}",
                Some("{"),
                None,
            ),
            (
                r"\left| \begin{matrix} a & b \\ c & d \\ e & f \end{matrix} \right|",
                Some("|"),
                Some("|"),
            ),
        ];

        for (latex, left, right) in cases {
            let result = render_math(latex, font_size, "#000000", &mut measure, true).unwrap();
            let image = rasterize(&result);
            let min_height = (result.ascent + result.descent) * 0.8;
            let mut columns = Vec::new();
            if let Some(op) = left {
                columns.push((op, 0.0, stretched_delimiter_width(op, font_size)));
            }
            if let Some(op) = right {
                let width = stretched_delimiter_width(op, font_size);
                columns.push((op, result.width - width, result.width));
            }
            for (op, x0, x1) in columns {
                let height = ink_height(&image, x0, x1) as f32;
                assert!(
                    height >= min_height,
                    "{latex}: delimiter {op} ink height {height} should reach {min_height}"
                );
            }
        }
    }

    #[test]
    fn test_preprocess_latex_handles_unclosed_array_spec() {
        // Malformed input must terminate and still rewrite the environment.
        let processed = preprocess_latex(r"\begin{array}{cc 1 & 2");
        assert!(processed.starts_with(r"\begin{matrix}"));

        // A spec-less array keeps its body.
        let processed = preprocess_latex(r"\begin{array} a \end{array}");
        assert_eq!(processed, r"\begin{matrix} a \end{matrix}");
    }

    #[test]
    fn test_render_math_with_chinese_text() {
        let mut measure = MockMeasure;
        let result = render_math(r"\text{价格} > 0", 16.0, "#000000", &mut measure, false).unwrap();
        assert!(
            result.svg_fragment.contains("价格"),
            "Chinese text should survive preprocessing: {}",
            result.svg_fragment
        );
    }

    #[test]
    fn nested_matrix_keeps_outer_rows() {
        let mut measure = MockMeasure;
        let mut height = |latex: &str| {
            let result = render_math(latex, 16.0, "#000000", &mut measure, true).unwrap();
            result.ascent + result.descent
        };
        let nested = height(
            r"\begin{pmatrix} \begin{pmatrix}a&b\\c&d\end{pmatrix} & x \\ y & z \end{pmatrix}",
        );
        let flat_two_rows = height(r"\begin{pmatrix} a & x \\ y & z \end{pmatrix}");

        // The outer matrix has two rows and its first row holds a two-row
        // matrix, so it must be taller than a plain two-row matrix.
        assert!(
            nested > flat_two_rows,
            "nested matrix height {nested} should exceed two-row matrix height {flat_two_rows}"
        );
    }
}
