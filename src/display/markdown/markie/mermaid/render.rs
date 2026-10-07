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

    let mut renderer = SequenceRenderer {
        centers: &participant_centers,
        style,
        measure,
        y: participant_bottom + 34.0,
        left_edge,
        right_edge,
        last_message_y: None,
        activation_starts: HashMap::new(),
        svg: String::new(),
    };
    renderer.render_elements(&diagram.elements, 0);
    renderer.close_open_activations();
    let message_y = renderer.y;

    let lifeline_end_y = (message_y + 6.0).max(lifeline_start_y + 24.0);
    for participant in &diagram.participants {
        if let Some(x) = participant_centers.get(participant.id.as_str()) {
            svg.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" stroke-dasharray="6,4" />"#,
                x, lifeline_start_y, x, lifeline_end_y, style.edge_stroke
            ));
        }
    }
    svg.push_str(&renderer.svg);

    (svg, diagram_right + padding, message_y + 20.0 + padding)
}

fn sequence_arrowhead(kind: &MessageKind) -> ArrowHead {
    match kind {
        MessageKind::Sync => ArrowHead::Filled,
        MessageKind::Async | MessageKind::Reply => ArrowHead::Open,
    }
}

fn message_dash(msg: &SequenceMessage) -> &'static str {
    if msg.msg_type == MessageType::Dotted || msg.kind == MessageKind::Reply {
        " stroke-dasharray=\"4,4\""
    } else {
        ""
    }
}

/// Draws sequence elements top to bottom, advancing `y` past each one.
struct SequenceRenderer<'a, T: TextMeasure> {
    centers: &'a HashMap<&'a str, f32>,
    style: &'a DiagramStyle,
    measure: &'a mut T,
    y: f32,
    left_edge: f32,
    right_edge: f32,
    /// Row of the latest message arrow, where activations start and end.
    last_message_y: Option<f32>,
    activation_starts: HashMap<String, Vec<f32>>,
    svg: String,
}

impl<T: TextMeasure> SequenceRenderer<'_, T> {
    fn render_elements(&mut self, elements: &[SequenceElement], depth: usize) {
        for element in elements {
            match element {
                SequenceElement::Message(msg) => self.render_message(msg),
                SequenceElement::Activation(activation) => {
                    self.render_activation(&activation.participant)
                }
                SequenceElement::Deactivation(activation) => {
                    self.render_deactivation(&activation.participant)
                }
                SequenceElement::Note {
                    participant,
                    position,
                    text,
                } => self.render_note(participant, position, text),
                SequenceElement::Block(block) => self.render_block(block, depth),
            }
        }
    }

    fn render_message(&mut self, msg: &SequenceMessage) {
        let (Some(&x1), Some(&x2)) = (
            self.centers.get(msg.from.as_str()),
            self.centers.get(msg.to.as_str()),
        ) else {
            return;
        };
        if (x1 - x2).abs() < 0.5 {
            self.render_self_message(msg, x1);
            return;
        }

        let style = self.style;
        let y = self.y;
        self.svg.push_str(&format!(
            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75"{} />"#,
            x1,
            y,
            x2,
            y,
            style.edge_stroke,
            message_dash(msg)
        ));
        let angle = if x2 > x1 { 0.0 } else { std::f32::consts::PI };
        self.svg.push_str(&arrowhead(
            sequence_arrowhead(&msg.kind),
            x2,
            y,
            angle,
            style,
        ));

        self.last_message_y = Some(y);

        if !msg.label.is_empty() {
            let center = ((x1 + x2) / 2.0, y - 10.0);
            let label_font = style.font_size * 0.82;
            let (pill, _) = label_pill(self.measure, style, &msg.label, label_font, center);
            self.svg.push_str(&pill);
        }

        self.y += 50.0;
    }

    /// Draws a message to the sender itself as a loop to the right.
    fn render_self_message(&mut self, msg: &SequenceMessage, cx: f32) {
        let style = self.style;
        let loop_w = 40.0;
        let loop_h = 36.0;
        let y_top = self.y;
        let y_bot = y_top + loop_h;

        self.svg.push_str(&format!(
            r#"<polyline points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="none" stroke="{}" stroke-width="0.75"{} />"#,
            cx,
            y_top,
            cx + loop_w,
            y_top,
            cx + loop_w,
            y_bot,
            cx,
            y_bot,
            style.edge_stroke,
            message_dash(msg)
        ));
        // Arrowhead pointing left at the return point
        self.svg.push_str(&arrowhead(
            sequence_arrowhead(&msg.kind),
            cx,
            y_bot,
            std::f32::consts::PI,
            style,
        ));
        self.last_message_y = Some(y_bot);

        if !msg.label.is_empty() {
            let label_font = style.font_size * 0.82;
            let (pill_w, _) = pill_size(self.measure, &msg.label, label_font);
            let center = (cx + loop_w + 4.0 + pill_w / 2.0, y_top + loop_h / 2.0);
            let (pill, _) = label_pill(self.measure, style, &msg.label, label_font, center);
            self.svg.push_str(&pill);
        }

        self.y = y_bot + 20.0;
    }

    /// Activations and deactivations attach to the latest message, as in
    /// Mermaid; before any message they attach to the next row.
    fn activation_row(&self) -> f32 {
        self.last_message_y.unwrap_or(self.y)
    }

    fn render_activation(&mut self, participant: &str) {
        let start = self.activation_row();
        self.activation_starts
            .entry(participant.to_string())
            .or_default()
            .push(start);
    }

    fn render_deactivation(&mut self, participant: &str) {
        if let Some(start) = self
            .activation_starts
            .get_mut(participant)
            .and_then(Vec::pop)
        {
            self.draw_activation_bar(participant, start, self.activation_row());
        }
    }

    /// Ends activations that are never deactivated at the last message.
    fn close_open_activations(&mut self) {
        let end = self.activation_row();
        let mut open: Vec<(String, Vec<f32>)> = self.activation_starts.drain().collect();
        open.sort_by(|a, b| a.0.cmp(&b.0));
        for (participant, starts) in open {
            for start in starts {
                self.draw_activation_bar(&participant, start, end);
            }
        }
    }

    fn draw_activation_bar(&mut self, participant: &str, start: f32, end: f32) {
        if let Some(&cx) = self.centers.get(participant) {
            self.svg.push_str(&format!(
                r#"<rect x="{:.2}" y="{:.2}" width="8" height="{:.2}" fill="{}" fill-opacity="0.35" stroke="{}" stroke-width="1" />"#,
                cx - 4.0,
                start,
                (end - start).max(16.0),
                self.style.node_fill,
                self.style.node_stroke
            ));
        }
    }

    fn render_note(&mut self, participant: &str, position: &str, text: &str) {
        if let Some(&cx) = self.centers.get(participant) {
            let style = self.style;
            let cleaned = sanitize_xml_text(text);
            let note_width =
                (self
                    .measure
                    .measure_width(&cleaned, style.font_size * 0.8, false, false, false)
                    + 20.0)
                    .clamp(80.0, 220.0);
            let x = match position {
                "left" => cx - note_width - 12.0,
                "right" => cx + 12.0,
                _ => cx - note_width / 2.0,
            };
            let y = self.y - 18.0;
            self.svg.push_str(&format!(
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="28" rx="3" fill="{}" fill-opacity="0.25" stroke="{}" stroke-width="1" />"#,
                x,
                y,
                note_width,
                style.node_fill,
                style.node_stroke
            ));
            self.svg.push_str(&format!(
                r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}">{}</text>"#,
                x + 8.0,
                y + 18.0,
                style.font_family,
                style.font_size * 0.8,
                style.node_text,
                escape_xml(&cleaned)
            ));
        }
        self.y += 42.0;
    }

    /// Draws an `alt`/`loop`/... frame around its branches, inset by nesting `depth`.
    fn render_block(&mut self, block: &SequenceBlock, depth: usize) {
        let style = self.style;
        self.y += 12.0;
        let start_y = self.y - 20.0;
        let inset = depth as f32 * 8.0;
        let block_left = self.left_edge - 36.0 + inset;
        let block_right = self.right_edge + 36.0 - inset;
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
        let title_w = self
            .measure
            .measure_width(&cleaned_title, title_font, false, true, false);
        let np_pad_x = 6.0;
        let np_pad_y = 3.0;
        let np_w = title_w + np_pad_x * 2.0;
        let np_h = title_font + np_pad_y * 2.0;
        let np_x = block_left + 10.0 - np_pad_x;
        let np_y = self.y - title_font - np_pad_y + 2.0;
        self.svg.push_str(&format!(
            r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}" stroke="{}" stroke-width="0.75" />"#,
            np_x, np_y, np_w, np_h, style.node_fill, style.node_stroke
        ));
        self.svg.push_str(&format!(
            r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}" font-weight="bold">{}</text>"#,
            block_left + 10.0,
            self.y,
            style.font_family,
            title_font,
            style.node_text,
            escape_xml(&title)
        ));
        self.y += 22.0;

        self.render_elements(&block.messages, depth + 1);

        for (label, branch_elements) in &block.else_branches {
            let separator_y = self.y + 2.0;
            self.svg.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1" stroke-dasharray="5,3" />"#,
                block_left, separator_y, block_right, separator_y, style.edge_stroke
            ));
            if !label.is_empty() {
                self.svg.push_str(&format!(
                    r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}">{}</text>"#,
                    block_left + 10.0,
                    separator_y - 12.0,
                    style.font_family,
                    style.font_size * 0.78,
                    style.node_text,
                    escape_xml(label)
                ));
            }
            self.y = separator_y + 30.0;
            self.render_elements(branch_elements, depth + 1);
        }

        self.svg.push_str(&format!(
            r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="none" stroke="{}" stroke-width="1" stroke-dasharray="5,3" />"#,
            block_left,
            start_y,
            (block_right - block_left).max(24.0),
            (self.y - start_y + 16.0).max(28.0),
            style.edge_stroke
        ));
        self.y += 10.0;
    }
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

    let line_style = if relation.dashed {
        " stroke-dasharray=\"6,3\""
    } else {
        ""
    };

    svg.push_str(&format!(
        r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75"{} />"#,
        x1, y1, x2, y2, style.edge_stroke, line_style
    ));

    svg.push_str(&draw_marker(
        relation.from_marker,
        x1,
        y1,
        angle + std::f32::consts::PI,
        style,
    ));
    svg.push_str(&draw_marker(relation.to_marker, x2, y2, angle, style));

    let ends = [
        (&relation.from_cardinality, (x1, y1), 1.0),
        (&relation.to_cardinality, (x2, y2), -1.0),
    ];
    for (cardinality, end, inward) in ends {
        if let Some(text) = cardinality {
            svg.push_str(&cardinality_text(text, end, angle, inward, style, measure));
        }
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

/// Cardinality text beside the line end at `end`. `angle` is the from-to
/// direction of the line and `inward` is +1 at the from end and -1 at the to
/// end. The text sits on the side opposite the relation label, just clear of
/// both the line and the class box.
fn cardinality_text(
    text: &str,
    end: (f32, f32),
    angle: f32,
    inward: f32,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
) -> String {
    let font_size = style.font_size * 0.8;
    let w = measure.measure_width(&sanitize_xml_text(text), font_size, false, false, false);
    let h = font_size;
    let (cos, sin) = (angle.cos(), angle.sin());
    // Half extents of the text box along the line and across it, plus a gap;
    // the gap across also clears the up to 7px wide end markers.
    let along = cos.abs() * w / 2.0 + sin.abs() * h / 2.0 + 4.0;
    let across = sin.abs() * w / 2.0 + cos.abs() * h / 2.0 + 8.0;
    let x = end.0 + inward * cos * along + sin * across;
    let y = end.1 + inward * sin * along - cos * across;
    format!(
        r#"<text x="{:.2}" y="{:.2}" dy="0.35em" font-family="{}" font-size="{:.1}" fill="{}" text-anchor="middle">{}</text>"#,
        x,
        y,
        style.font_family,
        font_size,
        style.edge_text,
        escape_xml(text)
    )
}

fn draw_marker(marker: ClassMarker, x: f32, y: f32, angle: f32, style: &DiagramStyle) -> String {
    let cos = angle.cos();
    let sin = angle.sin();

    match marker {
        ClassMarker::None => String::new(),
        // A hollow circle touching the class box.
        ClassMarker::Lollipop => format!(
            r#"<circle cx="{:.2}" cy="{:.2}" r="6" fill="{}" stroke="{}" stroke-width="1" />"#,
            x - cos * 6.0,
            y - sin * 6.0,
            style.node_fill,
            style.edge_stroke
        ),
        ClassMarker::Arrow => arrowhead(ArrowHead::Filled, x, y, angle, style),
        ClassMarker::Triangle => {
            let p1 = (x - cos * 14.0 + sin * 7.0, y - sin * 14.0 - cos * 7.0);
            let p2 = (x - cos * 14.0 - sin * 7.0, y - sin * 14.0 + cos * 7.0);
            format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                x, y, p1.0, p1.1, p2.0, p2.1, style.node_fill, style.edge_stroke
            )
        }
        ClassMarker::FilledDiamond => {
            let p1 = (x - cos * 16.0 + sin * 6.0, y - sin * 16.0 - cos * 6.0);
            let p2 = (x - cos * 16.0 - sin * 6.0, y - sin * 16.0 + cos * 6.0);
            let back = (x - cos * 24.0, y - sin * 24.0);
            format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" />"#,
                x, y, p1.0, p1.1, back.0, back.1, p2.0, p2.1, style.edge_stroke
            )
        }
        ClassMarker::HollowDiamond => {
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
    let state_obstacles: Vec<Rect> = positions.values().map(|p| p.expanded(8.0)).collect();
    let (transitions_svg, extents) = render_state_transitions(
        &visible_transitions,
        |id| positions.get(id),
        &state_obstacles,
        style,
        measure,
    );
    svg.push_str(&transitions_svg);
    let transition_min_x = extents.iter().map(|e| e.x).fold(f32::MAX, f32::min);
    let transition_max_x = extents.iter().map(|e| e.x + e.w).fold(f32::MIN, f32::max);
    let transition_max_y = extents.iter().map(|e| e.y + e.h).fold(f32::MIN, f32::max);

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
                let (nx, ny) = state_note_origin(pos);
                total_width = total_width.max(nx + STATE_NOTE_MAX_WIDTH + padding);
                total_height = total_height.max(ny + STATE_NOTE_HEIGHT + padding);
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
    let mut child_states: Vec<&State> =
        state.child_state_ids().map(|id| states_by_id[id]).collect();
    // The composite's own start dot heads the column and its end dot closes it.
    child_states.sort_by_key(|child| (!child.is_start, child.is_end));
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
        // Pseudo-states keep their size so they are still drawn as dots.
        let child_pos = if child_state.is_start || child_state.is_end {
            Rect::new(
                content_left + (content_width - child_w) / 2.0,
                y_cursor,
                child_w,
                child_h,
            )
        } else {
            Rect::new(content_left, y_cursor, content_width.max(child_w), child_h)
        };
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

    let mut child_obstacles: Vec<Rect> =
        child_positions.values().map(|p| p.expanded(3.0)).collect();
    // Include global positions but exclude the parent composite (we're routing inside it)
    child_obstacles.extend(
        positions
            .values()
            .filter(|p| !p.overlaps(parent_pos) || p.w < parent_pos.w * 0.5)
            .map(|p| p.expanded(3.0)),
    );

    // Transitions to states outside the composite resolve to their global positions.
    let (transitions_svg, _) = render_state_transitions(
        &child_transitions,
        |id| child_positions.get(id).or_else(|| positions.get(id)),
        &child_obstacles,
        style,
        measure,
    );
    svg.push_str(&transitions_svg);
    svg
}

/// Draws `transitions`, resolving state ids through `position_of` and
/// spreading transitions between the same pair of states into separate
/// lanes. Returns the SVG and the extent of each drawn transition.
fn render_state_transitions<'r>(
    transitions: &[&StateTransition],
    position_of: impl Fn(&str) -> Option<&'r Rect>,
    obstacles: &[Rect],
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
) -> (String, Vec<Rect>) {
    let mut pair_totals: HashMap<(String, String), usize> = HashMap::new();
    for transition in transitions {
        *pair_totals
            .entry(state_pair_key(&transition.from, &transition.to))
            .or_insert(0) += 1;
    }

    // Every transition is routed before any label is placed, so labels can
    // keep off all the lines.
    let mut pair_seen: HashMap<(String, String), usize> = HashMap::new();
    let routed: Vec<Option<(StateEdge, StateRoute)>> = transitions
        .iter()
        .map(|transition| {
            let key = state_pair_key(&transition.from, &transition.to);
            let seen = pair_seen.entry(key.clone()).or_insert(0);
            let route_index = *seen;
            *seen += 1;
            let from = position_of(&transition.from)?;
            let to = position_of(&transition.to)?;
            if transition.from == transition.to {
                return None;
            }
            let edge = StateEdge::new(
                from,
                to,
                RouteSlot::new(transition, route_index, pair_totals[&key]),
            );
            let route = edge.route(&Blockers::new(obstacles, edge.from_center, edge.to_center));
            Some((edge, route))
        })
        .collect();

    let mut svg = String::new();
    let mut extents = Vec::new();
    let mut occupied_labels: Vec<Rect> = Vec::new();
    for (index, (transition, routed_edge)) in transitions.iter().zip(&routed).enumerate() {
        let Some(routed_edge) = routed_edge else {
            if transition.from == transition.to
                && let Some(pos) = position_of(&transition.from)
            {
                let (t_svg, extent) = render_state_self_transition(
                    transition,
                    pos,
                    style,
                    measure,
                    &mut occupied_labels,
                );
                svg.push_str(&t_svg);
                extents.push(extent);
            }
            continue;
        };
        let route = &routed_edge.1;
        svg.push_str(&emit_route(route, style));
        let mut extent_points = route.points.clone();
        if let Some(label) = &transition.label {
            let other_routes: Vec<&StateRoute> = routed
                .iter()
                .enumerate()
                .filter(|&(other, _)| other != index)
                .filter_map(|(_, routed)| routed.as_ref().map(|(_, route)| route))
                .collect();
            let (pill, rect) = place_state_label(
                label,
                routed_edge,
                &other_routes,
                obstacles,
                &mut occupied_labels,
                style,
                measure,
            );
            svg.push_str(&pill);
            extent_points.extend([(rect.x, rect.y), (rect.right(), rect.bottom())]);
        }
        extents.push(transition_extent(&extent_points));
    }
    (svg, extents)
}

/// Bounds of `points`, stretched on the low side to include the origin.
fn transition_extent(points: &[(f32, f32)]) -> Rect {
    let (min_x, min_y, max_x, max_y) = point_bounds(points);
    let (x, y) = (min_x.min(0.0), min_y.min(0.0));
    Rect {
        x,
        y,
        w: (max_x - x).max(0.0),
        h: (max_y - y).max(0.0),
    }
}

const STATE_NOTE_HEIGHT: f32 = 26.0;
const STATE_NOTE_MAX_WIDTH: f32 = 180.0;

/// Top-left corner of the note drawn beside the state at `state_pos`.
fn state_note_origin(state_pos: &Rect) -> (f32, f32) {
    (state_pos.x + state_pos.w + 28.0, state_pos.y + 4.0)
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
        .clamp(72.0, STATE_NOTE_MAX_WIDTH);
    let note_height = STATE_NOTE_HEIGHT;
    let (x, y) = state_note_origin(state_pos);

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

/// Per-transition routing parameters derived from the state names and the
/// transition's position among parallel transitions between the same pair.
struct RouteSlot {
    hash: u32,
    /// Offset among parallel transitions, centered on 0.
    lane: f32,
    lane_offset: f32,
    /// Largest `|lane|` among the parallel transitions.
    max_lane: f32,
    /// Name-derived spread in -2..=2 that keeps unrelated labels apart.
    global_lane: f32,
    /// Preferred side for lanes and labels.
    side: f32,
}

impl RouteSlot {
    fn new(transition: &StateTransition, index: usize, total: usize) -> Self {
        let hash = transition
            .from
            .bytes()
            .chain(transition.to.bytes())
            .fold(0_u32, |acc, value| {
                acc.wrapping_mul(31).wrapping_add(value as u32)
            });
        let lane = if total > 1 {
            index as f32 - (total as f32 - 1.0) / 2.0
        } else {
            0.0
        };
        Self {
            hash,
            lane,
            lane_offset: lane * 30.0,
            max_lane: (total as f32 - 1.0) / 2.0,
            global_lane: (hash % 5) as f32 - 2.0,
            // Self transitions are drawn as loops, so the names always differ here.
            side: if transition.from < transition.to {
                1.0
            } else {
                -1.0
            },
        }
    }
}

/// Small square states are drawn as start/end circles.
fn is_round_state(rect: &Rect) -> bool {
    rect.w == rect.h && rect.w < 30.0
}

/// Where a line at `angle` from the center leaves `rect`.
fn state_anchor(rect: &Rect, angle: f32) -> (f32, f32) {
    if is_round_state(rect) {
        let (cx, cy) = rect.center();
        (
            cx + angle.cos() * (rect.w / 2.0),
            cy + angle.sin() * (rect.w / 2.0),
        )
    } else {
        rect_boundary_point(rect, angle)
    }
}

/// Half the width of `rect` measured across a line at `angle`.
fn cross_half_extent(rect: &Rect, angle: f32) -> f32 {
    if is_round_state(rect) {
        rect.w / 2.0
    } else {
        rect.w / 2.0 * angle.sin().abs() + rect.h / 2.0 * angle.cos().abs()
    }
}

/// Where a ray at `angle`, starting `offset` to the left of the center of
/// `rect`, leaves `rect`. `offset` must stay below `cross_half_extent`.
fn shifted_anchor(rect: &Rect, angle: f32, offset: f32) -> (f32, f32) {
    let (cx, cy) = rect.center();
    let (dx, dy) = (angle.cos(), angle.sin());
    let (ox, oy) = (cx - dy * offset, cy + dx * offset);
    let t = if is_round_state(rect) {
        let r = rect.w / 2.0;
        (r * r - offset * offset).sqrt()
    } else {
        // Distance along the ray to the side it leaves through on one axis.
        let exit = |d: f32, o: f32, lo: f32, hi: f32| {
            if d > 1e-5 {
                (hi - o) / d
            } else if d < -1e-5 {
                (lo - o) / d
            } else {
                f32::INFINITY
            }
        };
        exit(dx, ox, rect.x, rect.right()).min(exit(dy, oy, rect.y, rect.bottom()))
    };
    (ox + dx * t, oy + dy * t)
}

/// Where a vertical line at `x` leaves `rect` through its bottom or top.
fn state_vertical_anchor(rect: &Rect, x: f32, bottom: bool) -> f32 {
    let sign = if bottom { 1.0 } else { -1.0 };
    if is_round_state(rect) {
        let (cx, cy) = rect.center();
        let r = rect.w / 2.0;
        cy + sign * (r * r - (x - cx).powi(2)).sqrt()
    } else if bottom {
        rect.bottom()
    } else {
        rect.y
    }
}

/// Obstacles a transition must avoid: every state except its own endpoints,
/// which are recognized by their centers.
struct Blockers<'a> {
    rects: Vec<&'a Rect>,
}

impl<'a> Blockers<'a> {
    fn new(obstacles: &'a [Rect], from_center: (f32, f32), to_center: (f32, f32)) -> Self {
        let centered_on = |r: &Rect, (cx, cy): (f32, f32)| {
            let (rcx, rcy) = r.center();
            (rcx - cx).abs() < 1.0 && (rcy - cy).abs() < 1.0
        };
        let rects = obstacles
            .iter()
            .filter(|r| !centered_on(r, from_center) && !centered_on(r, to_center))
            .collect();
        Self { rects }
    }

    fn crosses(&self, a: (f32, f32), b: (f32, f32)) -> bool {
        self.rects
            .iter()
            .any(|r| line_intersects_rect(a.0, a.1, b.0, b.1, r))
    }

    /// Whether the axis-aligned polyline through `points` keeps a 6px margin
    /// from every blocker.
    fn orthogonal_clear(&self, points: &[(f32, f32)]) -> bool {
        self.rects.iter().all(|r| {
            let r = r.expanded(6.0);
            points.windows(2).all(|segment| {
                let ((x1, y1), (x2, y2)) = (segment[0], segment[1]);
                if x1 == x2 {
                    !vseg_hits_rect(x1, y1, y2, &r)
                } else {
                    !hseg_hits_rect(y1, x1, x2, &r)
                }
            })
        })
    }
}

/// Steps outward from `base(side)` for each of `sides` until `clear(side, lane)`
/// holds. The second side wins only with strictly fewer steps. Returns the
/// lane and its step count.
fn first_clear_lane(
    sides: [f32; 2],
    base: impl Fn(f32) -> f32,
    clear: impl Fn(f32, f32) -> bool,
) -> Option<(f32, usize)> {
    const STEP: f32 = 18.0;
    const MAX_STEPS: usize = 30;
    let mut best = None;
    let mut limit = MAX_STEPS;
    for side in sides {
        let base = base(side);
        let found = (0..limit)
            .map(|i| (base + side * (i as f32) * STEP, i))
            .find(|&(lane, _)| clear(side, lane));
        if let Some((_, steps)) = found {
            best = found;
            limit = steps;
        }
    }
    best
}

/// A routed transition. The arrowhead sits on the last point and follows
/// the last drawn segment.
struct StateRoute {
    points: Vec<(f32, f32)>,
    label_anchor: (f32, f32),
}

/// Endpoint geometry shared by every routing strategy for one transition.
struct StateEdge<'a> {
    from: &'a Rect,
    to: &'a Rect,
    from_center: (f32, f32),
    to_center: (f32, f32),
    /// Where the center-to-center line leaves `from` and enters `to`.
    start: (f32, f32),
    end: (f32, f32),
    slot: RouteSlot,
    /// The states share a column and sit far enough apart vertically that
    /// the transition runs vertically, directly or through a side lane.
    verticalish: bool,
}

impl<'a> StateEdge<'a> {
    fn new(from: &'a Rect, to: &'a Rect, slot: RouteSlot) -> Self {
        let from_center = from.center();
        let to_center = to.center();
        let angle = (to_center.1 - from_center.1).atan2(to_center.0 - from_center.0);
        let start = state_anchor(from, angle);
        let end = state_anchor(to, angle + std::f32::consts::PI);
        let verticalish = (from_center.0 - to_center.0).abs() < (from.w.min(to.w)) / 2.0
            && (end.1 - start.1).abs() > 30.0;
        Self {
            from,
            to,
            from_center,
            to_center,
            start,
            end,
            slot,
            verticalish,
        }
    }

    fn route(&self, blockers: &Blockers) -> StateRoute {
        if self.verticalish {
            if let Some(route) = self.vertical_route(blockers) {
                return route;
            }
            let max_half_width = (self.from.w / 2.0).max(self.to.w / 2.0);
            let lane_x = self.side_lane(blockers).map_or(
                self.from_center.0 + self.slot.side * (max_half_width + 30.0),
                |(lane_x, _)| lane_x,
            );
            self.side_lane_route(lane_x)
        } else if let Some(segment) = [1.0, 0.5]
            .map(|spacing| self.parallel_segment(spacing))
            .into_iter()
            .find(|&(start, end)| !blockers.crosses(start, end))
        {
            // Jitter the label anchor along the edge to reduce label-label collisions.
            let jitter = ((self.slot.hash % 7) as f32 - 3.0) * 0.07;
            self.straight_route(segment, (0.5 + jitter).clamp(0.25, 0.75))
        } else {
            self.detour_route(blockers)
        }
    }

    /// Tries a Z route, then the closer of a U route and a side lane, and
    /// finally draws the blocked straight line anyway.
    fn detour_route(&self, blockers: &Blockers) -> StateRoute {
        if let Some(route) = self.z_route(blockers) {
            return route;
        }
        match (self.u_lane(blockers), self.side_lane(blockers)) {
            (Some((lane_y, u_steps)), side)
                if side.is_none_or(|(_, side_steps)| u_steps <= side_steps) =>
            {
                self.u_route(lane_y)
            }
            (_, Some((lane_x, _))) => self.side_lane_route(lane_x),
            _ => self.straight_route(self.parallel_segment(1.0), 0.5),
        }
    }

    /// Joins vertically aligned states with a vertical line through the
    /// columns they share, shifted per parallel transition, if it is clear.
    fn vertical_route(&self, blockers: &Blockers) -> Option<StateRoute> {
        let x = (self.from_center.0 + self.to_center.0) / 2.0 + self.slot.lane_offset;
        let spans = |r: &Rect| (x - r.center().0).abs() < r.w / 2.0;
        if !spans(self.from) || !spans(self.to) {
            return None;
        }
        let down = self.to_center.1 > self.from_center.1;
        let (y1, y2) = (
            state_vertical_anchor(self.from, x, down),
            state_vertical_anchor(self.to, x, !down),
        );
        let points = vec![(x, y1), (x, y2)];
        if !blockers.orthogonal_clear(&points) {
            return None;
        }
        Some(StateRoute {
            points,
            label_anchor: (x, (y1 + y2) / 2.0),
        })
    }

    /// Draws `segment` with the label anchored at fraction `t` along it.
    fn straight_route(&self, segment: ((f32, f32), (f32, f32)), t: f32) -> StateRoute {
        let ((x1, y1), (x2, y2)) = segment;
        StateRoute {
            points: vec![(x1, y1), (x2, y2)],
            label_anchor: (x1 + (x2 - x1) * t, y1 + (y2 - y1) * t),
        }
    }

    /// The center-to-center line shifted sideways by this transition's lane,
    /// with `spacing` scaling the gap between lanes. Lanes are counted in the
    /// pair's canonical direction (`state_pair_key`), so `A --> B` and
    /// `B --> A` land on opposite sides.
    fn parallel_segment(&self, spacing: f32) -> ((f32, f32), (f32, f32)) {
        if self.slot.lane == 0.0 {
            return (self.start, self.end);
        }
        let (fx, fy) = self.from_center;
        let (tx, ty) = self.to_center;
        let angle = (ty - fy).atan2(tx - fx);
        // Keep the outermost lanes well inside both states' cross-sections.
        let limit =
            0.6 * cross_half_extent(self.from, angle).min(cross_half_extent(self.to, angle));
        let lane_gap = (limit / self.slot.max_lane).min(30.0) * spacing;
        let offset = self.slot.lane * lane_gap * self.slot.side;
        (
            shifted_anchor(self.from, angle, offset),
            shifted_anchor(self.to, angle + std::f32::consts::PI, -offset),
        )
    }

    /// Joins the facing sides with an orthogonal Z through the middle of the
    /// gap between the states: top and bottom ports when they are stacked,
    /// side midpoints when they sit side by side. Tries the axis with the
    /// wider gap first.
    fn z_route(&self, blockers: &Blockers) -> Option<StateRoute> {
        let (from, to) = (self.from, self.to);
        let (from_cx, from_cy) = self.from_center;
        let (to_cx, to_cy) = self.to_center;
        let vertical = {
            let down = to_cy > from_cy;
            let from_y = if down { from.bottom() } else { from.y };
            let to_y = if down { to.y } else { to.bottom() };
            let mid_y = (from_y + to_y) / 2.0;
            // Ports a quarter width toward each other leave the centers to
            // vertical transitions.
            let toward = (to_cx - from_cx).signum();
            let (from_x, to_x) = if is_round_state(from) || is_round_state(to) {
                (from_cx, to_cx)
            } else {
                (from_cx + toward * from.w / 4.0, to_cx - toward * to.w / 4.0)
            };
            vec![
                (from_x, from_y),
                (from_x, mid_y),
                (to_x, mid_y),
                (to_x, to_y),
            ]
        };
        let horizontal = {
            let right = to_cx > from_cx;
            let from_x = if right { from.right() } else { from.x };
            let to_x = if right { to.x } else { to.right() };
            let mid_x = (from_x + to_x) / 2.0;
            vec![
                (from_x, from_cy),
                (mid_x, from_cy),
                (mid_x, to_cy),
                (to_x, to_cy),
            ]
        };
        // The middle leg crosses this gap, so the states need room between them.
        let v_gap = (to.y - from.bottom()).max(from.y - to.bottom());
        let h_gap = (to.x - from.right()).max(from.x - to.right());
        let mut candidates = [(v_gap, vertical), (h_gap, horizontal)];
        if h_gap > v_gap {
            candidates.reverse();
        }
        let points = candidates
            .into_iter()
            .filter(|(gap, _)| *gap >= 16.0)
            .map(|(_, points)| points)
            .find(|points| blockers.orthogonal_clear(points))?;
        let label_anchor = (
            (points[1].0 + points[2].0) / 2.0,
            (points[1].1 + points[2].1) / 2.0,
        );
        Some(StateRoute {
            points,
            label_anchor,
        })
    }

    /// First clear horizontal lane below or above both states, preferring the
    /// direction of travel.
    fn u_lane(&self, blockers: &Blockers) -> Option<(f32, usize)> {
        let (from, to) = (self.from, self.to);
        let (from_cx, to_cx) = (self.from_center.0, self.to_center.0);
        let lane_pad = self.slot.lane.abs() * 14.0;
        let base = |side: f32| {
            if side > 0.0 {
                from.bottom().max(to.bottom()) + 30.0 + lane_pad
            } else {
                from.y.min(to.y) - 30.0 - lane_pad
            }
        };
        let preferred: f32 = if self.to_center.1 > self.from_center.1 {
            1.0
        } else {
            -1.0
        };
        first_clear_lane([preferred, -preferred], base, |side, lane_y| {
            let (from_y, to_y) = if side > 0.0 {
                (from.bottom(), to.bottom())
            } else {
                (from.y, to.y)
            };
            blockers.orthogonal_clear(&[
                (from_cx, from_y),
                (from_cx, lane_y),
                (to_cx, lane_y),
                (to_cx, to_y),
            ])
        })
    }

    /// Leaves both states vertically and joins them along `lane_y`.
    fn u_route(&self, lane_y: f32) -> StateRoute {
        let (from_cx, from_cy) = self.from_center;
        let (to_cx, to_cy) = self.to_center;
        let from_exit_y = if lane_y > from_cy {
            self.from.bottom()
        } else {
            self.from.y
        };
        let to_enter_y = if lane_y > to_cy {
            self.to.bottom()
        } else {
            self.to.y
        };
        StateRoute {
            points: vec![
                (from_cx, from_exit_y),
                (from_cx, lane_y),
                (to_cx, lane_y),
                (to_cx, to_enter_y),
            ],
            label_anchor: ((from_cx + to_cx) / 2.0, lane_y),
        }
    }

    /// First clear vertical lane beside both states, preferring the slot's side.
    fn side_lane(&self, blockers: &Blockers) -> Option<(f32, usize)> {
        let (from, to) = (self.from, self.to);
        let (from_cx, exit_y) = self.from_center;
        let (to_cx, enter_y) = self.to_center;
        let max_half_width = (from.w / 2.0).max(to.w / 2.0);
        let base =
            |side: f32| from_cx + side * (max_half_width + 30.0 + self.slot.lane.abs() * 14.0);
        first_clear_lane([self.slot.side, -self.slot.side], base, |side, lane_x| {
            let base_x = base(side);
            let from_x = if base_x >= from_cx {
                from.right()
            } else {
                from.x
            };
            let to_x = if base_x >= to_cx { to.right() } else { to.x };
            blockers.orthogonal_clear(&[
                (from_x, exit_y),
                (lane_x, exit_y),
                (lane_x, enter_y),
                (to_x, enter_y),
            ])
        })
    }

    /// Leaves both states sideways and joins them along `lane_x`.
    fn side_lane_route(&self, lane_x: f32) -> StateRoute {
        let (from_cx, exit_y) = self.from_center;
        let (to_cx, enter_y) = self.to_center;
        let from_exit_x = if lane_x >= from_cx {
            self.from.right()
        } else {
            self.from.x
        };
        let to_enter_x = if lane_x >= to_cx {
            self.to.right()
        } else {
            self.to.x
        };
        StateRoute {
            points: vec![
                (from_exit_x, exit_y),
                (lane_x, exit_y),
                (lane_x, enter_y),
                (to_enter_x, enter_y),
            ],
            label_anchor: (lane_x, (exit_y + enter_y) / 2.0),
        }
    }
}

/// SVG for `route`: a line or polyline plus the arrowhead on its last point.
fn emit_route(route: &StateRoute, style: &DiagramStyle) -> String {
    let mut svg = if let [(x1, y1), (x2, y2)] = route.points[..] {
        format!(
            r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="0.75" />"#,
            x1, y1, x2, y2, style.edge_stroke
        )
    } else {
        let points: Vec<String> = route
            .points
            .iter()
            .map(|(x, y)| format!("{x:.2},{y:.2}"))
            .collect();
        format!(
            r#"<polyline points="{}" fill="none" stroke="{}" stroke-width="0.75" />"#,
            points.join(" "),
            style.edge_stroke
        )
    };
    // A Z route between states at equal height ends in zero-length
    // segments, which have no direction.
    let heading = route
        .points
        .windows(2)
        .rev()
        .find(|segment| segment[0] != segment[1])
        .map_or(0.0, |segment| {
            let ((x1, y1), (x2, y2)) = (segment[0], segment[1]);
            (y2 - y1).atan2(x2 - x1)
        });
    let (x, y) = route.points[route.points.len() - 1];
    svg.push_str(&arrowhead(ArrowHead::Filled, x, y, heading, style));
    svg
}

/// Point and unit tangent at fraction `t` of the length of `points`.
fn polyline_at(points: &[(f32, f32)], t: f32) -> ((f32, f32), (f32, f32)) {
    let length = |((x1, y1), (x2, y2)): ((f32, f32), (f32, f32))| (x2 - x1).hypot(y2 - y1);
    let segments = || points.windows(2).map(|w| (w[0], w[1]));
    let mut remaining = segments().map(length).sum::<f32>() * t;
    let mut at = (points[0], (1.0, 0.0));
    // Zero-length segments, as in a Z between states at equal height, have
    // no direction.
    for segment @ ((x1, y1), end) in segments().filter(|&s| length(s) > 1e-3) {
        let len = length(segment);
        let tangent = ((end.0 - x1) / len, (end.1 - y1) / len);
        if remaining <= len {
            return (
                (x1 + tangent.0 * remaining, y1 + tangent.1 * remaining),
                tangent,
            );
        }
        remaining -= len;
        at = (end, tangent);
    }
    at
}

/// Places `label` beside the drawn `route`, trying spots along its middle
/// and scoring overlap with states, earlier labels and `other_routes` plus
/// distance moved from the route's label anchor.
fn place_state_label(
    label: &str,
    (edge, route): &(StateEdge, StateRoute),
    other_routes: &[&StateRoute],
    obstacles: &[Rect],
    occupied_labels: &mut Vec<Rect>,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
) -> (String, Rect) {
    let label_font = style.font_size * 0.85;
    let (label_width, label_height) = pill_size(measure, label, label_font);
    let slot = &edge.slot;
    let tangent_offset = slot.global_lane * 6.0;
    let (anchor_x, anchor_y) = route.label_anchor;
    // Among straight parallel transitions, a label beside an inner lane
    // would cover its neighbour, so it sits on its own line; the outermost
    // lanes put theirs on the outer side. Side 0 means on the line.
    let sides = match route.points[..] {
        [_, _] if slot.lane.abs() < slot.max_lane => vec![0.0],
        [(x1, y1), (x2, y2)] if slot.lane != 0.0 => {
            let ((fx, fy), (tx, ty)) = (edge.from_center, edge.to_center);
            let (mx, my) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
            vec![((tx - fx) * (my - fy) - (ty - fy) * (mx - fx)).signum()]
        }
        _ => vec![slot.side, -slot.side],
    };

    let score = |lx: f32, ly: f32| -> f32 {
        let r = Rect::new(
            lx - label_width / 2.0,
            ly - label_height / 2.0,
            label_width,
            label_height,
        );
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
        for other in other_routes {
            let crosses = other.points.windows(2).any(|segment| {
                let ((x1, y1), (x2, y2)) = (segment[0], segment[1]);
                line_intersects_rect(x1, y1, x2, y2, &r)
            });
            if crosses {
                s += 60.0;
            }
        }
        s
    };
    let movement_weight = 2.0;
    let cost_and_move = |lx: f32, ly: f32| {
        let mv = ((lx - anchor_x).powi(2) + (ly - anchor_y).powi(2)).sqrt();
        (score(lx, ly) + mv * movement_weight, mv)
    };

    // Center of a pill whose nearest edge is `gap` from the route, on the
    // left of its direction for `side` 1 and the right for -1.
    let beside = |t: f32, side: f32, gap: f32| {
        let ((ax, ay), (tx, ty)) = polyline_at(&route.points, t);
        let reach = gap + label_width / 2.0 * ty.abs() + label_height / 2.0 * tx.abs();
        (
            ax - ty * reach * side + tx * tangent_offset,
            ay + tx * reach * side + ty * tangent_offset,
        )
    };
    let mut best = None;
    for t in [0.38, 0.46, 0.5, 0.54, 0.62] {
        for &side in &sides {
            for gap in [8.0, 14.0, 24.0, 34.0, 44.0, 56.0, 68.0] {
                let (lx, ly) = beside(t, side, gap);
                let (cost, mv) = cost_and_move(lx, ly);
                if best.is_none_or(|(best_cost, best_move, _): (f32, f32, _)| {
                    cost < best_cost || ((cost - best_cost).abs() < f32::EPSILON && mv < best_move)
                }) {
                    best = Some((cost, mv, (lx, ly)));
                }
            }
        }
    }
    let (_, _, center) = best.expect("label candidates are never empty");

    let (pill, rect) = label_pill(measure, style, label, label_font, center);
    occupied_labels.push(rect);
    (pill, rect)
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
    let (min_x, min_y, max_x, max_y) = point_bounds(points);
    Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
}

/// `(min_x, min_y, max_x, max_y)` over `points`.
fn point_bounds(points: &[(f32, f32)]) -> (f32, f32, f32, f32) {
    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
    let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
    for &(x, y) in points {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    (min_x, min_y, max_x, max_y)
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
    fn sequence_activation_bar_spans_activating_to_deactivating_message() {
        let image = rasterize(&render_svg(
            "sequenceDiagram\n    participant A\n    participant B\n    A->>+B: Hello\n    B-->>-A: Hi",
        ));
        let (_, box_top, _, box_bottom) = node_bands(&image)[0];
        // Above the participant names.
        let box_row = box_top + 2;
        let box_xs: Vec<u32> = (0..image.width())
            .filter(|&x| is_node_fill(image.get_pixel(x, box_row)))
            .collect();
        let gap = box_xs.windows(2).position(|w| w[1] > w[0] + 1).unwrap();
        let a_center = (box_xs[0] + box_xs[gap]) / 2;
        let b_center = (box_xs[gap + 1] + box_xs[box_xs.len() - 1]) / 2;

        // Message lines cross the left quarter between the lifelines, clear
        // of the centered label pills; the dashed reply needs a range of x.
        let quarter = a_center + (b_center - a_center) / 4;
        let message_rows: Vec<u32> = (box_bottom + 2..image.height())
            .filter(|&y| {
                (quarter..quarter + 10).any(|x| {
                    let p = image.get_pixel(x, y);
                    p[3] > 32 && p[2] > p[0] && p[2] > p[1]
                })
            })
            .collect();
        let split = message_rows.windows(2).position(|w| w[1] > w[0] + 2);
        let hello_row = message_rows[0];
        let hi_row = message_rows[split.expect("two message rows") + 1];

        // Skip the participant box outline.
        let bar_rows: Vec<u32> = (box_bottom + 3..image.height())
            .filter(|&y| {
                let p = image.get_pixel(b_center, y);
                p[3] > 40 && (p[0] > 60 || p[1] > 60)
            })
            .collect();
        let (bar_top, bar_bottom) = (bar_rows[0], bar_rows[bar_rows.len() - 1]);
        assert!(
            bar_top.abs_diff(hello_row) <= 2 && bar_bottom.abs_diff(hi_row) <= 2,
            "bar spans {bar_top}..={bar_bottom}, messages at {hello_row} and {hi_row}"
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
    fn class_markers_sit_at_the_end_mermaid_puts_them() {
        // (relation, whether the marker belongs at the lower class B)
        let cases = [
            ("A <|-- B", false),
            ("A *-- B", false),
            ("A o-- B", false),
            ("A --> B", true),
            ("A ..> B", true),
            ("A ..|> B", true),
            ("A <-- B", false),
            ("A <.. B", false),
            ("A <|.. B", false),
            ("A --|> B", true),
            ("A --* B", true),
            ("A --o B", true),
            ("A ()-- B", false),
            ("A --() B", true),
        ];
        for (relation, at_b) in cases {
            let (image, a_bottom, b_top) = render_two_classes(relation);
            // Markers are the only solid or filled shapes between the classes.
            let marker_rows: Vec<u32> = image
                .enumerate_pixels()
                .filter(|(_, y, p)| *y > a_bottom && *y < b_top && (is_edge(p) || is_node_fill(p)))
                .map(|(_, y, _)| y)
                .collect();
            assert!(marker_rows.len() > 20, "{relation}: marker not visible");
            let mean = marker_rows.iter().sum::<u32>() / marker_rows.len() as u32;
            let near_b = mean > (a_bottom + b_top) / 2;
            assert_eq!(
                near_b, at_b,
                "{relation}: marker centered at row {mean}, A ends at {a_bottom}, B starts at {b_top}"
            );
        }
    }

    /// Renders classes A and B joined by `relation` and returns the image with
    /// the bottom row of A and the top row of B, which must sit below A.
    fn render_two_classes(relation: &str) -> (RgbaImage, u32, u32) {
        let image = rasterize(&render_svg(&format!(
            "classDiagram\n  class A\n  class B\n  {relation}\n"
        )));
        // Class boxes are wide bands split by compartment separators;
        // narrow bands are hollow markers.
        let mut boxes: Vec<(u32, u32)> = Vec::new();
        for (left, top, right, bottom) in node_bands(&image) {
            match boxes.last_mut() {
                _ if right - left < 60 => {}
                Some(last) if top <= last.1 + 4 => last.1 = bottom,
                _ => boxes.push((top, bottom)),
            }
        }
        assert_eq!(
            boxes.len(),
            2,
            "{relation}: expected A above B, got {boxes:?}"
        );
        (image, boxes[0].1, boxes[1].0)
    }

    #[test]
    fn plain_class_links_draw_no_markers() {
        for relation in ["A -- B", "A .. B"] {
            let (image, a_bottom, b_top) = render_two_classes(relation);
            let widest_row = (a_bottom + 1..b_top)
                .map(|y| {
                    (0..image.width())
                        .filter(|&x| is_edge(image.get_pixel(x, y)))
                        .count()
                })
                .max()
                .unwrap();
            assert!(
                widest_row <= 3,
                "{relation}: only a thin line should join the classes, got a {widest_row}px row"
            );
        }
    }

    #[test]
    fn class_cardinalities_render_as_their_own_text() {
        let text = visible_text(&render_svg(
            "classDiagram\n  Customer \"1\" --> \"many\" Ticket : buys",
        ));
        for expected in ["Customer", "Ticket", "1", "many", "buys"] {
            assert!(
                text.iter().any(|t| t == expected),
                "missing text {expected:?}, got {text:?}"
            );
        }
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

    /// Pixel bounds of each horizontal band of node fill, top to bottom.
    fn node_bands(image: &RgbaImage) -> Vec<(u32, u32, u32, u32)> {
        let mut bands: Vec<(u32, u32, u32, u32)> = Vec::new();
        for y in 0..image.height() {
            let xs: Vec<u32> = (0..image.width())
                .filter(|&x| is_node_fill(image.get_pixel(x, y)))
                .collect();
            let (Some(&left), Some(&right)) = (xs.first(), xs.last()) else {
                continue;
            };
            match bands.last_mut() {
                Some(band) if band.3 + 1 == y => {
                    *band = (band.0.min(left), band.1, band.2.max(right), y);
                }
                _ => bands.push((left, y, right, y)),
            }
        }
        bands
    }

    /// Pixels inside solid areas of the node stroke color, i.e. the start
    /// dot; state outlines are at most a pixel thick.
    fn start_dot_pixels(image: &RgbaImage) -> Vec<(u32, u32)> {
        let stroke = |x: u32, y: u32| {
            let p = image.get_pixel(x, y);
            p[3] > 200 && p[1] > 180 && p[0] < 60 && p[2] < 60
        };
        (0..image.width() - 1)
            .flat_map(|x| (0..image.height() - 1).map(move |y| (x, y)))
            .filter(|&(x, y)| stroke(x, y) && stroke(x + 1, y) && stroke(x, y + 1))
            .collect()
    }

    #[test]
    fn state_arrowhead_is_drawn_outside_target_state() {
        let image = rasterize(&render_svg("stateDiagram-v2\nA --> B"));
        let bands = node_bands(&image);
        assert_eq!(bands.len(), 2, "expected A above B, got {bands:?}");
        let (left, top, right, bottom) = bands[1];
        let near_outside = image
            .enumerate_pixels()
            .filter(|(x, y, p)| {
                let inside = (left..=right).contains(x) && (top..=bottom).contains(y);
                let near =
                    *x + 10 >= left && *x <= right + 10 && *y + 10 >= top && *y <= bottom + 10;
                is_edge(p) && near && !inside
            })
            .count();
        assert!(
            near_outside >= 20,
            "arrowhead must be visible just outside B, got {near_outside} pixels"
        );
    }

    #[test]
    fn vertically_aligned_states_are_joined_by_straight_line() {
        let image = rasterize(&render_svg("stateDiagram-v2\nA --> B"));
        let bands = node_bands(&image);
        assert_eq!(bands.len(), 2, "expected A above B, got {bands:?}");
        let ((left, _, right, a_bottom), (_, b_top, _, _)) = (bands[0], bands[1]);
        let center = (left + right) / 2;
        let ink = |x: u32, y: u32| image.get_pixel(x, y)[3] > 32;
        let gap_rows = a_bottom + 3..b_top - 3;
        let unjoined_rows: Vec<u32> = gap_rows
            .clone()
            .filter(|&y| !(center - 2..=center + 2).any(|x| ink(x, y)))
            .collect();
        assert!(
            unjoined_rows.is_empty(),
            "no line at x={center} on rows {unjoined_rows:?}"
        );
        let side_ink = gap_rows
            .flat_map(|y| (0..image.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| (x + 3 < left || x > right + 3) && ink(x, y))
            .count();
        assert_eq!(side_ink, 0, "edge detours beside the states");
    }

    /// Routes a transition from `from` to `to` around `blocker`, which sits
    /// on the straight line between them.
    fn route_around(from: Rect, to: Rect, blocker: Rect) -> Vec<(f32, f32)> {
        let transition = StateTransition {
            from: "A".into(),
            to: "B".into(),
            label: None,
        };
        let edge = StateEdge::new(&from, &to, RouteSlot::new(&transition, 0, 1));
        assert!(!edge.verticalish);
        let obstacles = [from, to, blocker];
        let blockers = Blockers::new(&obstacles, edge.from_center, edge.to_center);
        assert!(
            blockers.crosses(edge.start, edge.end),
            "blocker misses the chord"
        );
        edge.route(&blockers).points
    }

    #[test]
    fn stacked_states_join_through_facing_borders() {
        // B sits up and to the right of A; the diagonal is blocked.
        let (a, b) = (
            Rect::new(0.0, 300.0, 120.0, 40.0),
            Rect::new(300.0, 0.0, 120.0, 40.0),
        );
        let points = route_around(a, b, Rect::new(120.0, 230.0, 30.0, 30.0));
        let [(x1, y1), (x2, y2), (x3, y3), (x4, y4)] = points[..] else {
            panic!("expected a Z route, got {points:?}");
        };
        assert!(
            x1 == x2 && x3 == x4 && y2 == y3,
            "not a vertical Z: {points:?}"
        );
        assert!(
            y1 == a.y && (a.x..a.right()).contains(&x1),
            "leaves A off its top: {points:?}"
        );
        assert!(
            y4 == b.bottom() && (b.x..b.right()).contains(&x4),
            "enters B off its bottom: {points:?}"
        );
        assert!(
            y2 < a.y && y2 > b.bottom(),
            "middle leg outside the gap: {points:?}"
        );
    }

    #[test]
    fn side_by_side_states_join_through_facing_sides() {
        // B sits to the right of A and a little lower; the diagonal is blocked.
        let (a, b) = (
            Rect::new(0.0, 0.0, 120.0, 40.0),
            Rect::new(300.0, 100.0, 120.0, 40.0),
        );
        let points = route_around(a, b, Rect::new(140.0, 40.0, 20.0, 20.0));
        let [(x1, y1), (x2, y2), (x3, y3), (x4, y4)] = points[..] else {
            panic!("expected a Z route, got {points:?}");
        };
        assert!(
            y1 == y2 && y3 == y4 && x2 == x3,
            "not a horizontal Z: {points:?}"
        );
        assert!(
            x1 == a.right() && y1 == a.center().1,
            "leaves A off its right side: {points:?}"
        );
        assert!(
            x4 == b.x && y4 == b.center().1,
            "enters B off its left side: {points:?}"
        );
        assert!(
            x2 > a.right() && x2 < b.x,
            "middle leg outside the gap: {points:?}"
        );
    }

    #[test]
    fn opposite_diagonal_transitions_draw_separate_lines() {
        // C keeps B out of A's column, so A and B are joined diagonally.
        let image = rasterize(&render_svg("stateDiagram-v2\nA --> B\nB --> A\nA --> C"));
        let bands = node_bands(&image);
        assert_eq!(bands.len(), 2, "expected A above B and C, got {bands:?}");
        let ((a_left, _, _, a_bottom), (b_left, b_top, _, _)) = (bands[0], bands[1]);
        assert!(b_left + 40 < a_left, "B must sit left of A: {bands:?}");
        let row = (a_bottom + b_top) / 2;
        // Columns left of A hold only the diagonal transitions on this row;
        // thin diagonal lines are faint, so match their hue at any coverage.
        let xs: Vec<u32> = (0..a_left)
            .filter(|&x| {
                let p = image.get_pixel(x, row);
                p[3] > 32 && p[2] > p[0] && p[2] > p[1]
            })
            .collect();
        let gaps: Vec<u32> = xs
            .windows(2)
            .map(|w| w[1] - w[0])
            .filter(|&g| g > 1)
            .collect();
        assert!(
            gaps.len() == 1 && gaps[0] >= 8,
            "expected two lines at least 8 px apart on row {row}, got ink at {xs:?}"
        );
    }

    /// A transition's drawn points and the center of its label.
    type LabeledRoute = (Vec<(f32, f32)>, (f32, f32));

    /// Each labeled transition's route and label, in SVG order. Labels on
    /// self-transition loops are not told apart.
    fn routes_with_labels(svg: &str) -> Vec<LabeledRoute> {
        let style = test_style();
        let mut reader = Reader::from_str(svg);
        let mut routes: Vec<Vec<(f32, f32)>> = Vec::new();
        let mut labeled = Vec::new();
        loop {
            let e = match reader.read_event().unwrap() {
                Event::Empty(e) | Event::Start(e) => e,
                Event::Eof => break,
                _ => continue,
            };
            let attr = |name: &str| {
                e.try_get_attribute(name)
                    .unwrap()
                    .map(|a| a.value.into_owned())
            };
            let num = |name: &str| attr(name).unwrap().parse::<f32>().unwrap();
            let is_edge_stroke = attr("stroke").as_deref() == Some(style.edge_stroke.as_str());
            match e.name().as_ref() {
                "line" if is_edge_stroke => {
                    routes.push(vec![(num("x1"), num("y1")), (num("x2"), num("y2"))]);
                }
                "polyline" if is_edge_stroke => routes.push(
                    attr("points")
                        .unwrap()
                        .split_whitespace()
                        .map(|p| {
                            let (x, y) = p.split_once(',').unwrap();
                            (x.parse().unwrap(), y.parse().unwrap())
                        })
                        .collect(),
                ),
                "text" if attr("fill").as_deref() == Some(style.edge_text.as_str()) => {
                    labeled.push((routes.last().unwrap().clone(), (num("x"), num("y"))));
                }
                _ => {}
            }
        }
        labeled
    }

    fn distance_to_route(route: &[(f32, f32)], (px, py): (f32, f32)) -> f32 {
        route
            .windows(2)
            .map(|w| {
                let ((x1, y1), (x2, y2)) = (w[0], w[1]);
                let (dx, dy) = (x2 - x1, y2 - y1);
                let len2 = (dx * dx + dy * dy).max(1e-6);
                let t = (((px - x1) * dx + (py - y1) * dy) / len2).clamp(0.0, 1.0);
                (px - x1 - dx * t).hypot(py - y1 - dy * t)
            })
            .fold(f32::MAX, f32::min)
    }

    #[test]
    fn transition_labels_sit_nearest_their_own_lines() {
        for source in [
            // S2 --> S1 runs along the bottom of the triangle.
            "stateDiagram-v2\nS0 --> S2: L0\nS0 --> S1: L1\nS2 --> S1: L2",
            // Stacked states joined by vertical lanes.
            "stateDiagram-v2\nA --> B: go\nB --> A: back",
            "stateDiagram-v2\nA --> B: x\nA --> B: y\nB --> A: z",
            // C keeps B out of A's column, so the lanes run diagonally.
            "stateDiagram-v2\nA --> B: go\nB --> A: back\nA --> C",
        ] {
            let labeled = routes_with_labels(&render_svg(source));
            assert_eq!(
                labeled.len(),
                source.matches(':').count(),
                "{source:?}: {labeled:?}"
            );
            for (own, center) in &labeled {
                let own_distance = distance_to_route(own, *center);
                for (other, _) in &labeled {
                    if other != own {
                        let other_distance = distance_to_route(other, *center);
                        assert!(
                            own_distance < other_distance,
                            "{source:?}: label at {center:?} is {own_distance} from its line \
                             {own:?} but {other_distance} from {other:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn state_diagram_without_start_transition_draws_no_start_dot() {
        let source = "stateDiagram-v2\nA --> B";
        let MermaidDiagram::StateDiagram(diagram) = parse_mermaid(source).unwrap() else {
            panic!("expected a state diagram");
        };
        assert!(
            !diagram.states.iter().any(|s| s.is_start),
            "{:?}",
            diagram.states
        );
        let dot_pixels = start_dot_pixels(&rasterize(&render_svg(source))).len();
        assert_eq!(dot_pixels, 0, "unexpected start dot");
    }

    #[test]
    fn composite_start_dot_sits_between_title_and_first_child() {
        let image = rasterize(&render_svg(
            "stateDiagram-v2\nstate Comp {\n    [*] --> X\n    X --> Y\n}",
        ));
        let (left, top, right, bottom) = bounds(&image, is_node_fill);
        let dot = start_dot_pixels(&image);
        assert!(!dot.is_empty(), "missing start dot");
        let header_bottom = top + (test_style().font_size * 2.0 + 16.0) as u32;
        let outside = dot
            .iter()
            .filter(|(x, y)| !(left..=right).contains(x) || !(header_bottom..=bottom).contains(y))
            .count();
        assert_eq!(
            outside, 0,
            "start dot must sit inside Comp, below its title"
        );

        // X's top border is the first outline row below the title that is
        // wider than the dot.
        let outline = |x: u32, y: u32| {
            let p = image.get_pixel(x, y);
            p[3] > 32 && p[1] > 100
        };
        let x_top = (header_bottom..bottom)
            .find(|&y| (left..=right).filter(|&x| outline(x, y)).count() > 40)
            .expect("X outline");
        let dot_bottom = dot.iter().map(|&(_, y)| y).max().unwrap();
        assert!(
            dot_bottom < x_top,
            "start dot ends at {dot_bottom}, X starts at {x_top}"
        );

        let title_edge_ink = (top..header_bottom)
            .flat_map(|y| (left..=right).map(move |x| (x, y)))
            .filter(|&(x, y)| is_edge(image.get_pixel(x, y)))
            .count();
        assert_eq!(title_edge_ink, 0, "a transition crosses the title band");
    }

    #[test]
    fn mutually_nested_states_render_without_recursing_forever() {
        let source = "stateDiagram\n    state A {\n        B --> X\n    }\n    state B {\n        A --> Y\n    }";
        assert_renders_visible("nesting cycle", source, &["A", "B", "X"]);
    }

    #[test]
    fn reversed_flowchart_directions_keep_the_leading_margin() {
        let node_origin = |direction: &str| {
            let image = rasterize(&render_svg(&format!(
                "flowchart {direction}\n    A --> B\n    A --> C"
            )));
            let (left, top, _, _) = bounds(&image, is_node_fill);
            (left, top)
        };
        assert_eq!(node_origin("BT"), node_origin("TD"), "BT vs TD");
        assert_eq!(node_origin("RL"), node_origin("LR"), "RL vs LR");
    }

    #[test]
    fn unreachable_components_sit_side_by_side_from_the_top() {
        let image = rasterize(&render_svg(
            "flowchart TD\n    R --> S\n    C1 --> C2\n    C2 --> C1\n    E1 --> E2\n    E2 --> E1",
        ));
        let bands = node_bands(&image);
        assert_eq!(bands.len(), 2, "expected two rows of nodes, got {bands:?}");
        for (_, top, _, _) in bands {
            // Above the node labels, every node in the row shows as one run.
            let row: Vec<bool> = (0..image.width())
                .map(|x| is_node_fill(image.get_pixel(x, top + 2)))
                .collect();
            let runs = row.windows(2).filter(|w| !w[0] && w[1]).count();
            assert_eq!(runs, 3, "row at {top} should hold three separate nodes");
        }
    }

    #[test]
    fn parallel_long_edges_take_no_more_room_than_one() {
        // E1 starts a second BFS at rank 0, so E1 --> C4 spans three ranks.
        let layout = |extra: &str| {
            let image = rasterize(&render_svg(&format!(
                "flowchart TD\n    C1 --> C2\n    C2 --> C3\n    C3 --> C4\n    C4 --> C1\n    E1 --> E2\n    E2 --> E1\n    E1 --> C4\n{extra}"
            )));
            (image.dimensions(), bounds(&image, is_node_fill))
        };
        assert_eq!(layout("    E1 --> C4"), layout(""));
    }

    #[test]
    fn invisible_link_places_nodes_like_a_link_but_draws_nothing() {
        let visible = rasterize(&render_svg("flowchart TD\n    A --> B"));
        let invisible = rasterize(&render_svg("flowchart TD\n    A ~~~ B"));
        assert_eq!(node_bands(&invisible), node_bands(&visible));
        let edge_pixels = invisible.pixels().filter(|p| is_edge(p)).count();
        assert_eq!(edge_pixels, 0, "invisible link was drawn");
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
            "stateDiagram-v2\n",
        ] {
            let result = render_diagram(source, &DiagramStyle::default(), &mut MockMeasure);
            assert!(result.is_err(), "{source:?} should be rejected");
        }
    }
}
