use crate::display::markdown::markie::TextMeasure;
use crate::display::markdown::markie::xml::{escape_xml, sanitize_xml_text};
use latex2mathml::{DisplayStyle, latex_to_mathml};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event as XmlEvent};
use quick_xml::reader::Reader as XmlReader;
use std::collections::HashMap;

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
    let mbox = MathLayout {
        measure,
        color: text_color,
        extents: HashMap::new(),
        widths: HashMap::new(),
    }
    .layout(&root, font_size, 0.0, 0.0);

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

#[derive(Clone, Copy)]
enum MathFont {
    Serif,
    SerifItalic,
    SansSerif,
}

/// Width and vertical extent of a node laid out at the origin.
#[derive(Clone, Copy)]
struct Extent {
    width: f32,
    ascent: f32,
    descent: f32,
}

/// Lays out one formula. Parents size children by laying them out at the
/// origin before placing them; `extents` caches those measurements so nested
/// fractions, rows, and tables cost O(n·depth) instead of O(2^depth).
struct MathLayout<'a, T: TextMeasure> {
    measure: &'a mut T,
    color: &'a str,
    // Keyed by node address: the tree stays borrowed and unmoved for the render.
    extents: HashMap<(*const MathNode, u32), Extent>,
    // Text widths by `(text, font size bits, italic)`. Each leaf is still laid
    // out once for every ancestor that sizes it, and text shaping dominates
    // that cost.
    widths: HashMap<(String, u32, bool), f32>,
}

impl<T: TextMeasure> MathLayout<'_, T> {
    fn extent(&mut self, node: &MathNode, font_size: f32) -> Extent {
        let key = (node as *const MathNode, font_size.to_bits());
        if let Some(&extent) = self.extents.get(&key) {
            return extent;
        }
        let mbox = self.layout(node, font_size, 0.0, 0.0);
        let extent = Extent {
            width: mbox.width,
            ascent: mbox.ascent,
            descent: mbox.descent,
        };
        self.extents.insert(key, extent);
        extent
    }

    fn layout(&mut self, node: &MathNode, font_size: f32, x: f32, baseline_y: f32) -> MathBox {
        match node {
            MathNode::Ident(text) => {
                let italic =
                    text.len() == 1 && text.chars().next().is_some_and(|c| c.is_alphabetic());
                let font = if italic {
                    MathFont::SerifItalic
                } else {
                    MathFont::Serif
                };
                self.text_box(text, font, font_size, x, baseline_y)
            }
            MathNode::Number(text) => {
                self.text_box(text, MathFont::Serif, font_size, x, baseline_y)
            }
            MathNode::Operator(text) => {
                let is_large = is_large_operator(text);
                let effective_size = if is_large { font_size * 1.4 } else { font_size };
                let spacing = font_size * 0.15;
                let y_offset = if is_large {
                    baseline_y + (effective_size - font_size) * 0.2
                } else {
                    baseline_y
                };
                let mut op_box =
                    self.text_box(text, MathFont::Serif, effective_size, x + spacing, y_offset);
                op_box.width += spacing * 2.0;
                if is_large {
                    op_box.ascent = effective_size * 0.8;
                    op_box.descent = effective_size * 0.3;
                }
                op_box
            }
            MathNode::Text(text) => {
                self.text_box(text, MathFont::SansSerif, font_size, x, baseline_y)
            }
            MathNode::Space(em) => MathBox {
                width: font_size * em,
                ascent: 0.0,
                descent: 0.0,
                svg: String::new(),
            },
            MathNode::Row(children) => self.row(children, font_size, x, baseline_y),
            MathNode::Sup { base, sup } => {
                let base_box = self.layout(base, font_size, x, baseline_y);

                let sup_size = font_size * 0.7;
                let sup_y = baseline_y - base_box.ascent * 0.55;
                let sup_box = self.layout(sup, sup_size, x + base_box.width, sup_y);

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
                let base_box = self.layout(base, font_size, x, baseline_y);

                let sub_size = font_size * 0.7;
                let sub_y = baseline_y + base_box.descent + sub_size * 0.35;
                let sub_box = self.layout(sub, sub_size, x + base_box.width, sub_y);

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
                let base_box = self.layout(base, font_size, x, baseline_y);

                let script_size = font_size * 0.7;

                let sup_y = baseline_y - base_box.ascent * 0.55;
                let sup_box = self.layout(sup, script_size, x + base_box.width, sup_y);

                let sub_y = baseline_y + base_box.descent + script_size * 0.35;
                let sub_box = self.layout(sub, script_size, x + base_box.width, sub_y);

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
            MathNode::UnderOver { base, under, over } => self.underover(
                base,
                under.as_deref(),
                over.as_deref(),
                font_size,
                x,
                baseline_y,
            ),
            MathNode::Frac { num, den, has_rule } => {
                let frac_size = font_size * 0.85;

                let num_extent = self.extent(num, frac_size);
                let den_extent = self.extent(den, frac_size);

                let max_width = num_extent.width.max(den_extent.width);
                let padding = font_size * 0.2;
                let frac_width = max_width + padding * 2.0;

                let rule_y = baseline_y - font_size * 0.3;
                let gap = font_size * 0.15;

                let num_baseline = rule_y - gap - num_extent.descent;
                let den_baseline = rule_y + gap + den_extent.ascent;

                let num_x = x + (frac_width - num_extent.width) / 2.0;
                let den_x = x + (frac_width - den_extent.width) / 2.0;

                let num_rendered = self.layout(num, frac_size, num_x, num_baseline);
                let den_rendered = self.layout(den, frac_size, den_x, den_baseline);

                let rule_svg = if *has_rule {
                    format!(
                        r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1" />"#,
                        x,
                        rule_y,
                        x + frac_width,
                        rule_y,
                        self.color
                    )
                } else {
                    String::new()
                };

                let ascent =
                    (baseline_y - num_baseline + num_rendered.ascent).max(font_size * ASCENT_RATIO);
                let descent = (den_baseline - baseline_y + den_rendered.descent)
                    .max(font_size * DESCENT_RATIO);

                MathBox {
                    width: frac_width,
                    ascent,
                    descent,
                    svg: format!("{}{}{}", num_rendered.svg, rule_svg, den_rendered.svg),
                }
            }
            MathNode::Sqrt { radicand } => self.radical(radicand, None, font_size, x, baseline_y),
            MathNode::Root { radicand, index } => {
                self.radical(radicand, Some(index), font_size, x, baseline_y)
            }
            MathNode::Table { rows } => self.table(rows, font_size, x, baseline_y),
            MathNode::StretchyOp { op, form } => {
                self.regular_stretchy_operator(op, form, font_size, x, baseline_y)
            }
        }
    }

    /// Lays out a square root, or an nth root when `index` is given.
    fn radical(
        &mut self,
        radicand: &MathNode,
        index: Option<&MathNode>,
        font_size: f32,
        x: f32,
        baseline_y: f32,
    ) -> MathBox {
        let inner = self.extent(radicand, font_size);

        let index_width = index.map_or(0.0, |_| font_size * 0.5);
        let radical_width = font_size * 0.6;
        let padding = font_size * 0.1;
        let overbar_gap = font_size * 0.15;
        let total_width = index_width + radical_width + inner.width + padding;

        let sign_x = x + index_width;
        let inner_box = self.layout(radicand, font_size, sign_x + radical_width, baseline_y);

        let top_y = baseline_y - inner_box.ascent - overbar_gap;
        let bottom_y = baseline_y + inner_box.descent;
        let bar_end = sign_x + radical_width + inner_box.width + padding;

        // The index sits in the notch, and its overbar ends in a short hook.
        let (index_svg, hook) = match index {
            Some(index) => {
                let index_baseline = baseline_y - inner_box.ascent * 0.3;
                let index_box = self.layout(index, font_size * 0.6, x, index_baseline);
                let hook = format!(
                    " L {:.2} {:.2}",
                    bar_end - font_size * 0.1,
                    top_y - font_size * 0.05
                );
                (index_box.svg, hook)
            }
            None => (String::new(), String::new()),
        };

        let radical_svg = format!(
            r#"<path d="M {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2}{hook}" stroke="{}" stroke-width="1.2" fill="none" />"#,
            sign_x,
            baseline_y - font_size * 0.15,
            sign_x + radical_width * 0.35,
            baseline_y,
            sign_x + radical_width * 0.6,
            top_y,
            bar_end,
            top_y,
            self.color
        );

        let ascent = (baseline_y - top_y).max(inner_box.ascent + overbar_gap);
        let descent = inner_box.descent.max(bottom_y - baseline_y);

        MathBox {
            width: total_width,
            ascent,
            descent,
            svg: format!("{index_svg}{radical_svg}{}", inner_box.svg),
        }
    }

    /// Lays out one text run on the baseline with the default ascent and descent.
    fn text_box(
        &mut self,
        text: &str,
        font: MathFont,
        font_size: f32,
        x: f32,
        baseline_y: f32,
    ) -> MathBox {
        let (family, style) = match font {
            MathFont::Serif => ("serif", ""),
            MathFont::SerifItalic => ("serif", " font-style=\"italic\""),
            MathFont::SansSerif => ("sans-serif", ""),
        };
        let italic = matches!(font, MathFont::SerifItalic);
        let key = (sanitize_xml_text(text), font_size.to_bits(), italic);
        let width = *self.widths.entry(key).or_insert_with_key(|(text, _, _)| {
            self.measure
                .measure_width(text, font_size, false, false, italic)
        });
        let svg = format!(
            r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.2}" fill="{}"{}>{}</text>"#,
            x,
            baseline_y,
            family,
            font_size,
            self.color,
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

    fn row(
        &mut self,
        children: &[MathNode],
        font_size: f32,
        start_x: f32,
        baseline_y: f32,
    ) -> MathBox {
        let mut target_ascent: f32 = font_size * ASCENT_RATIO;
        let mut target_descent: f32 = font_size * DESCENT_RATIO;

        for child in children {
            if matches!(child, MathNode::StretchyOp { .. }) {
                continue;
            }
            let child_extent = self.extent(child, font_size);
            target_ascent = target_ascent.max(child_extent.ascent);
            target_descent = target_descent.max(child_extent.descent);
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
                        x: cx,
                        baseline_y,
                        target_ascent,
                        target_descent,
                    };
                    self.stretched_delimiter(op, form, layout)
                }
                _ => self.layout(child, font_size, cx, baseline_y),
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

    fn stretched_delimiter(
        &mut self,
        op: &str,
        form: &str,
        layout: StretchedDelimiterLayout,
    ) -> MathBox {
        let StretchedDelimiterLayout {
            font_size,
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
            return self.regular_stretchy_operator(op, form, font_size, x, baseline_y);
        }

        let width = stretched_delimiter_width(op, font_size);
        let stroke_width = (font_size * 0.075).clamp(1.0, 2.0);
        let top = baseline_y - target_ascent;
        let bottom = baseline_y + target_descent;
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
            "{" => brace_path(right, left, x + width / 2.0, top, bottom),
            "}" => brace_path(left, right, x + width / 2.0, top, bottom),
            "|" | "‖" => format!(
                "M {:.2} {top:.2} L {:.2} {bottom:.2}",
                x + width / 2.0,
                x + width / 2.0
            ),
            _ => {
                return self.regular_stretchy_operator(op, form, font_size, x, baseline_y);
            }
        };

        MathBox {
            width,
            ascent: target_ascent,
            descent: target_descent,
            svg: format!(
                r#"<path d="{}" stroke="{}" stroke-width="{:.2}" stroke-linecap="round" stroke-linejoin="round" fill="none" />"#,
                d, self.color, stroke_width
            ),
        }
    }

    fn regular_stretchy_operator(
        &mut self,
        op: &str,
        form: &str,
        font_size: f32,
        x: f32,
        baseline_y: f32,
    ) -> MathBox {
        let offset = match form {
            "prefix" => font_size * 0.08,
            "postfix" => -font_size * 0.08,
            _ => 0.0,
        };
        self.text_box(op, MathFont::Serif, font_size, x + offset, baseline_y)
    }

    fn underover(
        &mut self,
        base: &MathNode,
        under: Option<&MathNode>,
        over: Option<&MathNode>,
        font_size: f32,
        x: f32,
        baseline_y: f32,
    ) -> MathBox {
        let base_extent = self.extent(base, font_size);
        let script_size = font_size * 0.65;
        let gap = font_size * 0.15;

        let over = over.map(|node| (node, self.extent(node, script_size)));
        let under = under.map(|node| (node, self.extent(node, script_size)));

        let max_width = [
            base_extent.width,
            over.map_or(0.0, |(_, e)| e.width),
            under.map_or(0.0, |(_, e)| e.width),
        ]
        .into_iter()
        .fold(0.0f32, f32::max);

        let mut svg = String::new();
        let mut total_ascent = base_extent.ascent;
        let mut total_descent = base_extent.descent;

        let base_x = x + (max_width - base_extent.width) / 2.0;
        let base_rendered = self.layout(base, font_size, base_x, baseline_y);
        svg.push_str(&base_rendered.svg);

        if let Some((over_node, ob)) = over {
            let over_baseline = baseline_y - base_extent.ascent - gap - ob.descent;
            let over_x = x + (max_width - ob.width) / 2.0;
            let over_rendered = self.layout(over_node, script_size, over_x, over_baseline);
            svg.push_str(&over_rendered.svg);
            total_ascent = base_extent.ascent + gap + ob.ascent + ob.descent;
        }

        if let Some((under_node, ub)) = under {
            let under_baseline = baseline_y + base_extent.descent + gap + ub.ascent;
            let under_x = x + (max_width - ub.width) / 2.0;
            let under_rendered = self.layout(under_node, script_size, under_x, under_baseline);
            svg.push_str(&under_rendered.svg);
            total_descent = base_extent.descent + gap + ub.ascent + ub.descent;
        }

        MathBox {
            width: max_width,
            ascent: total_ascent,
            descent: total_descent,
            svg,
        }
    }

    fn table(
        &mut self,
        rows: &[Vec<MathNode>],
        font_size: f32,
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
                let cell_extent = self.extent(cell, cell_size);

                // Expand column width if needed
                while col_widths.len() <= col_idx {
                    col_widths.push(0.0);
                }
                col_widths[col_idx] = col_widths[col_idx].max(cell_extent.width);

                row_ascent = row_ascent.max(cell_extent.ascent);
                row_descent = row_descent.max(cell_extent.descent);
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
                let cell_width = self.extent(cell, cell_size).width;

                // Center cell in column
                let cell_x = current_x + (col_width - cell_width) / 2.0;

                let rendered = self.layout(cell, cell_size, cell_x, row_baseline);
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
}

struct StretchedDelimiterLayout {
    font_size: f32,
    x: f32,
    baseline_y: f32,
    target_ascent: f32,
    target_descent: f32,
}

/// Draws a curly brace whose arms curl toward `end_x` and whose cusp points
/// at `tip_x` halfway down. Each curl is a quarter circle of radius `r`
/// (control points at the usual 0.55 offset), so both halves mirror each other.
fn brace_path(end_x: f32, tip_x: f32, spine_x: f32, top: f32, bottom: f32) -> String {
    const K: f32 = 0.55;
    let mid = (top + bottom) / 2.0;
    let r = (end_x - spine_x).abs().min((bottom - top) / 4.0);
    let (end_c, tip_c) = (
        spine_x + (end_x - spine_x) * K,
        spine_x + (tip_x - spine_x) * K,
    );
    format!(
        "M {end_x:.2} {top:.2} C {end_c:.2} {top:.2} {spine_x:.2} {:.2} {spine_x:.2} {:.2} \
         L {spine_x:.2} {:.2} C {spine_x:.2} {:.2} {tip_c:.2} {mid:.2} {tip_x:.2} {mid:.2} \
         C {tip_c:.2} {mid:.2} {spine_x:.2} {:.2} {spine_x:.2} {:.2} \
         L {spine_x:.2} {:.2} C {spine_x:.2} {:.2} {end_c:.2} {bottom:.2} {end_x:.2} {bottom:.2}",
        top + r * (1.0 - K),
        top + r,
        mid - r,
        mid - r * (1.0 - K),
        mid + r * (1.0 - K),
        mid + r,
        bottom - r,
        bottom - r * (1.0 - K),
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

    /// Leftmost and rightmost inked columns of image row `y` between columns
    /// `x0..x1` of the formula box, in formula coordinates.
    fn ink_span(image: &image::RgbaImage, y: u32, x0: f32, x1: f32) -> Option<(f32, f32)> {
        let first = (x0 + RASTER_PAD).floor() as u32;
        let last = ((x1 + RASTER_PAD).ceil() as u32).min(image.width());
        let inked: Vec<u32> = (first..last)
            .filter(|&x| image.get_pixel(x, y)[3] > 64)
            .collect();
        Some((
            *inked.first()? as f32 - RASTER_PAD,
            *inked.last()? as f32 + 1.0 - RASTER_PAD,
        ))
    }

    #[test]
    fn stretched_braces_point_their_tip_at_mid_height() {
        let font_size = 32.0;
        let width = stretched_delimiter_width("{", font_size);
        let render = |latex: &str| {
            let result = render_math(latex, font_size, "#000000", &mut MockMeasure, true).unwrap();
            let image = rasterize(&result);
            (result, image)
        };
        let (_, open_image) = render(r"\left\{\begin{matrix}a\\b\\c\end{matrix}\right.");
        let (close, close_image) = render(r"\left.\begin{matrix}a\\b\\c\end{matrix}\right\}");
        let close_x0 = close.width - width;

        let rows: Vec<u32> = (0..open_image.height())
            .filter(|&y| ink_span(&open_image, y, 0.0, width).is_some())
            .collect();
        let (top_row, bottom_row) = (rows[0], rows[rows.len() - 1]);
        let span = |y: u32| ink_span(&open_image, y, 0.0, width).unwrap();

        // `{`: the arms curl to the right edge, run down the middle, and meet
        // in a cusp at the left edge halfway down.
        let (left, _) = span((top_row + bottom_row) / 2);
        assert!(
            left <= width * 0.15,
            "{{ tip starts at {left}, expected at the left edge"
        );
        for y in [top_row, bottom_row] {
            let (left, right) = span(y);
            assert!(
                left >= width * 0.4 && right >= width * 0.85,
                "{{ end at row {y} spans {left}..{right}, expected at the right edge"
            );
        }
        let quarter = (bottom_row - top_row) / 4;
        for y in [top_row + quarter, bottom_row - quarter] {
            let (left, right) = span(y);
            let centre = (left + right) / 2.0;
            assert!(
                (centre - width / 2.0).abs() <= width * 0.1,
                "{{ arm at row {y} centred at {centre}, expected near {}",
                width / 2.0
            );
        }
        // Antialiasing can shift the lower half by a row, so each row may
        // match any of the three rows around its mirror image.
        for y in top_row + 1..bottom_row {
            let upper = span(y);
            let mirror = top_row + bottom_row - y;
            assert!(
                (mirror - 1..=mirror + 1).any(|m| {
                    let lower = span(m);
                    (upper.0 - lower.0).abs() <= 1.0 && (upper.1 - lower.1).abs() <= 1.0
                }),
                "row {y}: upper half {upper:?} does not mirror lower half near row {mirror}"
            );
        }

        // Comparing every row against the mirrored `}` catches a control
        // point moved in only one of the two paths.
        for y in 0..open_image.height() {
            let open_span = ink_span(&open_image, y, 0.0, width);
            let close_span = ink_span(&close_image, y, close_x0, close.width)
                .map(|(left, right)| (close.width - right, close.width - left));
            match (open_span, close_span) {
                (None, None) => {}
                (Some(open), Some(mirrored)) => assert!(
                    (open.0 - mirrored.0).abs() <= 1.0 && (open.1 - mirrored.1).abs() <= 1.0,
                    "row {y}: {{ ink {open:?} differs from mirrored }} ink {mirrored:?}"
                ),
                _ => panic!("row {y}: {{ ink {open_span:?} vs mirrored }} ink {close_span:?}"),
            }
        }
    }

    /// Records how often each `(text, font size, italic)` run is measured.
    #[derive(Default)]
    struct CountingMeasure {
        calls: HashMap<(String, u32, bool), usize>,
    }

    impl TextMeasure for CountingMeasure {
        fn measure_width(
            &mut self,
            text: &str,
            font_size: f32,
            is_code: bool,
            is_bold: bool,
            is_italic: bool,
        ) -> f32 {
            *self
                .calls
                .entry((text.to_string(), font_size.to_bits(), is_italic))
                .or_default() += 1;
            MockMeasure.measure_width(text, font_size, is_code, is_bold, is_italic)
        }
    }

    #[test]
    fn nested_fractions_measure_each_text_run_once() {
        let mut latex = "x".to_string();
        for _ in 0..8 {
            latex = format!(r"1 + \frac{{1}}{{{latex}}}");
        }
        let mut measure = CountingMeasure::default();
        render_math(&latex, 16.0, "#000000", &mut measure, true).unwrap();

        let repeated: Vec<_> = measure.calls.iter().filter(|(_, n)| **n > 1).collect();
        assert!(
            repeated.is_empty(),
            "runs measured more than once: {repeated:?}"
        );
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
