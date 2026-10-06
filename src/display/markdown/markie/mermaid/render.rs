use std::collections::HashMap;

use crate::display::markdown::markie::TextMeasure;
use crate::display::markdown::markie::layout::Rect;
use crate::display::markdown::markie::xml::{escape_xml, sanitize_xml_text};

use super::flowchart::SelfLoop;
use super::layout::{
    CLASS_MEMBER_LINE_HEIGHT, ER_ATTRIBUTE_FONT_SCALE, LayoutEngine, RectExt, STATE_CHILD_GAP,
    STATE_INNER_PAD, STATE_ROUTE_LANE,
};
use super::types::*;
use super::{MermaidDiagram, parse_mermaid};

/// Font size used when a diagram is rendered without a Markdown base size.
const DEFAULT_DIAGRAM_FONT_SIZE: f32 = 13.0;

/// Style configuration for diagram rendering
#[derive(Debug, Clone)]
pub struct DiagramStyle {
    pub node_fill: String,
    pub node_stroke: String,
    pub node_text: String,
    pub edge_stroke: String,
    pub edge_text: String,
    pub background: String,
    pub font_family: String,
    pub font_size: f32,
}

impl Default for DiagramStyle {
    fn default() -> Self {
        Self {
            node_fill: "#f5f5f5".to_string(),
            node_stroke: "#333333".to_string(),
            node_text: "#333333".to_string(),
            edge_stroke: "#333333".to_string(),
            edge_text: "#666666".to_string(),
            background: "transparent".to_string(),
            font_family: "sans-serif".to_string(),
            font_size: DEFAULT_DIAGRAM_FONT_SIZE,
        }
    }
}

fn parse_hex_rgb(value: &str) -> Option<(f32, f32, f32)> {
    let hex = value.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0;
    Some((r, g, b))
}

/// Mix two hex colors: result = base * (1-t) + fg * t
fn mix_color(base: &str, fg: &str, t: f32) -> String {
    let (br, bg, bb) = parse_hex_rgb(base).unwrap_or((0.95, 0.95, 0.95));
    let (fr, fg_g, fb) = parse_hex_rgb(fg).unwrap_or((0.2, 0.2, 0.2));
    let r = br * (1.0 - t) + fr * t;
    let g = bg * (1.0 - t) + fg_g * t;
    let b = bb * (1.0 - t) + fb * t;
    format!(
        "#{:02x}{:02x}{:02x}",
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8
    )
}

/// Arrowhead drawing style shared by every diagram type.
pub(super) enum ArrowHead {
    Filled,
    Open,
}

/// Arrowhead whose tip sits at `(x, y)`, pointing along `angle` (radians).
pub(super) fn arrowhead(
    kind: ArrowHead,
    x: f32,
    y: f32,
    angle: f32,
    style: &DiagramStyle,
) -> String {
    const LENGTH: f32 = 8.0;
    const HALF_WIDTH: f32 = 4.8;
    let (sin, cos) = angle.sin_cos();
    let p1 = (
        x - cos * LENGTH + sin * HALF_WIDTH,
        y - sin * LENGTH - cos * HALF_WIDTH,
    );
    let p2 = (
        x - cos * LENGTH - sin * HALF_WIDTH,
        y - sin * LENGTH + cos * HALF_WIDTH,
    );
    match kind {
        ArrowHead::Filled => format!(
            r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" />"#,
            x, y, p1.0, p1.1, p2.0, p2.1, style.edge_stroke
        ),
        ArrowHead::Open => format!(
            r#"<polyline points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="none" stroke="{}" stroke-width="0.75" />"#,
            p1.0, p1.1, x, y, p2.0, p2.1, style.edge_stroke
        ),
    }
}

const PILL_PAD_X: f32 = 5.0;
const PILL_PAD_Y: f32 = 4.0;

/// Width and height of the pill `label_pill` draws around `text`.
fn pill_size(measure: &mut impl TextMeasure, text: &str, font_size: f32) -> (f32, f32) {
    let text_w = measure.measure_width(&sanitize_xml_text(text), font_size, false, false, false);
    (text_w + PILL_PAD_X * 2.0, font_size + PILL_PAD_Y * 2.0)
}

/// Edge label centered on `center` over a rounded background, so it stays
/// readable where it crosses lines. Returns the SVG and the pill bounds.
fn label_pill(
    measure: &mut impl TextMeasure,
    style: &DiagramStyle,
    text: &str,
    font_size: f32,
    center: (f32, f32),
) -> (String, Rect) {
    let (w, h) = pill_size(measure, text, font_size);
    let (cx, cy) = center;
    let rect = Rect::new(cx - w / 2.0, cy - h / 2.0, w, h);
    let svg = format!(
        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="3" fill="{}" stroke="{}" stroke-width="0.5" /><text x="{:.2}" y="{:.2}" dy="0.35em" font-family="{}" font-size="{:.1}" fill="{}" text-anchor="middle">{}</text>"#,
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        style.node_fill,
        style.node_stroke,
        cx,
        cy,
        style.font_family,
        font_size,
        style.edge_text,
        escape_xml(text)
    );
    (svg, rect)
}

/// Render any mermaid diagram to SVG
pub fn render_diagram<T: TextMeasure>(
    source: &str,
    style: &DiagramStyle,
    measure: &mut T,
) -> Result<(String, f32, f32), String> {
    let diagram = parse_mermaid(source)?;

    Ok(match diagram {
        MermaidDiagram::Flowchart(fc) => super::flowchart::render_flowchart(&fc, style, measure),
        MermaidDiagram::Sequence(seq) => render_sequence(&seq, style, measure),
        MermaidDiagram::ClassDiagram(cls) => render_class(&cls, style, measure),
        MermaidDiagram::StateDiagram(st) => render_state(&st, style, measure),
        MermaidDiagram::ErDiagram(er) => render_er(&er, style, measure),
    })
}

// ============================================
// FLOWCHART RENDERING (moved to flowchart.rs)
// ============================================

// Note: render_flowchart is in flowchart.rs

// ============================================
// SEQUENCE DIAGRAM RENDERING
// ============================================

fn render_sequence(
    diagram: &SequenceDiagram,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
) -> (String, f32, f32) {
    let mut layout = LayoutEngine::new(measure, style.font_size);
    let (positions, diagram_right) = layout.layout_sequence(diagram);

    let mut svg = String::new();
    let padding = 20.0;

    let mut participant_centers: HashMap<&str, f32> = HashMap::new();
    for participant in &diagram.participants {
        if let Some(pos) = positions.get(&participant.id) {
            let (cx, _) = pos.center();
            participant_centers.insert(participant.id.as_str(), cx);
        }
    }

    let left_edge = participant_centers
        .values()
        .copied()
        .fold(f32::MAX, f32::min)
        .min(40.0);
    let right_edge = participant_centers
        .values()
        .copied()
        .fold(f32::MIN, f32::max)
        .max(120.0);

    for participant in &diagram.participants {
        if let Some(pos) = positions.get(&participant.id) {
            let display_name = participant.alias.as_ref().unwrap_or(&participant.id);
            let label = escape_xml(display_name);
            let (text_x, _) = pos.center();

            svg.push_str(&format!(
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}" stroke="{}" stroke-width="1" rx="4" />"#,
                pos.x, pos.y, pos.w, pos.h, style.node_fill, style.node_stroke
            ));
            svg.push_str(&format!(
                r#"<text x="{:.2}" y="{:.2}" dy="0.35em" font-family="{}" font-size="{:.1}" font-weight="500" fill="{}" text-anchor="middle">{}</text>"#,
                text_x,
                pos.y + pos.h / 2.0,
                style.font_family,
                style.font_size,
                style.node_text,
                label
            ));
        }
    }

    let participant_bottom = positions
        .values()
        .map(RectExt::bottom)
        .fold(f32::MIN, f32::max);
    let lifeline_start_y = participant_bottom + 8.0;

    let mut message_y = participant_bottom + 34.0;

    let mut activation_starts: HashMap<String, Vec<f32>> = HashMap::new();
    let elements_svg = render_sequence_elements(&mut RenderSequenceContext {
        elements: &diagram.elements,
        participant_centers: &participant_centers,
        style,
        measure,
        message_y: &mut message_y,
        block_depth: 0,
        left_edge,
        right_edge,
        activation_starts: &mut activation_starts,
    });

    let lifeline_end_y = (message_y + 6.0).max(lifeline_start_y + 24.0);
    for participant in &diagram.participants {
        if let Some(x) = participant_centers.get(participant.id.as_str()) {
            svg.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" stroke-dasharray="6,4" />"#,
                x, lifeline_start_y, x, lifeline_end_y, style.edge_stroke
            ));
        }
    }
    svg.push_str(&elements_svg);

    (svg, diagram_right + padding, message_y + 20.0 + padding)
}

struct RenderSequenceContext<'a, T: TextMeasure> {
    elements: &'a [SequenceElement],
    participant_centers: &'a HashMap<&'a str, f32>,
    style: &'a DiagramStyle,
    measure: &'a mut T,
    message_y: &'a mut f32,
    block_depth: usize,
    left_edge: f32,
    right_edge: f32,
    activation_starts: &'a mut HashMap<String, Vec<f32>>,
}

fn sequence_arrowhead(kind: &MessageKind) -> ArrowHead {
    match kind {
        MessageKind::Sync => ArrowHead::Filled,
        MessageKind::Async | MessageKind::Reply => ArrowHead::Open,
    }
}

fn render_sequence_elements<T: TextMeasure>(ctx: &mut RenderSequenceContext<'_, T>) -> String {
    let elements = ctx.elements;
    let participant_centers = ctx.participant_centers;
    let style = ctx.style;
    let measure = &mut *ctx.measure;
    let message_y = &mut *ctx.message_y;
    let block_depth = ctx.block_depth;
    let left_edge = ctx.left_edge;
    let right_edge = ctx.right_edge;
    let activation_starts = &mut *ctx.activation_starts;

    let mut svg = String::new();
    for element in elements {
        match element {
            SequenceElement::Message(msg) => {
                if let (Some(x1), Some(x2)) = (
                    participant_centers.get(msg.from.as_str()),
                    participant_centers.get(msg.to.as_str()),
                ) {
                    if (x1 - x2).abs() < 0.5 {
                        // Self-message: draw a loop to the right
                        let cx = *x1;
                        let loop_w = 40.0;
                        let loop_h = 36.0;
                        let y_top = *message_y;
                        let y_bot = y_top + loop_h;

                        let dash = if msg.msg_type == MessageType::Dotted
                            || msg.kind == MessageKind::Reply
                        {
                            " stroke-dasharray=\"4,4\""
                        } else {
                            ""
                        };

                        svg.push_str(&format!(
                            r#"<polyline points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="none" stroke="{}" stroke-width="0.75"{} />"#,
                            cx, y_top,
                            cx + loop_w, y_top,
                            cx + loop_w, y_bot,
                            cx, y_bot,
                            style.edge_stroke, dash
                        ));

                        // Arrowhead pointing left at return point
                        svg.push_str(&arrowhead(
                            sequence_arrowhead(&msg.kind),
                            cx,
                            y_bot,
                            std::f32::consts::PI,
                            style,
                        ));

                        if !msg.label.is_empty() {
                            let label_font = style.font_size * 0.82;
                            let (pill_w, _) = pill_size(measure, &msg.label, label_font);
                            let center = (cx + loop_w + 4.0 + pill_w / 2.0, y_top + loop_h / 2.0);
                            let (pill, _) =
                                label_pill(measure, style, &msg.label, label_font, center);
                            svg.push_str(&pill);
                        }

                        *message_y = y_bot + 20.0;
                    } else {
                        let is_right = x2 > x1;
                        let dash = if msg.msg_type == MessageType::Dotted
                            || msg.kind == MessageKind::Reply
                        {
                            " stroke-dasharray=\"4,4\""
                        } else {
                            ""
                        };

                        svg.push_str(&format!(
                            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75"{} />"#,
                            x1, *message_y, x2, *message_y, style.edge_stroke, dash
                        ));

                        let angle = if is_right { 0.0 } else { std::f32::consts::PI };
                        svg.push_str(&arrowhead(
                            sequence_arrowhead(&msg.kind),
                            *x2,
                            *message_y,
                            angle,
                            style,
                        ));

                        if !msg.label.is_empty() {
                            let center = ((x1 + x2) / 2.0, *message_y - 10.0);
                            let label_font = style.font_size * 0.82;
                            let (pill, _) =
                                label_pill(measure, style, &msg.label, label_font, center);
                            svg.push_str(&pill);
                        }

                        *message_y += 50.0;
                    }
                }
            }
            SequenceElement::Activation(activation) => {
                if let Some(cx) = participant_centers.get(activation.participant.as_str()) {
                    activation_starts
                        .entry(activation.participant.clone())
                        .or_default()
                        .push(*message_y - 10.0);
                    svg.push_str(&format!(
                        r#"<rect x="{:.2}" y="{:.2}" width="8" height="16" fill="{}" stroke="{}" stroke-width="1" />"#,
                        cx - 4.0,
                        *message_y - 10.0,
                        style.node_fill,
                        style.node_stroke
                    ));
                }
                *message_y += 24.0;
            }
            SequenceElement::Deactivation(activation) => {
                if let Some(cx) = participant_centers.get(activation.participant.as_str())
                    && let Some(start) = activation_starts
                        .entry(activation.participant.clone())
                        .or_default()
                        .pop()
                {
                    svg.push_str(&format!(
                            r#"<rect x="{:.2}" y="{:.2}" width="8" height="{:.2}" fill="{}" fill-opacity="0.35" stroke="{}" stroke-width="1" />"#,
                            cx - 4.0,
                            start,
                            (*message_y - start).max(16.0),
                            style.node_fill,
                            style.node_stroke
                        ));
                }
                *message_y += 24.0;
            }
            SequenceElement::Note {
                participant,
                position,
                text,
            } => {
                if let Some(cx) = participant_centers.get(participant.as_str()) {
                    let cleaned = sanitize_xml_text(text);
                    let note_width = (measure.measure_width(
                        &cleaned,
                        style.font_size * 0.8,
                        false,
                        false,
                        false,
                    ) + 20.0)
                        .clamp(80.0, 220.0);
                    let x = match position.as_str() {
                        "left" => cx - note_width - 12.0,
                        "right" => cx + 12.0,
                        _ => cx - note_width / 2.0,
                    };
                    let y = *message_y - 18.0;
                    svg.push_str(&format!(
                        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="28" rx="3" fill="{}" fill-opacity="0.25" stroke="{}" stroke-width="1" />"#,
                        x,
                        y,
                        note_width,
                        style.node_fill,
                        style.node_stroke
                    ));
                    svg.push_str(&format!(
                        r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}">{}</text>"#,
                        x + 8.0,
                        y + 18.0,
                        style.font_family,
                        style.font_size * 0.8,
                        style.node_text,
                        escape_xml(&cleaned)
                    ));
                }
                *message_y += 42.0;
            }
            SequenceElement::Block(block) => {
                *message_y += 12.0;
                let start_y = *message_y - 20.0;
                let inset = block_depth as f32 * 8.0;
                let block_left = left_edge - 36.0 + inset;
                let block_right = right_edge + 36.0 - inset;
                let block_kind = match block.block_type {
                    SequenceBlockType::Alt => "alt",
                    SequenceBlockType::Opt => "opt",
                    SequenceBlockType::Loop => "loop",
                    SequenceBlockType::Par => "par",
                    SequenceBlockType::Critical => "critical",
                };
                let title = if block.label.is_empty() {
                    block_kind.to_string()
                } else {
                    format!("{} {}", block_kind, block.label)
                };

                let title_font = style.font_size * 0.8;
                let cleaned_title = sanitize_xml_text(&title);
                let title_w = measure.measure_width(&cleaned_title, title_font, false, true, false);
                let np_pad_x = 6.0;
                let np_pad_y = 3.0;
                let np_w = title_w + np_pad_x * 2.0;
                let np_h = title_font + np_pad_y * 2.0;
                let np_x = block_left + 10.0 - np_pad_x;
                let np_y = *message_y - title_font - np_pad_y + 2.0;
                svg.push_str(&format!(
                    r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}" stroke="{}" stroke-width="0.75" />"#,
                    np_x, np_y, np_w, np_h,
                    style.node_fill, style.node_stroke
                ));
                svg.push_str(&format!(
                    r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}" font-weight="bold">{}</text>"#,
                    block_left + 10.0,
                    *message_y,
                    style.font_family,
                    title_font,
                    style.node_text,
                    escape_xml(&title)
                ));
                *message_y += 22.0;

                svg.push_str(&render_sequence_elements(&mut RenderSequenceContext {
                    elements: &block.messages,
                    participant_centers,
                    style,
                    measure,
                    message_y,
                    block_depth: block_depth + 1,
                    left_edge,
                    right_edge,
                    activation_starts,
                }));

                for (label, branch_elements) in &block.else_branches {
                    let separator_y = *message_y + 2.0;
                    svg.push_str(&format!(
                        r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1" stroke-dasharray="5,3" />"#,
                        block_left,
                        separator_y,
                        block_right,
                        separator_y,
                        style.edge_stroke
                    ));
                    if !label.is_empty() {
                        svg.push_str(&format!(
                            r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}">{}</text>"#,
                            block_left + 10.0,
                            separator_y - 12.0,
                            style.font_family,
                            style.font_size * 0.78,
                            style.node_text,
                            escape_xml(label)
                        ));
                    }
                    *message_y = separator_y + 30.0;
                    svg.push_str(&render_sequence_elements(&mut RenderSequenceContext {
                        elements: branch_elements,
                        participant_centers,
                        style,
                        measure,
                        message_y,
                        block_depth: block_depth + 1,
                        left_edge,
                        right_edge,
                        activation_starts,
                    }));
                }

                svg.push_str(&format!(
                    r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="none" stroke="{}" stroke-width="1" stroke-dasharray="5,3" />"#,
                    block_left,
                    start_y,
                    (block_right - block_left).max(24.0),
                    (*message_y - start_y + 16.0).max(28.0),
                    style.edge_stroke
                ));
                *message_y += 10.0;
            }
        }
    }
    svg
}

// ============================================
// CLASS DIAGRAM RENDERING
// ============================================

fn render_class(
    diagram: &ClassDiagram,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
) -> (String, f32, f32) {
    let mut layout = LayoutEngine::new(measure, style.font_size);
    let (positions, bbox) = layout.layout_class(diagram);

    let mut svg = String::new();
    let padding = 20.0;

    // Draw classes
    for class in &diagram.classes {
        if let Some(pos) = positions.get(&class.name) {
            svg.push_str(&render_class_box(class, pos, style));
        }
    }

    // Draw relations
    for relation in &diagram.relations {
        let from_pos = positions.get(&relation.from);
        let to_pos = positions.get(&relation.to);

        if let (Some(from), Some(to)) = (from_pos, to_pos) {
            svg.push_str(&render_class_relation(relation, from, to, style, measure));
        }
    }

    let total_width = bbox.right() + padding;
    let total_height = bbox.bottom() + padding;

    (svg, total_width, total_height)
}

fn render_class_box(class: &ClassDefinition, pos: &Rect, style: &DiagramStyle) -> String {
    let mut svg = String::new();

    // Main box
    svg.push_str(&format!(
        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
        pos.x, pos.y, pos.w, pos.h,
        style.node_fill, style.node_stroke
    ));

    let mut y = pos.y + style.font_size + 8.0;

    let name_text = escape_xml(&class.title());
    let name_style = if class.is_abstract || class.is_interface {
        " font-style=\"italic\""
    } else {
        ""
    };

    // Header band (subtle tinted background for class name area)
    let header_h = y + 6.0 - pos.y;
    let header_fill = mix_color(&style.node_fill, &style.node_text, 0.05);
    svg.push_str(&format!(
        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}" />"#,
        pos.x + 0.5,
        pos.y + 0.5,
        pos.w - 1.0,
        header_h,
        header_fill
    ));

    svg.push_str(&format!(
        r#"<text x="{:.2}" y="{:.2}" dy="0.35em" font-family="{}" font-size="{:.1}" fill="{}" text-anchor="middle" font-weight="bold"{}>{}</text>"#,
        pos.x + pos.w / 2.0,
        pos.y + header_h / 2.0,
        style.font_family,
        style.font_size,
        style.node_text,
        name_style,
        name_text
    ));

    // Divider line after name
    y += 6.0;
    svg.push_str(&format!(
        r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" />"#,
        pos.x,
        y,
        pos.x + pos.w,
        y,
        style.node_stroke
    ));

    // Attributes
    y += style.font_size + 4.0;
    for attr in &class.attributes {
        svg.push_str(&class_member_text(&attr.display(), pos.x + 8.0, y, style));
        y += style.font_size * CLASS_MEMBER_LINE_HEIGHT;
    }

    // Divider line before methods
    if !class.methods.is_empty() {
        y += 2.0;
        svg.push_str(&format!(
            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1" />"#,
            pos.x,
            y,
            pos.x + pos.w,
            y,
            style.node_stroke
        ));
        y += style.font_size + 2.0;
    }

    // Methods
    for method in &class.methods {
        svg.push_str(&class_member_text(&method.display(), pos.x + 8.0, y, style));
        y += style.font_size * CLASS_MEMBER_LINE_HEIGHT;
    }

    svg
}

fn class_member_text(text: &str, x: f32, y: f32, style: &DiagramStyle) -> String {
    format!(
        r#"<text x="{:.2}" y="{:.2}" font-family="monospace" font-size="{:.1}" fill="{}">{}</text>"#,
        x,
        y,
        style.font_size * 0.85,
        style.node_text,
        escape_xml(text)
    )
}

fn render_class_relation(
    relation: &ClassRelation,
    from: &Rect,
    to: &Rect,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
) -> String {
    let mut svg = String::new();

    let (from_cx, from_cy) = from.center();
    let (to_cx, to_cy) = to.center();
    let angle = (to_cy - from_cy).atan2(to_cx - from_cx);

    let (x1, y1) = rect_boundary_point(from, angle);
    let (x2, y2) = rect_boundary_point(to, angle + std::f32::consts::PI);

    let line_style = match relation.relation_type {
        ClassRelationType::Dependency | ClassRelationType::Realization => {
            " stroke-dasharray=\"6,3\""
        }
        _ => "",
    };

    svg.push_str(&format!(
        r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75"{} />"#,
        x1, y1, x2, y2, style.edge_stroke, line_style
    ));

    // Association points at the target; every other marker sits on the source end.
    if relation.relation_type == ClassRelationType::Association {
        svg.push_str(&draw_marker(&relation.relation_type, x2, y2, angle, style));
    } else {
        let back = angle + std::f32::consts::PI;
        svg.push_str(&draw_marker(&relation.relation_type, x1, y1, back, style));
    }

    if let Some(label) = &relation.label {
        // Offset the label to one side of the line, along its normal.
        let label_offset = 18.0;
        let center = (
            (x1 + x2) / 2.0 - angle.sin() * label_offset,
            (y1 + y2) / 2.0 + angle.cos() * label_offset,
        );
        let (pill, _) = label_pill(measure, style, label, style.font_size * 0.8, center);
        svg.push_str(&pill);
    }

    svg
}

fn draw_marker(
    relation_type: &ClassRelationType,
    x: f32,
    y: f32,
    angle: f32,
    style: &DiagramStyle,
) -> String {
    let cos = angle.cos();
    let sin = angle.sin();

    match relation_type {
        ClassRelationType::Association | ClassRelationType::Dependency => {
            arrowhead(ArrowHead::Filled, x, y, angle, style)
        }
        ClassRelationType::Inheritance | ClassRelationType::Realization => {
            let p1 = (x - cos * 14.0 + sin * 7.0, y - sin * 14.0 - cos * 7.0);
            let p2 = (x - cos * 14.0 - sin * 7.0, y - sin * 14.0 + cos * 7.0);
            format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                x, y, p1.0, p1.1, p2.0, p2.1, style.node_fill, style.edge_stroke
            )
        }
        ClassRelationType::Composition => {
            let p1 = (x - cos * 16.0 + sin * 6.0, y - sin * 16.0 - cos * 6.0);
            let p2 = (x - cos * 16.0 - sin * 6.0, y - sin * 16.0 + cos * 6.0);
            let back = (x - cos * 24.0, y - sin * 24.0);
            format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" />"#,
                x, y, p1.0, p1.1, back.0, back.1, p2.0, p2.1, style.edge_stroke
            )
        }
        ClassRelationType::Aggregation => {
            let p1 = (x - cos * 16.0 + sin * 6.0, y - sin * 16.0 - cos * 6.0);
            let p2 = (x - cos * 16.0 - sin * 6.0, y - sin * 16.0 + cos * 6.0);
            let back = (x - cos * 24.0, y - sin * 24.0);
            format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                x, y, p1.0, p1.1, back.0, back.1, p2.0, p2.1, style.node_fill, style.edge_stroke
            )
        }
    }
}

// ============================================
// STATE DIAGRAM RENDERING
// ============================================

fn vseg_hits_rect(x: f32, y1: f32, y2: f32, r: &Rect) -> bool {
    let (ya, yb) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };
    x >= r.x && x <= r.x + r.w && yb >= r.y && ya <= r.y + r.h
}

fn hseg_hits_rect(y: f32, x1: f32, x2: f32, r: &Rect) -> bool {
    let (xa, xb) = if x1 <= x2 { (x1, x2) } else { (x2, x1) };
    y >= r.y && y <= r.y + r.h && xb >= r.x && xa <= r.x + r.w
}

fn line_intersects_rect(x1: f32, y1: f32, x2: f32, y2: f32, r: &Rect) -> bool {
    // Cohen–Sutherland: check if segment (x1,y1)→(x2,y2) intersects axis-aligned rect r
    let left = r.x;
    let right = r.x + r.w;
    let top = r.y;
    let bottom = r.y + r.h;

    let code = |x: f32, y: f32| -> u8 {
        let mut c = 0u8;
        if x < left {
            c |= 1;
        }
        if x > right {
            c |= 2;
        }
        if y < top {
            c |= 4;
        }
        if y > bottom {
            c |= 8;
        }
        c
    };

    let mut c1 = code(x1, y1);
    let mut c2 = code(x2, y2);
    let mut ax = x1;
    let mut ay = y1;
    let mut bx = x2;
    let mut by = y2;

    for _ in 0..20 {
        if c1 == 0 || c2 == 0 {
            return true;
        } // one endpoint inside
        if c1 & c2 != 0 {
            return false;
        } // both on same outside
        let c = if c1 != 0 { c1 } else { c2 };
        let (nx, ny);
        if c & 8 != 0 {
            nx = ax + (bx - ax) * (bottom - ay) / (by - ay);
            ny = bottom;
        } else if c & 4 != 0 {
            nx = ax + (bx - ax) * (top - ay) / (by - ay);
            ny = top;
        } else if c & 2 != 0 {
            ny = ay + (by - ay) * (right - ax) / (bx - ax);
            nx = right;
        } else {
            ny = ay + (by - ay) * (left - ax) / (bx - ax);
            nx = left;
        }
        if c == c1 {
            ax = nx;
            ay = ny;
            c1 = code(ax, ay);
        } else {
            bx = nx;
            by = ny;
            c2 = code(bx, by);
        }
    }
    false
}

fn render_state(
    diagram: &StateDiagram,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
) -> (String, f32, f32) {
    let mut layout = LayoutEngine::new(measure, style.font_size);
    let (positions, bbox) = layout.layout_state(diagram);

    let mut svg = String::new();
    let padding = 20.0;

    let child_state_ids = diagram.nested_state_ids();
    let states_by_id = diagram.states_by_id();

    // Draw transitions first (behind states)
    let visible_transitions: Vec<&StateTransition> = diagram
        .transitions
        .iter()
        .filter(|transition| {
            !child_state_ids.contains(transition.from.as_str())
                && !child_state_ids.contains(transition.to.as_str())
        })
        .collect();

    let mut pair_totals: HashMap<(String, String), usize> = HashMap::new();
    for transition in &visible_transitions {
        *pair_totals
            .entry(state_pair_key(&transition.from, &transition.to))
            .or_insert(0) += 1;
    }

    let state_obstacles: Vec<Rect> = positions.values().map(|p| p.expanded(8.0)).collect();

    let mut transition_min_x = f32::MAX;
    let mut transition_max_x = f32::MIN;
    let mut transition_max_y = f32::MIN;

    let mut pair_seen: HashMap<(String, String), usize> = HashMap::new();
    let mut occupied_labels: Vec<Rect> = Vec::new();
    for transition in visible_transitions {
        let key = state_pair_key(&transition.from, &transition.to);
        let route_index = pair_seen.get(&key).copied().unwrap_or(0);
        pair_seen.insert(key.clone(), route_index + 1);
        let route_total = pair_totals.get(&key).copied().unwrap_or(1);

        let from_pos = positions.get(&transition.from);
        let to_pos = positions.get(&transition.to);

        if let (Some(from), Some(to)) = (from_pos, to_pos) {
            let (t_svg, ext) = render_state_transition(&mut StateTransitionContext {
                transition,
                from,
                to,
                style,
                measure,
                route_index,
                route_total,
                occupied_labels: &mut occupied_labels,
                obstacles: &state_obstacles,
            });
            svg.push_str(&t_svg);
            transition_min_x = transition_min_x.min(ext.x);
            transition_max_x = transition_max_x.max(ext.x + ext.w);
            transition_max_y = transition_max_y.max(ext.y + ext.h);
        }
    }

    // Draw states
    for state in &diagram.states {
        if child_state_ids.contains(state.id.as_str()) {
            continue;
        }

        if let Some(pos) = positions.get(&state.id) {
            svg.push_str(&render_state_node(
                state,
                pos,
                style,
                &states_by_id,
                &positions,
                measure,
            ));
        }
    }

    let mut total_width = bbox.right() + padding;
    let mut total_height = bbox.bottom() + padding;

    for state in &diagram.states {
        for child in &state.children {
            if let StateElement::Note {
                state: note_state,
                text,
            } = child
                && !text.is_empty()
                && let Some(pos) = positions.get(note_state.as_str())
            {
                let note_width = 180.0_f32;
                let note_height = 26.0_f32;
                let nx = pos.x + pos.w + 28.0;
                let ny = pos.y + 4.0;
                total_width = total_width.max(nx + note_width + padding);
                total_height = total_height.max(ny + note_height + padding);
            }
        }
    }

    // Expand to contain any transition routing that extends beyond the bbox
    if transition_max_x != f32::MIN {
        total_width = total_width.max(transition_max_x + padding);
    }
    if transition_max_y != f32::MIN {
        total_height = total_height.max(transition_max_y + padding);
    }

    // If transitions route into negative x territory, shift everything right
    if transition_min_x != f32::MAX && transition_min_x < 0.0 {
        let shift = -transition_min_x + padding;
        let shifted_svg = format!(r#"<g transform="translate({:.2},0)">{}</g>"#, shift, svg);
        total_width += shift;
        return (shifted_svg, total_width, total_height);
    }

    (svg, total_width, total_height)
}

fn render_state_node(
    state: &State,
    pos: &Rect,
    style: &DiagramStyle,
    states_by_id: &HashMap<&str, &State>,
    positions: &HashMap<String, Rect>,
    measure: &mut impl TextMeasure,
) -> String {
    let mut svg = String::new();

    if state.is_start {
        // Start state (filled circle)
        svg.push_str(&format!(
            r#"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="{}" />"#,
            pos.x + pos.w / 2.0,
            pos.y + pos.h / 2.0,
            pos.w / 2.0,
            style.node_stroke
        ));
    } else if state.is_end {
        // End state (circle with ring)
        let cx = pos.x + pos.w / 2.0;
        let cy = pos.y + pos.h / 2.0;
        svg.push_str(&format!(
            r#"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="{}" stroke="{}" stroke-width="2" />"#,
            cx,
            cy,
            pos.w / 2.0 - 3.0,
            style.node_stroke,
            style.node_stroke
        ));
        svg.push_str(&format!(
            r#"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="none" stroke="{}" stroke-width="2" />"#,
            cx, cy, pos.w / 2.0, style.node_stroke
        ));
    } else {
        svg.push_str(&format!(
            r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
            pos.x, pos.y, pos.w, pos.h,
            10.0, style.node_fill, style.node_stroke
        ));

        let text_x = pos.x + pos.w / 2.0;
        let text_y = if state.is_composite {
            pos.y + style.font_size + 8.0
        } else {
            pos.y + pos.h / 2.0 + style.font_size / 3.0
        };
        svg.push_str(&format!(
            r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}" text-anchor="middle">{}</text>"#,
            text_x, text_y, style.font_family, style.font_size, style.node_text, escape_xml(&state.label)
        ));

        if state.is_composite {
            svg.push_str(&render_composite_state_contents(
                state,
                pos,
                style,
                states_by_id,
                positions,
                measure,
            ));
        }
        for child in &state.children {
            if let StateElement::Note { text, .. } = child
                && !text.is_empty()
            {
                svg.push_str(&render_state_note(text, pos, style, measure));
            }
        }
    }

    svg
}

fn render_composite_state_contents(
    state: &State,
    parent_pos: &Rect,
    style: &DiagramStyle,
    states_by_id: &HashMap<&str, &State>,
    positions: &HashMap<String, Rect>,
    measure: &mut impl TextMeasure,
) -> String {
    let mut svg = String::new();
    let child_states: Vec<&State> = state.child_state_ids().map(|id| states_by_id[id]).collect();
    if child_states.is_empty() {
        return svg;
    }

    let child_transitions: Vec<&StateTransition> = state
        .children
        .iter()
        .filter_map(|child| match child {
            StateElement::Transition(transition) => Some(transition),
            _ => None,
        })
        .collect();

    let mut child_positions: HashMap<String, Rect> = HashMap::new();
    let header_h = style.font_size * 2.0 + 16.0;
    let inner_top = parent_pos.y + header_h + STATE_INNER_PAD;
    let content_left = parent_pos.x + STATE_INNER_PAD + STATE_ROUTE_LANE;
    let content_width = parent_pos.w - STATE_INNER_PAD * 2.0 - STATE_ROUTE_LANE * 2.0;

    let mut y_cursor = inner_top;
    for child_state in child_states {
        let (child_w, child_h) =
            LayoutEngine::new(measure, style.font_size).state_size(child_state, states_by_id);
        let child_pos = Rect::new(content_left, y_cursor, content_width.max(child_w), child_h);
        child_positions.insert(child_state.id.clone(), child_pos);

        svg.push_str(&render_state_node(
            child_state,
            &child_pos,
            style,
            states_by_id,
            positions,
            measure,
        ));

        y_cursor += child_h + STATE_CHILD_GAP;
    }

    let mut pair_totals: HashMap<(String, String), usize> = HashMap::new();
    for transition in &child_transitions {
        *pair_totals
            .entry(state_pair_key(&transition.from, &transition.to))
            .or_insert(0) += 1;
    }

    let mut child_obstacles: Vec<Rect> =
        child_positions.values().map(|p| p.expanded(3.0)).collect();
    // Include global positions but exclude the parent composite (we're routing inside it)
    child_obstacles.extend(
        positions
            .values()
            .filter(|p| !p.overlaps(parent_pos) || p.w < parent_pos.w * 0.5)
            .map(|p| p.expanded(3.0)),
    );

    let mut pair_seen: HashMap<(String, String), usize> = HashMap::new();
    let mut occupied_labels: Vec<Rect> = Vec::new();
    for transition in child_transitions {
        let key = state_pair_key(&transition.from, &transition.to);
        let route_index = pair_seen.get(&key).copied().unwrap_or(0);
        pair_seen.insert(key.clone(), route_index + 1);
        let route_total = pair_totals.get(&key).copied().unwrap_or(1);

        let from = child_positions
            .get(&transition.from)
            .or_else(|| positions.get(&transition.from));
        let to = child_positions
            .get(&transition.to)
            .or_else(|| positions.get(&transition.to));
        if let (Some(from_pos), Some(to_pos)) = (from, to) {
            let (t_svg, _ext) = render_state_transition(&mut StateTransitionContext {
                transition,
                from: from_pos,
                to: to_pos,
                style,
                measure,
                route_index,
                route_total,
                occupied_labels: &mut occupied_labels,
                obstacles: &child_obstacles,
            });
            svg.push_str(&t_svg);
        }
    }

    svg
}

fn render_state_note(
    text: &str,
    state_pos: &Rect,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
) -> String {
    let cleaned = sanitize_xml_text(text);
    let note_width = (measure.measure_width(&cleaned, style.font_size * 0.8, false, false, false)
        + 16.0)
        .clamp(72.0, 180.0);
    let note_height = 26.0;
    let x = state_pos.x + state_pos.w + 28.0;
    let y = state_pos.y + 4.0;

    let y_mid = y + note_height / 2.0;
    let line_x1 = state_pos.x + state_pos.w;
    let line_x2 = x;

    format!(
        r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1" stroke-dasharray="4,3" /><rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="3" fill="{}" fill-opacity="0.25" stroke="{}" stroke-width="1" /><text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}">{}</text>"#,
        line_x1,
        y_mid,
        line_x2,
        y_mid,
        style.edge_stroke,
        x,
        y,
        note_width,
        note_height,
        style.node_fill,
        style.node_stroke,
        x + 8.0,
        y + note_height * 0.65,
        style.font_family,
        style.font_size * 0.8,
        style.edge_text,
        escape_xml(&cleaned)
    )
}

struct StateTransitionContext<'a, T: TextMeasure> {
    transition: &'a StateTransition,
    from: &'a Rect,
    to: &'a Rect,
    style: &'a DiagramStyle,
    measure: &'a mut T,
    route_index: usize,
    route_total: usize,
    occupied_labels: &'a mut Vec<Rect>,
    obstacles: &'a [Rect],
}

fn render_state_transition<T: TextMeasure>(
    ctx: &mut StateTransitionContext<'_, T>,
) -> (String, Rect) {
    let transition = ctx.transition;
    let from = ctx.from;
    let to = ctx.to;
    let style = ctx.style;
    let measure = &mut *ctx.measure;
    let route_index = ctx.route_index;
    let route_total = ctx.route_total;
    let occupied_labels = &mut *ctx.occupied_labels;
    let obstacles = ctx.obstacles;

    if transition.from == transition.to {
        return render_state_self_transition(transition, from, style, measure, occupied_labels);
    }

    let mut svg = String::new();
    let mut ext_min_x = f32::MAX;
    let mut ext_min_y = f32::MAX;
    let mut ext_max_x = f32::MIN;
    let mut ext_max_y = f32::MIN;

    macro_rules! track_point {
        ($x:expr, $y:expr) => {
            ext_min_x = ext_min_x.min($x);
            ext_min_y = ext_min_y.min($y);
            ext_max_x = ext_max_x.max($x);
            ext_max_y = ext_max_y.max($y);
        };
    }

    let (from_cx, from_cy) = from.center();
    let (to_cx, to_cy) = to.center();
    let center_angle = (to_cy - from_cy).atan2(to_cx - from_cx);

    let (px1, py1) = if from.w == from.h && from.w < 30.0 {
        (
            from_cx + center_angle.cos() * (from.w / 2.0),
            from_cy + center_angle.sin() * (from.w / 2.0),
        )
    } else {
        rect_boundary_point(from, center_angle)
    };

    let (px2, py2) = if to.w == to.h && to.w < 30.0 {
        (
            to_cx + (center_angle + std::f32::consts::PI).cos() * (to.w / 2.0),
            to_cy + (center_angle + std::f32::consts::PI).sin() * (to.w / 2.0),
        )
    } else {
        rect_boundary_point(to, center_angle + std::f32::consts::PI)
    };

    let route_hash = transition
        .from
        .bytes()
        .chain(transition.to.bytes())
        .fold(0_u32, |acc, value| {
            acc.wrapping_mul(31).wrapping_add(value as u32)
        });
    let lane = if route_total > 1 {
        route_index as f32 - (route_total as f32 - 1.0) / 2.0
    } else {
        0.0
    };
    let lane_offset = lane * 30.0;
    let global_lane = (route_hash % 5) as f32 - 2.0;
    let global_offset = global_lane * 6.0;
    let route_side = match transition.from.cmp(&transition.to) {
        std::cmp::Ordering::Less => 1.0,
        std::cmp::Ordering::Greater => -1.0,
        std::cmp::Ordering::Equal => {
            if route_hash % 2 == 0 {
                1.0
            } else {
                -1.0
            }
        }
    };
    let verticalish =
        (from_cx - to_cx).abs() < (from.w.min(to.w)) / 2.0 && (py2 - py1).abs() > 30.0;

    let label_anchor_x;
    let label_anchor_y;
    let arrow_angle;
    let mut arrow_x = px2;
    let mut arrow_y = py2;

    if verticalish {
        // Determine if from and to are adjacent (no boxes between them)
        let gap = (to.y - from.bottom()).max(from.y - to.bottom());
        let (top_y, bot_y) = if from_cy < to_cy {
            (from.bottom(), to.y)
        } else {
            (to.bottom(), from.y)
        };
        let mid_x = (from_cx + to_cx) / 2.0;
        let is_src_or_dst = |r: &Rect| -> bool {
            let rcx = r.x + r.w / 2.0;
            let rcy = r.y + r.h / 2.0;
            ((rcx - from_cx).abs() < 1.0 && (rcy - from_cy).abs() < 1.0)
                || ((rcx - to_cx).abs() < 1.0 && (rcy - to_cy).abs() < 1.0)
        };
        let has_obstacle_between = gap > 0.0
            && obstacles.iter().any(|r| {
                if is_src_or_dst(r) {
                    return false;
                }
                // Check if a straight vertical line from from→to would cross this obstacle
                let line_x = mid_x;
                line_x >= r.x && line_x <= r.x + r.w && r.y < bot_y && r.y + r.h > top_y
            });
        // Also check if the straight line crosses any obstacle using full intersection test
        let straight_crosses = obstacles.iter().any(|r| {
            if is_src_or_dst(r) {
                return false;
            }
            line_intersects_rect(px1, py1, px2, py2, r)
        });
        let adjacent = gap > 0.0 && gap < 60.0 && !has_obstacle_between && !straight_crosses;

        if adjacent {
            // Adjacent states: draw a straight vertical line
            let x = (from_cx + to_cx) / 2.0;
            let y1 = if from_cy < to_cy {
                from.bottom()
            } else {
                from.y
            };
            let y2 = if from_cy < to_cy { to.y } else { to.bottom() };
            svg.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" />"#,
                x, y1, x, y2, style.edge_stroke
            ));
            track_point!(x, y1);
            track_point!(x, y2);
            label_anchor_x = x;
            label_anchor_y = (y1 + y2) / 2.0;
            arrow_angle = if from_cy < to_cy {
                -std::f32::consts::FRAC_PI_2 // arrowhead points up (into top of target)
            } else {
                std::f32::consts::FRAC_PI_2
            };
            arrow_x = x;
            arrow_y = y2;
        } else {
            // Non-adjacent: use orthogonal routing (out → down → in)
            let max_half_width = (from.w / 2.0).max(to.w / 2.0);
            let exit_y = from_cy;
            let enter_y = to_cy;
            let is_endpoint = |r: &Rect| -> bool {
                let rcx = r.x + r.w / 2.0;
                let rcy = r.y + r.h / 2.0;
                ((rcx - from_cx).abs() < 1.0 && (rcy - from_cy).abs() < 1.0)
                    || ((rcx - to_cx).abs() < 1.0 && (rcy - to_cy).abs() < 1.0)
            };

            // Try both sides and pick the one that clears first (fewer steps)
            let step = 18.0;
            let max_steps = 30;

            let mut best_lane_x = from_cx + route_side * (max_half_width + 30.0);
            let mut best_step_count = max_steps;

            for &try_side in &[route_side, -route_side] {
                let base = from_cx + try_side * (max_half_width + 30.0 + lane.abs() * 14.0);
                let fex = if base >= from_cx {
                    from.x + from.w
                } else {
                    from.x
                };
                let tex = if base >= to_cx { to.x + to.w } else { to.x };
                for i in 0..max_steps {
                    let candidate = base + try_side * (i as f32) * step;
                    let clear = obstacles.iter().all(|r| {
                        if is_endpoint(r) {
                            return true;
                        }
                        let rr = r.expanded(6.0);
                        !hseg_hits_rect(exit_y, fex, candidate, &rr)
                            && !vseg_hits_rect(candidate, exit_y, enter_y, &rr)
                            && !hseg_hits_rect(enter_y, candidate, tex, &rr)
                    });
                    if clear && i < best_step_count {
                        best_lane_x = candidate;
                        best_step_count = i;
                        break;
                    }
                }
            }

            let lane_x = best_lane_x;
            let from_exit_x = if lane_x >= from_cx {
                from.x + from.w
            } else {
                from.x
            };
            let to_enter_x = if lane_x >= to_cx { to.x + to.w } else { to.x };

            svg.push_str(&format!(
                r#"<polyline points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="none" stroke="{}" stroke-width="0.75" />"#,
                from_exit_x, exit_y, lane_x, exit_y, lane_x, enter_y, to_enter_x, enter_y,
                style.edge_stroke
            ));
            track_point!(from_exit_x, exit_y);
            track_point!(lane_x, exit_y);
            track_point!(lane_x, enter_y);
            track_point!(to_enter_x, enter_y);
            label_anchor_x = lane_x;
            label_anchor_y = (exit_y + enter_y) / 2.0;
            arrow_angle = if to_enter_x > lane_x {
                0.0 // pointing right
            } else {
                std::f32::consts::PI // pointing left
            };
            arrow_x = to_enter_x;
            arrow_y = enter_y;
        }
    } else {
        // Check if a straight line would cross any obstacles
        let is_from_or_to = |r: &Rect| -> bool {
            let rcx = r.x + r.w / 2.0;
            let rcy = r.y + r.h / 2.0;
            ((rcx - from_cx).abs() < 1.0 && (rcy - from_cy).abs() < 1.0)
                || ((rcx - to_cx).abs() < 1.0 && (rcy - to_cy).abs() < 1.0)
        };
        let straight_blocked = obstacles.iter().any(|r| {
            if is_from_or_to(r) {
                return false;
            }
            line_intersects_rect(px1, py1, px2, py2, r)
        });

        if straight_blocked {
            // Obstacle-aware orthogonal routing for non-verticalish blocked paths.
            // We try multiple strategies and pick the first clear one:
            //   1. Simple Z-route (exit side → horizontal mid → enter side)
            //   2. U-route via vertical center exit (exit top/bottom → horizontal lane → enter top/bottom)
            //   3. Side-exit L-route (exit side → vertical lane → enter side) - like verticalish routing
            let is_endpoint = |r: &Rect| -> bool {
                let rcx = r.x + r.w / 2.0;
                let rcy = r.y + r.h / 2.0;
                ((rcx - from_cx).abs() < 1.0 && (rcy - from_cy).abs() < 1.0)
                    || ((rcx - to_cx).abs() < 1.0 && (rcy - to_cy).abs() < 1.0)
            };

            let step = 18.0;
            let max_steps = 30;

            // Strategy 1: Simple Z-route (exit from side, horizontal midline, enter from side)
            let going_right = to_cx > from_cx;
            let simple_from_exit_x = if going_right { from.x + from.w } else { from.x };
            let simple_to_enter_x = if going_right { to.x } else { to.x + to.w };
            let mid_y = (from_cy + to_cy) / 2.0;
            let simple_clear = obstacles.iter().all(|r| {
                if is_endpoint(r) {
                    return true;
                }
                let rr = r.expanded(6.0);
                !vseg_hits_rect(simple_from_exit_x, from_cy, mid_y, &rr)
                    && !hseg_hits_rect(mid_y, simple_from_exit_x, simple_to_enter_x, &rr)
                    && !vseg_hits_rect(simple_to_enter_x, mid_y, to_cy, &rr)
            });

            if simple_clear {
                // Use simpler Z-route since it's clear
                svg.push_str(&format!(
                    r#"<polyline points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="none" stroke="{}" stroke-width="0.75" />"#,
                    simple_from_exit_x, from_cy, simple_from_exit_x, mid_y, simple_to_enter_x, mid_y, simple_to_enter_x, to_cy,
                    style.edge_stroke
                ));
                track_point!(simple_from_exit_x, from_cy);
                track_point!(simple_from_exit_x, mid_y);
                track_point!(simple_to_enter_x, mid_y);
                track_point!(simple_to_enter_x, to_cy);
                label_anchor_x = (simple_from_exit_x + simple_to_enter_x) / 2.0;
                label_anchor_y = mid_y;
                arrow_angle = if to_cy > mid_y {
                    -std::f32::consts::FRAC_PI_2
                } else {
                    std::f32::consts::FRAC_PI_2
                };
                arrow_x = simple_to_enter_x;
                arrow_y = to_cy;
            } else {
                // Strategy 2: U-route via vertical center exit
                let going_down = to_cy > from_cy;
                let pref_side: f32 = if going_down { 1.0 } else { -1.0 };
                let mut best_u_lane_y = f32::NAN;
                let mut best_u_steps = max_steps;

                for &try_side in &[pref_side, -pref_side] {
                    let base = if try_side > 0.0 {
                        from.bottom().max(to.bottom()) + 30.0 + lane.abs() * 14.0
                    } else {
                        from.y.min(to.y) - 30.0 - lane.abs() * 14.0
                    };
                    let fey = if try_side > 0.0 {
                        from.bottom()
                    } else {
                        from.y
                    };
                    let tey = if try_side > 0.0 { to.bottom() } else { to.y };
                    for i in 0..max_steps {
                        let candidate = base + try_side * (i as f32) * step;
                        let clear = obstacles.iter().all(|r| {
                            if is_endpoint(r) {
                                return true;
                            }
                            let rr = r.expanded(6.0);
                            !vseg_hits_rect(from_cx, fey, candidate, &rr)
                                && !hseg_hits_rect(candidate, from_cx, to_cx, &rr)
                                && !vseg_hits_rect(to_cx, candidate, tey, &rr)
                        });
                        if clear && i < best_u_steps {
                            best_u_lane_y = candidate;
                            best_u_steps = i;
                            break;
                        }
                    }
                }

                // Strategy 3: Side-exit L-route (exit from side, vertical lane, enter from side)
                // Same approach as the verticalish non-adjacent routing.
                let max_half_width = (from.w / 2.0).max(to.w / 2.0);
                let exit_y = from_cy;
                let enter_y = to_cy;
                let mut best_side_lane_x = f32::NAN;
                let mut best_side_steps = max_steps;

                for &try_side in &[route_side, -route_side] {
                    let base_x = from_cx + try_side * (max_half_width + 30.0 + lane.abs() * 14.0);
                    let fex = if base_x >= from_cx {
                        from.x + from.w
                    } else {
                        from.x
                    };
                    let tex = if base_x >= to_cx { to.x + to.w } else { to.x };
                    for i in 0..max_steps {
                        let candidate = base_x + try_side * (i as f32) * step;
                        let clear = obstacles.iter().all(|r| {
                            if is_endpoint(r) {
                                return true;
                            }
                            let rr = r.expanded(6.0);
                            !hseg_hits_rect(exit_y, fex, candidate, &rr)
                                && !vseg_hits_rect(candidate, exit_y, enter_y, &rr)
                                && !hseg_hits_rect(enter_y, candidate, tex, &rr)
                        });
                        if clear && i < best_side_steps {
                            best_side_lane_x = candidate;
                            best_side_steps = i;
                            break;
                        }
                    }
                }

                // Pick the best strategy: prefer fewer steps (closer route)
                let use_u_route = !best_u_lane_y.is_nan()
                    && (best_side_lane_x.is_nan() || best_u_steps <= best_side_steps);

                if use_u_route {
                    let lane_y = best_u_lane_y;
                    let from_exit_y = if lane_y > from_cy {
                        from.bottom()
                    } else {
                        from.y
                    };
                    let to_enter_y = if lane_y > to_cy { to.bottom() } else { to.y };

                    svg.push_str(&format!(
                        r#"<polyline points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="none" stroke="{}" stroke-width="0.75" />"#,
                        from_cx, from_exit_y, from_cx, lane_y, to_cx, lane_y, to_cx, to_enter_y,
                        style.edge_stroke
                    ));
                    track_point!(from_cx, from_exit_y);
                    track_point!(from_cx, lane_y);
                    track_point!(to_cx, lane_y);
                    track_point!(to_cx, to_enter_y);
                    label_anchor_x = (from_cx + to_cx) / 2.0;
                    label_anchor_y = lane_y;
                    arrow_angle = if to_enter_y > lane_y {
                        -std::f32::consts::FRAC_PI_2
                    } else {
                        std::f32::consts::FRAC_PI_2
                    };
                    arrow_x = to_cx;
                    arrow_y = to_enter_y;
                } else if !best_side_lane_x.is_nan() {
                    let lane_x = best_side_lane_x;
                    let from_exit_x = if lane_x >= from_cx {
                        from.x + from.w
                    } else {
                        from.x
                    };
                    let to_enter_x = if lane_x >= to_cx { to.x + to.w } else { to.x };

                    svg.push_str(&format!(
                        r#"<polyline points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="none" stroke="{}" stroke-width="0.75" />"#,
                        from_exit_x, exit_y, lane_x, exit_y, lane_x, enter_y, to_enter_x, enter_y,
                        style.edge_stroke
                    ));
                    track_point!(from_exit_x, exit_y);
                    track_point!(lane_x, exit_y);
                    track_point!(lane_x, enter_y);
                    track_point!(to_enter_x, enter_y);
                    label_anchor_x = lane_x;
                    label_anchor_y = (exit_y + enter_y) / 2.0;
                    arrow_angle = if to_enter_x > lane_x {
                        0.0
                    } else {
                        std::f32::consts::PI
                    };
                    arrow_x = to_enter_x;
                    arrow_y = enter_y;
                } else {
                    // Fallback: draw the straight line anyway
                    svg.push_str(&format!(
                        r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" />"#,
                        px1, py1, px2, py2, style.edge_stroke
                    ));
                    track_point!(px1, py1);
                    track_point!(px2, py2);
                    arrow_angle = (py1 - py2).atan2(px1 - px2);
                    label_anchor_x = (px1 + px2) / 2.0;
                    label_anchor_y = (py1 + py2) / 2.0;
                }
            }
        } else {
            svg.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" />"#,
                px1, py1, px2, py2, style.edge_stroke
            ));
            track_point!(px1, py1);
            track_point!(px2, py2);
            arrow_angle = (py1 - py2).atan2(px1 - px2);

            // Jitter label anchor along the edge to reduce label-label collisions.
            let dx = px2 - px1;
            let dy = py2 - py1;
            let jitter = ((route_hash % 7) as f32 - 3.0) * 0.07;
            let t = (0.5 + jitter).clamp(0.25, 0.75);
            label_anchor_x = px1 + dx * t;
            label_anchor_y = py1 + dy * t;
        }
    }

    // `arrow_angle` points back along the edge, away from the target.
    svg.push_str(&arrowhead(
        ArrowHead::Filled,
        arrow_x,
        arrow_y,
        arrow_angle + std::f32::consts::PI,
        style,
    ));

    // Label
    if let Some(ref label) = transition.label {
        let label_font = style.font_size * 0.85;
        let (label_width, label_height) = pill_size(measure, label, label_font);
        let dx = px2 - px1;
        let dy = py2 - py1;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let tx = dx / len;
        let ty = dy / len;
        let perp_x = -ty;
        let perp_y = tx;
        let tangent_offset = lane_offset + global_offset;

        let rect_for = |lx: f32, ly: f32| {
            Rect::new(
                lx - label_width / 2.0,
                ly - label_height / 2.0,
                label_width,
                label_height,
            )
        };

        let score = |r: &Rect| -> f32 {
            let mut s = 0.0;
            if r.x < 0.0 || r.y < 0.0 {
                s += 1000.0;
            }
            for o in obstacles {
                if r.overlaps(o) {
                    s += 140.0;
                }
            }
            for o in occupied_labels.iter() {
                if r.overlaps(o) {
                    s += 220.0;
                }
            }
            s
        };

        // Candidate search: try different anchor t and perpendicular distances on both sides.
        let t_candidates: [f32; 5] = [0.38, 0.46, 0.5, 0.54, 0.62];
        let dist_candidates: [f32; 6] = [24.0, 34.0, 44.0, 54.0, 66.0, 78.0];
        let side_candidates: [f32; 2] = [route_side, -route_side];

        // Always include the previous heuristic as a candidate.
        let movement_weight = 2.0;
        let base_dist = 28.0 + lane.abs() * 10.0 + global_lane.abs() * 4.0;
        let base_x = label_anchor_x + perp_x * base_dist * route_side + tx * tangent_offset;
        let base_y = label_anchor_y + perp_y * base_dist * route_side + ty * tangent_offset;
        let base_rect = rect_for(base_x, base_y);
        let base_score = score(&base_rect);
        let mut best_x = base_x;
        let mut best_y = base_y;
        let mut best_move =
            ((base_x - label_anchor_x).powi(2) + (base_y - label_anchor_y).powi(2)).sqrt();
        let mut best_cost = base_score + best_move * movement_weight;

        for &t in &t_candidates {
            let ax = px1 + dx * t;
            let ay = py1 + dy * t;
            for &side in &side_candidates {
                for &dist in &dist_candidates {
                    let lx = ax + perp_x * dist * side + tx * tangent_offset;
                    let ly = ay + perp_y * dist * side + ty * tangent_offset;
                    let r = rect_for(lx, ly);
                    let sc = score(&r);
                    let mv = ((lx - label_anchor_x).powi(2) + (ly - label_anchor_y).powi(2)).sqrt();
                    let cost = sc + mv * movement_weight;
                    if cost < best_cost
                        || ((cost - best_cost).abs() < f32::EPSILON && mv < best_move)
                    {
                        best_cost = cost;
                        best_move = mv;
                        best_x = lx;
                        best_y = ly;
                    }
                }
            }
        }

        // For vertical-ish routes (polyline), also try shifting sideways.
        if verticalish {
            for &side in &side_candidates {
                for &dist in &dist_candidates {
                    let lx = label_anchor_x + side * dist;
                    let ly = label_anchor_y + lane_offset * 0.5;
                    let r = rect_for(lx, ly);
                    let sc = score(&r);
                    let mv = ((lx - label_anchor_x).powi(2) + (ly - label_anchor_y).powi(2)).sqrt();
                    let cost = sc + mv * movement_weight;
                    if cost < best_cost
                        || ((cost - best_cost).abs() < f32::EPSILON && mv < best_move)
                    {
                        best_cost = cost;
                        best_move = mv;
                        best_x = lx;
                        best_y = ly;
                    }
                }
            }
        }

        let (pill, rect) = label_pill(measure, style, label, label_font, (best_x, best_y));
        svg.push_str(&pill);
        occupied_labels.push(rect);
        track_point!(rect.x, rect.y);
        track_point!(rect.right(), rect.bottom());
    }

    let extent = Rect {
        x: ext_min_x.min(0.0),
        y: ext_min_y.min(0.0),
        w: (ext_max_x - ext_min_x.min(0.0)).max(0.0),
        h: (ext_max_y - ext_min_y.min(0.0)).max(0.0),
    };
    (svg, extent)
}

fn render_state_self_transition(
    transition: &StateTransition,
    pos: &Rect,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
    occupied_labels: &mut Vec<Rect>,
) -> (String, Rect) {
    let self_loop = SelfLoop::new(pos, FlowDirection::TopDown);
    let mut svg = format!(
        r#"<path d="{}" fill="none" stroke="{}" stroke-width="0.75" />"#,
        self_loop.path_data(),
        style.edge_stroke
    );
    let (ax, ay) = self_loop.end;
    svg.push_str(&arrowhead(
        ArrowHead::Filled,
        ax,
        ay,
        self_loop.end_angle(),
        style,
    ));

    // The curve stays inside the hull of its control points.
    let points = [
        self_loop.start,
        self_loop.control1,
        self_loop.control2,
        self_loop.end,
    ];
    let mut extent = bounding_rect(&points);

    if let Some(label) = &transition.label {
        let font_size = style.font_size * 0.85;
        let (label_w, _) = pill_size(measure, label, font_size);
        let center = (
            self_loop.control1.0 + 4.0 + label_w / 2.0,
            self_loop.control1.1,
        );
        let (pill, rect) = label_pill(measure, style, label, font_size, center);
        svg.push_str(&pill);
        occupied_labels.push(rect);
        extent = bounding_rect(&[
            (extent.x, extent.y),
            (extent.right(), extent.bottom()),
            (rect.x, rect.y),
            (rect.right(), rect.bottom()),
        ]);
    }

    (svg, extent)
}

fn bounding_rect(points: &[(f32, f32)]) -> Rect {
    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
    let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
    for &(x, y) in points {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
}

fn state_pair_key(from: &str, to: &str) -> (String, String) {
    if from <= to {
        (from.to_string(), to.to_string())
    } else {
        (to.to_string(), from.to_string())
    }
}

// ============================================
// ER DIAGRAM RENDERING
// ============================================

fn render_er(
    diagram: &ErDiagram,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
) -> (String, f32, f32) {
    let mut layout = LayoutEngine::new(measure, style.font_size);
    let (positions, bbox) = layout.layout_er(diagram);

    let mut svg = String::new();
    let padding = 20.0;
    let mut occupied_labels: Vec<Rect> = Vec::new();

    // Draw relationships first
    for relation in &diagram.relationships {
        let from_pos = positions.get(&relation.from);
        let to_pos = positions.get(&relation.to);

        if let (Some(from), Some(to)) = (from_pos, to_pos) {
            svg.push_str(&render_er_relationship(
                relation,
                from,
                to,
                style,
                measure,
                &mut occupied_labels,
            ));
        }
    }

    // Draw entities
    for entity in &diagram.entities {
        if let Some(pos) = positions.get(&entity.name) {
            svg.push_str(&render_er_entity(entity, pos, style));
        }
    }

    let total_width = bbox.right() + padding;
    let total_height = bbox.bottom() + padding;

    (svg, total_width, total_height)
}

fn render_er_entity(entity: &ErEntity, pos: &Rect, style: &DiagramStyle) -> String {
    let mut svg = String::new();

    // Entity box
    svg.push_str(&format!(
        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
        pos.x, pos.y, pos.w, pos.h,
        style.node_fill, style.node_stroke
    ));

    let mut y = pos.y + style.font_size + 6.0;

    // Entity name (bold)
    svg.push_str(&format!(
        r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}" text-anchor="middle" font-weight="bold">{}</text>"#,
        pos.x + pos.w / 2.0, y, style.font_family, style.font_size, style.node_text, escape_xml(&entity.name)
    ));

    // Divider line between name and attributes
    if !entity.attributes.is_empty() {
        y += 4.0;
        svg.push_str(&format!(
            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" />"#,
            pos.x, y, pos.x + pos.w, y, style.node_stroke
        ));
        y += style.font_size * 0.5;
    }

    // Attributes
    y += style.font_size;
    for attr in &entity.attributes {
        svg.push_str(&format!(
            r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}">{}</text>"#,
            pos.x + 8.0,
            y,
            style.font_family,
            style.font_size * ER_ATTRIBUTE_FONT_SCALE,
            style.node_text,
            escape_xml(&attr.display())
        ));
        y += style.font_size * 1.3;
    }

    svg
}

fn render_er_relationship(
    relation: &ErRelationship,
    from: &Rect,
    to: &Rect,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
    occupied_labels: &mut Vec<Rect>,
) -> String {
    let mut svg = String::new();

    let (from_cx, from_cy) = from.center();
    let (to_cx, to_cy) = to.center();
    let angle = (to_cy - from_cy).atan2(to_cx - from_cx);
    let (x1, y1) = rect_boundary_point(from, angle);
    let (x2, y2) = rect_boundary_point(to, angle + std::f32::consts::PI);

    // Line
    svg.push_str(&format!(
        r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" />"#,
        x1, y1, x2, y2, style.edge_stroke
    ));

    svg.push_str(&render_er_cardinality_marker(
        x1,
        y1,
        angle,
        &relation.from_cardinality,
        style,
    ));
    svg.push_str(&render_er_cardinality_marker(
        x2,
        y2,
        angle + std::f32::consts::PI,
        &relation.to_cardinality,
        style,
    ));

    if let Some(label) = &relation.label {
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let (tx, ty) = (dx / len, dy / len);
        let (nx, ny) = (-ty, tx);
        let mx = (x1 + x2) / 2.0;
        let my = (y1 + y2) / 2.0;
        let label_font = style.font_size * 0.8;
        let (label_w, label_h) = pill_size(measure, label, label_font);
        let rect_for =
            |lx: f32, ly: f32| Rect::new(lx - label_w / 2.0, ly - label_h / 2.0, label_w, label_h);
        let from_r = from.expanded(3.0);
        let to_r = to.expanded(3.0);

        // Search outward from the line on both sides; the first spot clear of
        // both entities and earlier labels wins.
        let mut best = (mx, my);
        let mut best_score = i32::MAX;
        'search: for off in [22.0_f32, 32.0, 44.0, 56.0, 68.0, 80.0] {
            for tangent in [-28.0_f32, -14.0, 0.0, 14.0, 28.0] {
                for side in [1.0_f32, -1.0] {
                    let lx = mx + side * nx * off + tx * tangent;
                    let ly = my + side * ny * off + ty * tangent;
                    let r = rect_for(lx, ly);
                    let mut sc = 0;
                    if r.overlaps(&from_r) {
                        sc += 500;
                    }
                    if r.overlaps(&to_r) {
                        sc += 500;
                    }
                    for o in occupied_labels.iter() {
                        if r.overlaps(o) {
                            sc += 800;
                        }
                    }
                    if sc < best_score {
                        best_score = sc;
                        best = (lx, ly);
                        if best_score == 0 {
                            break 'search;
                        }
                    }
                }
            }
        }

        let (pill, rect) = label_pill(measure, style, label, label_font, best);
        svg.push_str(&pill);
        occupied_labels.push(rect);
    }

    svg
}

fn rect_boundary_point(rect: &Rect, angle: f32) -> (f32, f32) {
    let (cx, cy) = rect.center();
    let dx = angle.cos();
    let dy = angle.sin();
    let half_w = rect.w / 2.0;
    let half_h = rect.h / 2.0;

    let tx = if dx.abs() > 1e-5 {
        half_w / dx.abs()
    } else {
        f32::INFINITY
    };
    let ty = if dy.abs() > 1e-5 {
        half_h / dy.abs()
    } else {
        f32::INFINITY
    };
    let t = tx.min(ty);

    (cx + dx * t, cy + dy * t)
}

fn render_er_cardinality_marker(
    x: f32,
    y: f32,
    angle: f32,
    cardinality: &ErCardinality,
    style: &DiagramStyle,
) -> String {
    let ux = angle.cos();
    let uy = angle.sin();
    let nx = -uy;
    let ny = ux;

    let mut marker = String::new();

    let draw_one = |dist: f32| {
        format!(
            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" />"#,
            x + ux * dist + nx * 6.0,
            y + uy * dist + ny * 6.0,
            x + ux * dist - nx * 6.0,
            y + uy * dist - ny * 6.0,
            style.edge_stroke
        )
    };

    let draw_zero = |dist: f32| {
        format!(
            r#"<circle cx="{:.2}" cy="{:.2}" r="4.50" fill="{}" stroke="{}" stroke-width="1.2" />"#,
            x + ux * dist,
            y + uy * dist,
            style.background,
            style.edge_stroke
        )
    };

    let draw_many = |dist: f32| {
        let cx = x + ux * dist;
        let cy = y + uy * dist;
        format!(
            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" /><line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" /><line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" />"#,
            cx,
            cy,
            cx + ux * 8.0 + nx * 8.0,
            cy + uy * 8.0 + ny * 8.0,
            style.edge_stroke,
            cx,
            cy,
            cx + ux * 10.0,
            cy + uy * 10.0,
            style.edge_stroke,
            cx,
            cy,
            cx + ux * 8.0 - nx * 8.0,
            cy + uy * 8.0 - ny * 8.0,
            style.edge_stroke
        )
    };

    match cardinality {
        ErCardinality::ExactlyOne => {
            marker.push_str(&draw_one(8.0));
            marker.push_str(&draw_one(14.0));
        }
        ErCardinality::ZeroOrOne => {
            marker.push_str(&draw_zero(8.0));
            marker.push_str(&draw_one(16.0));
        }
        ErCardinality::ZeroOrMore => {
            marker.push_str(&draw_zero(8.0));
            marker.push_str(&draw_many(16.0));
        }
        ErCardinality::OneOrMore => {
            marker.push_str(&draw_one(8.0));
            marker.push_str(&draw_many(16.0));
        }
    }

    marker
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::markdown::markie::MockMeasure;
    use image::{Rgba, RgbaImage};
    use quick_xml::{Reader, events::Event};

    fn test_style() -> DiagramStyle {
        DiagramStyle {
            node_fill: "#ff0000".into(),
            node_stroke: "#00ff00".into(),
            edge_stroke: "#0000ff".into(),
            ..DiagramStyle::default()
        }
    }

    fn render_svg(source: &str) -> String {
        let (content, width, height) =
            render_diagram(source, &test_style(), &mut MockMeasure).unwrap();
        // Leave a margin around the reported size so drawing that overflows it
        // stays measurable instead of being clipped away.
        let (width, height) = (width + 40.0, height + 40.0);
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">{content}</svg>"#
        )
    }

    fn visible_text(svg: &str) -> Vec<String> {
        let mut reader = Reader::from_str(svg);
        let mut text = Vec::new();
        loop {
            match reader.read_event().unwrap() {
                Event::Text(t) => text.push(t.xml10_content().into_owned()),
                Event::Eof => break,
                _ => {}
            }
        }
        text
    }

    fn rasterize(svg: &str) -> RgbaImage {
        crate::display::markdown::markie::svg_to_image(svg)
            .unwrap()
            .to_rgba8()
    }

    fn is_node_fill(p: &Rgba<u8>) -> bool {
        p[3] > 32 && p[0] > 180 && p[1] < 60 && p[2] < 60
    }

    fn is_edge(p: &Rgba<u8>) -> bool {
        p[3] > 32 && p[2] > 180 && p[0] < 60 && p[1] < 60
    }

    /// Inclusive pixel bounds `(left, top, right, bottom)` of matching pixels.
    fn bounds(image: &RgbaImage, pred: impl Fn(&Rgba<u8>) -> bool) -> (u32, u32, u32, u32) {
        let mut found = None;
        for (x, y, p) in image.enumerate_pixels() {
            if pred(p) {
                let (l, t, r, b) = found.unwrap_or((x, y, x, y));
                found = Some((l.min(x), t.min(y), r.max(x), b.max(y)));
            }
        }
        found.expect("no matching pixels")
    }

    fn assert_renders_visible(name: &str, source: &str, labels: &[&str]) -> Vec<String> {
        let svg = render_svg(source);
        let text = visible_text(&svg);
        for label in labels {
            assert!(
                text.iter().any(|t| t.contains(label)),
                "{name}: missing visible label {label}, got {text:?}"
            );
        }
        let image = rasterize(&svg);
        let node_pixels = image.pixels().filter(|p| is_node_fill(p)).count();
        let edge_pixels = image.pixels().filter(|p| is_edge(p)).count();
        assert!(
            node_pixels > 100,
            "{name}: nodes must be visible after rasterization, got {node_pixels}"
        );
        assert!(
            edge_pixels > 20,
            "{name}: edges must be visible after rasterization, got {edge_pixels}"
        );
        text
    }

    #[test]
    fn complex_diagrams_render_labels_and_visible_nodes_and_edges() {
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "flowchart",
                r#"flowchart TB
subgraph Workers
    A([Start]) --> B[[Routine]]
    B --> C[(Store)]
    C --> D{{Decision}}
    D --> E[/Input/]
    E --> F[\Output\]
    F --> G[/Batch\]
    G --> H[\Tail/]
    H --> I((Done))
end"#,
                &[
                    "Workers", "Start", "Routine", "Store", "Decision", "Input", "Output", "Batch",
                    "Tail", "Done",
                ],
            ),
            (
                "cyclic flowchart",
                "flowchart TD\n    A[Alpha] --> B[Beta]\n    B --> C[Gamma]\n    C --> A",
                &["Alpha", "Beta", "Gamma"],
            ),
            (
                "sequence",
                r#"sequenceDiagram
participant Alice
participant Bob
Note right of Alice: Start here
alt success
    Alice->>Bob: do work
    Bob->>Bob: local work
else retry
    Bob-->>Alice: try again
end"#,
                &[
                    "Alice",
                    "Bob",
                    "Start here",
                    "do work",
                    "local work",
                    "try again",
                ],
            ),
            (
                "class",
                r#"classDiagram
  class User {
    +String id
  }
  class Session {
    +String token
  }
  class AuditLog {
    +record(event: String): void
  }
  User --> Session : creates
  User ..> AuditLog : writes"#,
                &[
                    "User",
                    "+ String id",
                    "+ record(event: String): void",
                    "creates",
                    "writes",
                ],
            ),
            (
                "state",
                r#"stateDiagram
state Parent {
    state Child
    Child --> Child: loop
}
Note right of Child: child note"#,
                &["Parent", "Child", "loop", "child note"],
            ),
            (
                "er",
                "erDiagram\n    CUSTOMER ||--o{ ORDER : places\n    CUSTOMER {\n        *int id\n    }",
                &["CUSTOMER", "ORDER", "places", "*int id"],
            ),
        ];
        for (name, source, labels) in cases {
            assert_renders_visible(name, source, labels);
        }
    }

    #[test]
    fn sequence_renders_participants_implied_by_messages() {
        assert_renders_visible(
            "implicit participants",
            "sequenceDiagram\n    Alice->>John: Hi",
            &["Alice", "John", "Hi"],
        );
    }

    #[test]
    fn sequence_activation_shorthand_renders_message_to_target() {
        let text = assert_renders_visible(
            "activation shorthand",
            "sequenceDiagram\n    participant A\n    participant B\n    A->>+B: Hello\n    B-->>-A: Hi",
            &["Hello", "Hi"],
        );
        assert!(
            !text.iter().any(|t| t.contains("+B") || t.contains("-A")),
            "activation markers must not become participants: {text:?}"
        );
    }

    #[test]
    fn class_diagram_with_only_relations_renders_classes() {
        assert_renders_visible(
            "relations only",
            "classDiagram\n    Animal <|-- Duck\n    Animal <|-- Fish",
            &["Animal", "Duck", "Fish"],
        );
    }

    #[test]
    fn inheritance_marker_is_hollow_triangle() {
        let svg = render_svg("classDiagram\n  class A\n  class B\n  A <|-- B\n");
        let style = test_style();
        let mut reader = Reader::from_str(&svg);
        let mut polygons = Vec::new();
        loop {
            match reader.read_event().unwrap() {
                Event::Empty(e) if e.name().as_ref() == "polygon" => {
                    let attr = |name: &str| {
                        e.try_get_attribute(name)
                            .unwrap()
                            .map(|a| a.value.into_owned())
                    };
                    polygons.push((attr("points").unwrap(), attr("fill")));
                }
                Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(polygons.len(), 1, "expected one marker, got {polygons:?}");
        let (points, fill) = &polygons[0];
        assert_eq!(points.split_whitespace().count(), 3, "{points}");
        assert_eq!(fill.as_deref(), Some(style.node_fill.as_str()));
    }

    #[test]
    fn state_self_transition_draws_loop_outside_state_box() {
        let image = rasterize(&render_svg("stateDiagram\n    A --> A"));
        let (left, top, right, bottom) = bounds(&image, is_node_fill);
        let outside = image
            .enumerate_pixels()
            .filter(|(x, y, p)| {
                is_edge(p) && (*x + 1 < left || *x > right + 1 || *y + 1 < top || *y > bottom + 1)
            })
            .count();
        assert!(
            outside > 20,
            "self-transition must be drawn outside the state box, got {outside} pixels"
        );
    }

    #[test]
    fn mutually_nested_states_render_without_recursing_forever() {
        let source = "stateDiagram\n    state A {\n        B --> X\n    }\n    state B {\n        A --> Y\n    }";
        assert_renders_visible("nesting cycle", source, &["A", "B", "X"]);
    }

    #[test]
    fn subgraph_box_keeps_equal_padding_when_clamped_at_top() {
        let image = rasterize(&render_svg(
            "flowchart TD\n    subgraph Group\n        A[Alpha] --> B[Beta]\n    end",
        ));
        let (_, _, nodes_right, nodes_bottom) = bounds(&image, |p| is_node_fill(p) && p[3] == 255);
        let (_, _, box_right, box_bottom) = bounds(&image, |p| p[3] > 20);
        let side_gap = box_right as i64 - nodes_right as i64;
        let bottom_gap = box_bottom as i64 - nodes_bottom as i64;
        assert!(
            (side_gap - bottom_gap).abs() <= 2,
            "subgraph padding differs: side {side_gap}, bottom {bottom_gap}"
        );
    }

    #[test]
    fn subgraph_title_pill_fits_cjk_title() {
        let image = rasterize(&render_svg(
            "flowchart TD\n    subgraph 数据流\n        A[Alpha]\n    end",
        ));
        // The title pill is the only node-colored fill drawn at 90% opacity.
        let (left, _, right, _) =
            bounds(&image, |p| is_node_fill(p) && (225..=250).contains(&p[3]));
        let width = right - left + 1;
        // MockMeasure: 3 chars * 0.6 * 11.7px + 12px padding ≈ 33px.
        assert!(
            (28..=40).contains(&width),
            "title pill should fit three CJK characters, got {width}px"
        );
    }

    #[test]
    fn unsupported_and_empty_diagrams_return_err() {
        for source in [
            "pie title Pets\n  \"Dogs\" : 50",
            "flowchart LR\n  ??? invalid syntax ???",
        ] {
            let result = render_diagram(source, &DiagramStyle::default(), &mut MockMeasure);
            assert!(result.is_err(), "{source:?} should be rejected");
        }
    }
}
