use std::collections::HashMap;

use crate::display::markdown::markie::TextMeasure;

use super::layout::{LayoutEngine, RectExt};
use super::render::{ArrowHead, DiagramStyle, arrowhead};
use super::types::{ArrowType, EdgeStyle, FlowDirection, Flowchart, NodeShape, Subgraph};
use crate::display::markdown::markie::layout::Rect;
use crate::display::markdown::markie::xml::{escape_xml, sanitize_xml_text};

/// Render a flowchart to SVG
pub fn render_flowchart(
    flowchart: &Flowchart,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
) -> (String, f32, f32) {
    let mut layout = LayoutEngine::new(measure, style.font_size);
    let (positions, edge_waypoints, bbox) = layout.layout_flowchart(flowchart);

    let mut svg = String::new();
    let padding = 20.0;

    let node_map: HashMap<&str, &super::types::FlowchartNode> =
        flowchart.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // Subgraphs without positioned nodes have no box.
    let subgraph_boxes: Vec<(&Subgraph, Rect)> = flowchart
        .subgraphs
        .iter()
        .filter_map(|subgraph| Some((subgraph, subgraph_bbox(subgraph, &positions)?)))
        .collect();

    // Draw subgraph boxes first (background layer)
    for (_, bbox) in &subgraph_boxes {
        svg.push_str(&render_subgraph_box(bbox, style));
    }

    // Draw edges (behind nodes but on top of subgraph boxes)
    for edge in &flowchart.edges {
        let from_pos = positions.get(&edge.from);
        let to_pos = positions.get(&edge.to);
        let from_node = node_map.get(edge.from.as_str());
        let to_node = node_map.get(edge.to.as_str());

        if let (Some(from), Some(to), Some(fn_), Some(tn)) = (from_pos, to_pos, from_node, to_node)
        {
            let waypoints = edge_waypoints
                .get(&(edge.from.clone(), edge.to.clone()))
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            svg.push_str(&render_edge(&mut RenderEdgeContext {
                edge,
                from_node: fn_,
                from,
                to_node: tn,
                to,
                style,
                direction: &flowchart.direction,
                waypoints,
                measure,
            }));
        }
    }

    // Draw nodes on top
    for node in &flowchart.nodes {
        if let Some(pos) = positions.get(&node.id) {
            svg.push_str(&render_node(&node.label, &node.shape, pos, style));
        }
    }

    // Draw subgraph titles last (on top of everything) with collision avoidance
    let mut used_title_rects: Vec<Rect> = Vec::new();
    for (subgraph, bbox) in &subgraph_boxes {
        svg.push_str(&render_subgraph_title(
            &subgraph.title,
            bbox,
            style,
            measure,
            &mut used_title_rects,
        ));
    }

    let total_width = bbox.right() + padding;
    let total_height = bbox.bottom() + padding;

    (svg, total_width, total_height)
}

fn render_node(label: &str, shape: &NodeShape, pos: &Rect, style: &DiagramStyle) -> String {
    let mut svg = String::new();
    let escaped_label = escape_xml(label);

    match shape {
        NodeShape::Rect => {
            svg.push_str(&format!(
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x, pos.y, pos.w, pos.h,
                style.node_fill, style.node_stroke
            ));
        }
        NodeShape::RoundedRect => {
            let rx = 6.0_f32.min(pos.h / 4.0);
            svg.push_str(&format!(
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x, pos.y, pos.w, pos.h, rx,
                style.node_fill, style.node_stroke
            ));
        }
        NodeShape::Stadium => {
            let rx = pos.h / 2.0;
            svg.push_str(&format!(
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x, pos.y, pos.w, pos.h, rx,
                style.node_fill, style.node_stroke
            ));
        }
        NodeShape::Subroutine => {
            // Rect with vertical lines at ends
            svg.push_str(&format!(
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x, pos.y, pos.w, pos.h,
                style.node_fill, style.node_stroke
            ));
            let line_offset = 6.0;
            svg.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1" />"#,
                pos.x + line_offset, pos.y, pos.x + line_offset, pos.y + pos.h, style.node_stroke
            ));
            svg.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1" />"#,
                pos.x + pos.w - line_offset, pos.y, pos.x + pos.w - line_offset, pos.y + pos.h, style.node_stroke
            ));
        }
        NodeShape::Cylinder => {
            let cap_height = 12.0;
            let rx = pos.w / 2.0;
            let bottom_y = pos.y + pos.h - cap_height;
            // Body: left side, bottom arc, right side (filled, no top/bottom strokes)
            svg.push_str(&format!(
                r#"<path d="M {:.2} {:.2} L {:.2} {:.2} A {:.2} {:.2} 0 0 0 {:.2} {:.2} L {:.2} {:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x, pos.y + cap_height,
                pos.x, bottom_y,
                rx, cap_height,
                pos.x + pos.w, bottom_y,
                pos.x + pos.w, pos.y + cap_height,
                style.node_fill, style.node_stroke
            ));
            // Top ellipse (full, drawn on top of body)
            svg.push_str(&format!(
                r#"<ellipse cx="{:.2}" cy="{:.2}" rx="{:.2}" ry="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x + pos.w / 2.0, pos.y + cap_height,
                rx, cap_height,
                style.node_fill, style.node_stroke
            ));
        }
        NodeShape::Circle => {
            let radius = pos.w.min(pos.h) / 2.0;
            let cx = pos.x + pos.w / 2.0;
            let cy = pos.y + pos.h / 2.0;
            svg.push_str(&format!(
                r#"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                cx, cy, radius, style.node_fill, style.node_stroke
            ));
        }
        NodeShape::DoubleCircle => {
            let radius = pos.w.min(pos.h) / 2.0 - 4.0;
            let cx = pos.x + pos.w / 2.0;
            let cy = pos.y + pos.h / 2.0;
            svg.push_str(&format!(
                r#"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                cx, cy, radius + 4.0, style.node_fill, style.node_stroke
            ));
            svg.push_str(&format!(
                r#"<circle cx="{:.2}" cy="{:.2}" r="{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                cx, cy, radius, style.node_fill, style.node_stroke
            ));
        }
        NodeShape::Rhombus => {
            let cx = pos.x + pos.w / 2.0;
            let cy = pos.y + pos.h / 2.0;
            svg.push_str(&format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                cx, pos.y,
                pos.x + pos.w, cy,
                cx, pos.y + pos.h,
                pos.x, cy,
                style.node_fill, style.node_stroke
            ));
        }
        NodeShape::Hexagon => {
            let offset = 15.0_f32.min(pos.w / 4.0);
            svg.push_str(&format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x + offset, pos.y,
                pos.x + pos.w - offset, pos.y,
                pos.x + pos.w, pos.y + pos.h / 2.0,
                pos.x + pos.w - offset, pos.y + pos.h,
                pos.x + offset, pos.y + pos.h,
                pos.x, pos.y + pos.h / 2.0,
                style.node_fill, style.node_stroke
            ));
        }
        NodeShape::Parallelogram => {
            let offset = 20.0_f32.min(pos.w / 3.0);
            svg.push_str(&format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x + offset, pos.y,
                pos.x + pos.w, pos.y,
                pos.x + pos.w - offset, pos.y + pos.h,
                pos.x, pos.y + pos.h,
                style.node_fill, style.node_stroke
            ));
        }
        NodeShape::ParallelogramAlt => {
            let offset = 20.0_f32.min(pos.w / 3.0);
            svg.push_str(&format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x, pos.y,
                pos.x + pos.w - offset, pos.y,
                pos.x + pos.w, pos.y + pos.h,
                pos.x + offset, pos.y + pos.h,
                style.node_fill, style.node_stroke
            ));
        }
        NodeShape::Trapezoid => {
            let offset = 15.0_f32.min(pos.w / 4.0);
            svg.push_str(&format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x + offset, pos.y,
                pos.x + pos.w - offset, pos.y,
                pos.x + pos.w, pos.y + pos.h,
                pos.x, pos.y + pos.h,
                style.node_fill, style.node_stroke
            ));
        }
        NodeShape::TrapezoidAlt => {
            let offset = 15.0_f32.min(pos.w / 4.0);
            svg.push_str(&format!(
                r#"<polygon points="{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}" fill="{}" stroke="{}" stroke-width="1" />"#,
                pos.x, pos.y,
                pos.x + pos.w, pos.y,
                pos.x + pos.w - offset, pos.y + pos.h,
                pos.x + offset, pos.y + pos.h,
                style.node_fill, style.node_stroke
            ));
        }
    }

    // Draw label
    let text_x = pos.x + pos.w / 2.0;
    let text_y = pos.y + pos.h / 2.0;

    // Handle multi-line labels
    let lines: Vec<&str> = escaped_label.lines().collect();
    let line_height = style.font_size * 1.2;
    let total_height = line_height * lines.len() as f32;
    let start_y = text_y - (total_height / 2.0) + line_height / 2.0;

    for (i, line) in lines.iter().enumerate() {
        let y = start_y + i as f32 * line_height;
        svg.push_str(&format!(
            r#"<text x="{:.2}" y="{:.2}" dy="0.35em" font-family="{}" font-size="{:.1}" font-weight="500" fill="{}" text-anchor="middle">{}</text>"#,
            text_x, y, style.font_family, style.font_size, style.node_text, line
        ));
    }

    svg
}

/// Clip a point from center of a node to its shape boundary.
fn clip_to_shape(
    node: &super::types::FlowchartNode,
    pos: &Rect,
    target_x: f32,
    target_y: f32,
) -> (f32, f32) {
    let cx = pos.x + pos.w / 2.0;
    let cy = pos.y + pos.h / 2.0;
    let dx = target_x - cx;
    let dy = target_y - cy;
    if dx.abs() < 0.001 && dy.abs() < 0.001 {
        return (cx, cy);
    }

    match node.shape {
        NodeShape::Circle | NodeShape::DoubleCircle => {
            let r = pos.w.min(pos.h) / 2.0;
            let dist = (dx * dx + dy * dy).sqrt();
            (cx + dx / dist * r, cy + dy / dist * r)
        }
        NodeShape::Rhombus => {
            let hw = pos.w / 2.0;
            let hh = pos.h / 2.0;
            let t = 1.0 / (dx.abs() / hw + dy.abs() / hh);
            (cx + dx * t, cy + dy * t)
        }
        _ => {
            let hw = pos.w / 2.0;
            let hh = pos.h / 2.0;
            let scale_x = if dx.abs() > 0.001 {
                hw / dx.abs()
            } else {
                f32::MAX
            };
            let scale_y = if dy.abs() > 0.001 {
                hh / dy.abs()
            } else {
                f32::MAX
            };
            let scale = scale_x.min(scale_y);
            (cx + dx * scale, cy + dy * scale)
        }
    }
}

/// Cubic Bézier loop for an edge from a node back to itself. It re-enters at
/// the top center and leaves away from the primary flow axis so it does not
/// cross neighbouring nodes.
pub(super) struct SelfLoop {
    pub start: (f32, f32),
    pub control1: (f32, f32),
    pub control2: (f32, f32),
    pub end: (f32, f32),
}

impl SelfLoop {
    pub const RADIUS: f32 = 20.0;

    pub fn new(pos: &Rect, direction: FlowDirection) -> Self {
        let r = Self::RADIUS;
        let (cx, cy) = pos.center();
        let end = (cx, pos.y);
        match direction {
            // Horizontal graph: loop upward
            FlowDirection::LeftRight | FlowDirection::RightLeft => Self {
                start: end,
                control1: (cx + r, pos.y - r * 1.5),
                control2: (cx - r, pos.y - r * 1.5),
                end,
            },
            // Vertical graph: loop out of the right side
            FlowDirection::TopDown | FlowDirection::BottomUp => Self {
                start: (pos.right(), cy),
                control1: (pos.right() + r, cy - r),
                control2: (cx + r, pos.y - r),
                end,
            },
        }
    }

    pub fn path_data(&self) -> String {
        format!(
            "M {:.2},{:.2} C {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}",
            self.start.0,
            self.start.1,
            self.control1.0,
            self.control1.1,
            self.control2.0,
            self.control2.1,
            self.end.0,
            self.end.1
        )
    }

    /// Direction of travel where the curve re-enters the node.
    pub fn end_angle(&self) -> f32 {
        (self.end.1 - self.control2.1).atan2(self.end.0 - self.control2.0)
    }
}

struct RenderEdgeContext<'a, T: TextMeasure> {
    edge: &'a super::types::FlowchartEdge,
    from_node: &'a super::types::FlowchartNode,
    from: &'a Rect,
    to_node: &'a super::types::FlowchartNode,
    to: &'a Rect,
    style: &'a DiagramStyle,
    direction: &'a FlowDirection,
    waypoints: &'a [(f32, f32)],
    measure: &'a mut T,
}

fn render_edge<T: TextMeasure>(ctx: &mut RenderEdgeContext<'_, T>) -> String {
    let edge = ctx.edge;
    let from_node = ctx.from_node;
    let from = ctx.from;
    let to_node = ctx.to_node;
    let to = ctx.to;
    let style = ctx.style;
    let direction = ctx.direction;
    let waypoints = ctx.waypoints;
    let measure = &mut *ctx.measure;

    let mut svg = String::new();
    let vertical = matches!(direction, FlowDirection::TopDown | FlowDirection::BottomUp);

    let (dash_attr, stroke_width) = match edge.style {
        EdgeStyle::Solid => ("", 0.75),
        EdgeStyle::Dotted => (" stroke-dasharray=\"4,4\"", 0.75),
        EdgeStyle::Thick => ("", 1.5),
    };

    let from_cx = from.x + from.w / 2.0;
    let from_cy = from.y + from.h / 2.0;
    let to_cx = to.x + to.w / 2.0;
    let to_cy = to.y + to.h / 2.0;

    if edge.from == edge.to {
        let self_loop = SelfLoop::new(from, *direction);
        svg.push_str(&format!(
            r#"<path d="{}" fill="none" stroke="{}" stroke-width="{:.2}"{} />"#,
            self_loop.path_data(),
            style.edge_stroke,
            stroke_width,
            dash_attr
        ));

        if edge.arrow_head != ArrowType::None {
            let (enter_x, enter_y) = self_loop.end;
            svg.push_str(&render_arrow_head(
                enter_x,
                enter_y,
                self_loop.end_angle(),
                &edge.arrow_head,
                style,
            ));
        }

        // Edge label
        if let Some(ref label) = edge.label {
            let escaped = escape_xml(label);
            let label_w = measure.measure_width(label, style.font_size * 0.82, false, false, false);
            let label_x = self_loop.start.0 + SelfLoop::RADIUS - label_w / 2.0;
            let label_y = from.y - SelfLoop::RADIUS;
            svg.push_str(&format!(
                r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="4" fill="{}" stroke="{}" stroke-width="0.5" />"#,
                label_x - 2.0,
                label_y - style.font_size * 0.7,
                label_w + 4.0,
                style.font_size * 1.1,
                style.background,
                style.node_stroke
            ));
            svg.push_str(&format!(
                r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}" text-anchor="start">{}</text>"#,
                label_x, label_y, style.font_family, style.font_size * 0.82, style.edge_text, escaped
            ));
        }

        return svg;
    }

    // Determine exit and entry ports based on flow direction and relative position
    let (exit_x, exit_y, enter_x, enter_y) = if vertical {
        let going_down = from_cy <= to_cy;
        let ey = if going_down { from.bottom() } else { from.y };
        let ny = if going_down { to.y } else { to.bottom() };
        // Handle same-rank (same y) case: route horizontally
        if (from_cy - to_cy).abs() < 1.0 {
            let going_right = from_cx < to_cx;
            let ex = if going_right { from.right() } else { from.x };
            let nx = if going_right { to.x } else { to.right() };
            (ex, from_cy, nx, to_cy)
        } else {
            (from_cx, ey, to_cx, ny)
        }
    } else {
        let going_right = from_cx <= to_cx;
        let ex = if going_right { from.right() } else { from.x };
        let nx = if going_right { to.x } else { to.right() };
        // Handle same-column case: route vertically
        if (from_cx - to_cx).abs() < 1.0 {
            let going_down = from_cy < to_cy;
            let ey = if going_down { from.bottom() } else { from.y };
            let ny = if going_down { to.y } else { to.bottom() };
            (from_cx, ey, to_cx, ny)
        } else {
            (ex, from_cy, nx, to_cy)
        }
    };

    // Clip exit/enter points to actual shape boundaries
    let (exit_x, exit_y) = clip_to_shape(from_node, from, exit_x, exit_y);
    let (enter_x, enter_y) = clip_to_shape(to_node, to, enter_x, enter_y);

    // Build polyline points through waypoints
    let mut path: Vec<(f32, f32)> = Vec::with_capacity(waypoints.len() + 2);
    path.push((exit_x, exit_y));
    path.extend_from_slice(waypoints);
    path.push((enter_x, enter_y));

    let mut points: Vec<(f32, f32)> = vec![path[0]];
    for pair in path.windows(2) {
        let ((x1, y1), (x2, y2)) = (pair[0], pair[1]);

        if vertical || ((from_cy - to_cy).abs() < 1.0 && waypoints.is_empty()) {
            // Vertical primary axis (or horizontal same-rank): dogleg with vertical-first
            if (x1 - x2).abs() > 0.5 {
                let mid_y = (y1 + y2) / 2.0;
                points.push((x1, mid_y));
                points.push((x2, mid_y));
            }
        } else {
            // Horizontal primary axis: dogleg with horizontal-first
            if (y1 - y2).abs() > 0.5 {
                let mid_x = (x1 + x2) / 2.0;
                points.push((mid_x, y1));
                points.push((mid_x, y2));
            }
        }
        points.push((x2, y2));
    }

    // Render polyline
    let points_str: String = points
        .iter()
        .map(|(x, y)| format!("{:.2},{:.2}", x, y))
        .collect::<Vec<_>>()
        .join(" ");
    svg.push_str(&format!(
        r#"<polyline points="{}" fill="none" stroke="{}" stroke-width="{:.2}" stroke-linecap="round" stroke-linejoin="round"{} />"#,
        points_str, style.edge_stroke, stroke_width, dash_attr
    ));

    // `points` always holds at least the exit and entry points.
    let last = points[points.len() - 1];
    let prev = points[points.len() - 2];
    let head_angle = (last.1 - prev.1).atan2(last.0 - prev.0);

    if edge.arrow_head != ArrowType::None {
        svg.push_str(&render_arrow_head(
            enter_x,
            enter_y,
            head_angle,
            &edge.arrow_head,
            style,
        ));
    }

    // Arrow tail
    if edge.arrow_tail != ArrowType::None {
        let (first, second) = (points[0], points[1]);
        let tail_angle = (first.1 - second.1).atan2(first.0 - second.0);
        svg.push_str(&render_arrow_head(
            exit_x,
            exit_y,
            tail_angle,
            &edge.arrow_tail,
            style,
        ));
    }

    // Label on middle segment
    let mid = points.len() / 2;
    let label_x = (points[mid - 1].0 + points[mid].0) / 2.0;
    let label_y = (points[mid - 1].1 + points[mid].1) / 2.0;

    if let Some(ref label) = edge.label {
        let cleaned = sanitize_xml_text(label);
        let label_font_size = style.font_size * 0.85;
        let text_w = measure.measure_width(&cleaned, label_font_size, false, false, false);
        let pill_pad = 8.0;
        let pill_w = (text_w + pill_pad * 2.0).max(label_font_size * 2.5);
        let pill_h = label_font_size + pill_pad * 2.0;

        svg.push_str(&format!(
            r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="4" fill="{}" stroke="{}" stroke-width="0.5" />"#,
            label_x - pill_w / 2.0,
            label_y - pill_h / 2.0,
            pill_w,
            pill_h,
            style.background,
            style.node_stroke
        ));

        svg.push_str(&format!(
            r#"<text x="{:.2}" y="{:.2}" dy="0.35em" font-family="{}" font-size="{:.1}" fill="{}" text-anchor="middle">{}</text>"#,
            label_x,
            label_y,
            style.font_family,
            label_font_size,
            style.edge_text,
            escape_xml(&cleaned)
        ));
    }

    svg
}

fn render_arrow_head(
    x: f32,
    y: f32,
    angle: f32,
    arrow_type: &ArrowType,
    style: &DiagramStyle,
) -> String {
    let cos = angle.cos();
    let sin = angle.sin();

    match arrow_type {
        ArrowType::Arrow => arrowhead(ArrowHead::Filled, x, y, angle, style),
        ArrowType::Circle => {
            format!(
                r#"<circle cx="{:.2}" cy="{:.2}" r="5" fill="{}" stroke="{}" stroke-width="1" />"#,
                x - cos * 5.0,
                y - sin * 5.0,
                style.node_fill,
                style.edge_stroke
            )
        }
        ArrowType::Cross => {
            let s = 7.0_f32;
            let cx = x - cos * s;
            let cy = y - sin * s;
            // Rotate ±45° from the edge direction for an "×" shape
            let angle_a = angle + std::f32::consts::FRAC_PI_4;
            let angle_b = angle - std::f32::consts::FRAC_PI_4;
            let (ca, sa) = (angle_a.cos(), angle_a.sin());
            let (cb, sb) = (angle_b.cos(), angle_b.sin());
            format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="2.5" /><line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="2.5" />"#,
                cx + ca * s,
                cy + sa * s,
                cx - ca * s,
                cy - sa * s,
                style.edge_stroke,
                cx + cb * s,
                cy + sb * s,
                cx - cb * s,
                cy - sb * s,
                style.edge_stroke
            )
        }
        ArrowType::None => String::new(),
    }
}

fn subgraph_bbox(subgraph: &Subgraph, positions: &HashMap<String, Rect>) -> Option<Rect> {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_right = f32::MIN;
    let mut max_bottom = f32::MIN;
    let mut found = false;

    for node_id in &subgraph.nodes {
        if let Some(pos) = positions.get(node_id) {
            found = true;
            min_x = min_x.min(pos.x);
            min_y = min_y.min(pos.y);
            max_right = max_right.max(pos.right());
            max_bottom = max_bottom.max(pos.bottom());
        }
    }

    if !found {
        return None;
    }

    let content_bbox = Rect::new(min_x, min_y, max_right - min_x, max_bottom - min_y);
    let padded_bbox = content_bbox.expanded(20.0);
    // Extra room above the content holds the title; clamping it at the canvas
    // top must not move the bottom edge.
    let top = (padded_bbox.y - 20.0).max(0.0);
    Some(Rect::new(
        padded_bbox.x,
        top,
        padded_bbox.w,
        padded_bbox.bottom() - top,
    ))
}

fn render_subgraph_box(bbox: &Rect, style: &DiagramStyle) -> String {
    format!(
        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="8" fill="{}" fill-opacity="0.3" stroke="{}" stroke-width="1" stroke-dasharray="4,2" />"#,
        bbox.x, bbox.y, bbox.w, bbox.h, style.node_fill, style.node_stroke
    )
}

fn render_subgraph_title(
    title: &str,
    bbox: &Rect,
    style: &DiagramStyle,
    measure: &mut impl TextMeasure,
    used_rects: &mut Vec<Rect>,
) -> String {
    if title.is_empty() {
        return String::new();
    }

    let title = sanitize_xml_text(title);
    let title_font = style.font_size * 0.9;
    let text_w = measure.measure_width(&title, title_font, false, true, false);
    let title_w = text_w + 12.0;
    let title_h = title_font + 6.0;

    let title_x = bbox.x + 12.0;
    let mut title_y = bbox.y + title_font + 4.0;

    // Offset title if it would overlap a previously placed title
    for used in used_rects.iter() {
        let candidate = Rect::new(
            title_x - 4.0,
            title_y - title_h / 2.0 - 1.0,
            title_w,
            title_h,
        );
        if candidate.overlaps(used) {
            title_y = used.y + used.h + title_h / 2.0 + 3.0;
        }
    }

    let pill_x = title_x - 4.0;
    let pill_y = title_y - title_h / 2.0 - 1.0;
    used_rects.push(Rect::new(pill_x, pill_y, title_w, title_h));

    let mut svg = String::new();
    svg.push_str(&format!(
        r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" rx="3" fill="{}" fill-opacity="0.9" />"#,
        pill_x, pill_y, title_w, title_h, style.node_fill
    ));
    svg.push_str(&format!(
        r#"<text x="{:.2}" y="{:.2}" font-family="{}" font-size="{:.1}" fill="{}" font-weight="bold" text-anchor="start">{}</text>"#,
        title_x, title_y, style.font_family, title_font, style.node_text, escape_xml(&title)
    ));

    svg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::markdown::markie::MockMeasure;

    fn first_polygon_points(svg: &str) -> Vec<(f32, f32)> {
        let marker = "<polygon points=\"";
        let start = svg.find(marker).expect("expected polygon");
        let rest = &svg[start + marker.len()..];
        let end = rest.find('"').expect("expected polygon points close quote");
        rest[..end]
            .split_whitespace()
            .map(|pair| {
                let mut it = pair.split(',');
                let x = it
                    .next()
                    .expect("x")
                    .parse::<f32>()
                    .expect("x should parse");
                let y = it
                    .next()
                    .expect("y")
                    .parse::<f32>()
                    .expect("y should parse");
                (x, y)
            })
            .collect()
    }

    #[test]
    fn orthogonal_edge_arrow_points_in_correct_direction() {
        let mut measure = MockMeasure;
        let style = DiagramStyle::default();
        let edge = super::super::types::FlowchartEdge {
            from: "A".to_string(),
            to: "B".to_string(),
            label: None,
            style: EdgeStyle::Solid,
            arrow_head: ArrowType::Arrow,
            arrow_tail: ArrowType::None,
        };
        let from_node = super::super::types::FlowchartNode {
            id: "A".to_string(),
            label: "A".to_string(),
            shape: NodeShape::Rect,
        };
        let to_node = super::super::types::FlowchartNode {
            id: "B".to_string(),
            label: "B".to_string(),
            shape: NodeShape::Rect,
        };
        let from = Rect::new(0.0, 0.0, 100.0, 40.0);
        let to = Rect::new(200.0, 100.0, 100.0, 40.0);

        let svg = render_edge(&mut RenderEdgeContext {
            edge: &edge,
            from_node: &from_node,
            from: &from,
            to_node: &to_node,
            to: &to,
            style: &style,
            direction: &FlowDirection::LeftRight,
            waypoints: &[],
            measure: &mut measure,
        });

        let pts = first_polygon_points(&svg);
        assert_eq!(pts.len(), 3);

        // Orthogonal routing: exit right of from=(100,20), enter left of to=(200,120)
        // Arrow tip at entry point (200, 120), pointing right (angle=0)
        assert!((pts[0].0 - 200.0).abs() < 1.0, "tip x={}", pts[0].0);
        assert!((pts[0].1 - 120.0).abs() < 1.0, "tip y={}", pts[0].1);
    }

    #[test]
    fn self_loop_arrow_points_into_top_of_node() {
        let edge = super::super::types::FlowchartEdge {
            from: "A".to_string(),
            to: "A".to_string(),
            label: None,
            style: EdgeStyle::Solid,
            arrow_head: ArrowType::Arrow,
            arrow_tail: ArrowType::None,
        };
        let node = super::super::types::FlowchartNode {
            id: "A".to_string(),
            label: "A".to_string(),
            shape: NodeShape::Rect,
        };
        let pos = Rect::new(0.0, 0.0, 100.0, 40.0);

        for direction in [FlowDirection::LeftRight, FlowDirection::TopDown] {
            let svg = render_edge(&mut RenderEdgeContext {
                edge: &edge,
                from_node: &node,
                from: &pos,
                to_node: &node,
                to: &pos,
                style: &DiagramStyle::default(),
                direction: &direction,
                waypoints: &[],
                measure: &mut MockMeasure,
            });

            let pts = first_polygon_points(&svg);
            assert_eq!(pts.len(), 3);
            // The loop re-enters at the top center; the arrow body must sit above
            // the node so it is not hidden behind the node fill.
            assert!(
                (pts[0].0 - 50.0).abs() < 1.0,
                "{direction:?} tip x={}",
                pts[0].0
            );
            assert!(pts[0].1.abs() < 1.0, "{direction:?} tip y={}", pts[0].1);
            for (x, y) in &pts[1..] {
                assert!(
                    *y < -1.0,
                    "{direction:?} arrow base ({x}, {y}) is inside the node"
                );
            }
        }
    }
}
