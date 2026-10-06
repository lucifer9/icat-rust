use std::collections::{HashMap, HashSet, VecDeque};

use crate::display::markdown::markie::TextMeasure;
use crate::display::markdown::markie::layout::Rect;

use super::types::*;

/// Edge and center accessors used throughout diagram layout and rendering.
pub(super) trait RectExt {
    fn right(&self) -> f32;
    fn bottom(&self) -> f32;
    fn center(&self) -> (f32, f32);
}

impl RectExt for Rect {
    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }

    fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// Intermediate waypoints for edges routed through dummy nodes.
pub type EdgeWaypoints = HashMap<(String, String), Vec<(f32, f32)>>;

// Slightly larger default spacing improves readability in dense docs.
const NODE_SPACING_X: f32 = 64.0;
const NODE_SPACING_Y: f32 = 72.0;
const EDGE_LABEL_PADDING: f32 = 8.0;
const NODE_PADDING_H: f32 = 18.0;
const NODE_PADDING_V: f32 = 12.0;

/// Baseline-to-baseline distance of class member rows, relative to the font size.
pub(super) const CLASS_MEMBER_LINE_HEIGHT: f32 = 0.9;
/// ER attribute rows are drawn slightly smaller than the entity name.
pub(super) const ER_ATTRIBUTE_FONT_SCALE: f32 = 0.9;

// Composite state interior: children are stacked with routing lanes on both sides.
pub(super) const STATE_CHILD_GAP: f32 = 20.0;
pub(super) const STATE_INNER_PAD: f32 = 16.0;
pub(super) const STATE_ROUTE_LANE: f32 = 40.0;

/// Layout engine for diagrams
pub struct LayoutEngine<'a, T: TextMeasure> {
    measure: &'a mut T,
    font_size: f32,
}

impl<'a, T: TextMeasure> LayoutEngine<'a, T> {
    pub fn new(measure: &'a mut T, font_size: f32) -> Self {
        Self { measure, font_size }
    }

    fn measure_text_width(
        &mut self,
        text: &str,
        font_size: f32,
        is_code: bool,
        bold: bool,
        italic: bool,
    ) -> f32 {
        let cleaned = crate::display::markdown::markie::xml::sanitize_xml_text(text);
        self.measure
            .measure_width(&cleaned, font_size, is_code, bold, italic)
    }

    fn measure_multiline(
        &mut self,
        text: &str,
        font_size: f32,
        is_code: bool,
        bold: bool,
        italic: bool,
    ) -> (f32, usize) {
        let mut max_width: f32 = 0.0;
        let mut lines = 0;

        for line in text.lines() {
            lines += 1;
            let width = self.measure_text_width(line, font_size, is_code, bold, italic);
            max_width = max_width.max(width);
        }

        if lines == 0 {
            lines = 1;
            max_width = self.measure_text_width(text, font_size, is_code, bold, italic);
        }

        (max_width, lines)
    }

    /// Layout a flowchart diagram using layered layout with barycenter ordering.
    pub fn layout_flowchart(
        &mut self,
        flowchart: &Flowchart,
    ) -> (HashMap<String, Rect>, EdgeWaypoints, Rect) {
        let nodes: Vec<String> = flowchart.nodes.iter().map(|n| n.id.clone()).collect();
        let edges: Vec<(String, String)> = flowchart
            .edges
            .iter()
            .map(|e| (e.from.clone(), e.to.clone()))
            .collect();
        let mut node_sizes: HashMap<String, (f32, f32)> = HashMap::new();
        for node in &flowchart.nodes {
            node_sizes.insert(
                node.id.clone(),
                self.calculate_flowchart_node_size(&node.label, &node.shape),
            );
        }

        Self::layout_layered_graph(&nodes, &edges, &node_sizes, flowchart.direction)
    }

    fn calculate_flowchart_node_size(&mut self, label: &str, shape: &NodeShape) -> (f32, f32) {
        let line_height = self.font_size * 1.2;
        let (text_width, lines) =
            self.measure_multiline(label, self.font_size, false, false, false);
        let pad_w = NODE_PADDING_H * 2.0;
        let pad_h = NODE_PADDING_V * 2.0;

        let text_h = line_height * lines as f32;
        let mut width = (text_width + pad_w).max(56.0);
        let mut height = (text_h + pad_h).max(36.0);

        match shape {
            NodeShape::Circle => {
                let size = width.max(height);
                width = size;
                height = size;
            }
            NodeShape::DoubleCircle => {
                let size = width.max(height) + 8.0;
                width = size;
                height = size;
            }
            NodeShape::Rhombus => {
                width += 26.0;
                height += 16.0;
            }
            NodeShape::Hexagon => {
                width += 24.0;
            }
            NodeShape::Parallelogram | NodeShape::ParallelogramAlt => {
                width += 20.0;
            }
            NodeShape::Trapezoid | NodeShape::TrapezoidAlt => {
                width += 16.0;
            }
            NodeShape::Stadium => {
                height = height.max(40.0);
                width = width.max(height + 20.0);
            }
            NodeShape::Cylinder => {
                height += 24.0;
            }
            NodeShape::Subroutine => {
                width += 16.0;
            }
            NodeShape::Rect | NodeShape::RoundedRect => {}
        }

        (width, height)
    }

    /// Place participants left to right. Returns their boxes and the right edge
    /// of the diagram; the renderer owns the vertical extent of the messages.
    pub fn layout_sequence(&mut self, diagram: &SequenceDiagram) -> (HashMap<String, Rect>, f32) {
        let mut positions: HashMap<String, Rect> = HashMap::new();

        let participant_height = (self.font_size * 2.4).max(36.0);
        let start_x = 40.0;
        let start_y = 20.0;

        let mut widths: Vec<f32> = Vec::with_capacity(diagram.participants.len());
        for participant in &diagram.participants {
            let label = participant.alias.as_ref().unwrap_or(&participant.id);
            let label_w = self.measure_text_width(label, self.font_size, false, false, false);
            widths.push((label_w + 28.0).max(96.0));
        }

        let mut centers: Vec<f32> = Vec::with_capacity(diagram.participants.len());
        centers.push(start_x + widths[0] / 2.0);
        for i in 1..diagram.participants.len() {
            let prev = centers[i - 1];
            // Sequence diagrams get cramped quickly; prefer a wider default gap.
            let base_gap = ((widths[i - 1] + widths[i]) / 2.0 + 72.0).max(140.0);
            centers.push(prev + base_gap);
        }

        let mut index_by_id: HashMap<&str, usize> = HashMap::new();
        for (idx, participant) in diagram.participants.iter().enumerate() {
            index_by_id.insert(participant.id.as_str(), idx);
        }

        let mut pair_requirements: Vec<(usize, usize, f32)> = Vec::new();
        self.collect_sequence_pair_requirements(
            &diagram.elements,
            &index_by_id,
            &mut pair_requirements,
        );

        for _ in 0..3 {
            let mut changed = false;
            for (a, b, required) in &pair_requirements {
                let distance = centers[*b] - centers[*a];
                if distance + 0.5 < *required {
                    let delta = *required - distance;
                    for center in centers.iter_mut().skip(*b) {
                        *center += delta;
                    }
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        for (i, participant) in diagram.participants.iter().enumerate() {
            let width = widths[i];
            let x = centers[i] - width / 2.0;
            positions.insert(
                participant.id.clone(),
                Rect::new(x, start_y, width, participant_height),
            );
        }

        let max_label_half_width = self.max_message_label_half_width(&diagram.elements);
        let right = positions
            .values()
            .map(|p| p.right())
            .fold(0.0, f32::max)
            .max(start_x + 120.0);

        (
            positions,
            right + max_label_half_width * 2.0 + EDGE_LABEL_PADDING / 2.0,
        )
    }

    fn collect_sequence_pair_requirements(
        &mut self,
        elements: &[SequenceElement],
        index_by_id: &HashMap<&str, usize>,
        out: &mut Vec<(usize, usize, f32)>,
    ) {
        for element in elements {
            match element {
                SequenceElement::Message(msg) => {
                    if let (Some(&from), Some(&to)) = (
                        index_by_id.get(msg.from.as_str()),
                        index_by_id.get(msg.to.as_str()),
                    ) && from != to
                    {
                        let a = from.min(to);
                        let b = from.max(to);
                        let label_w = self.measure_text_width(
                            &msg.label,
                            self.font_size * 0.85,
                            false,
                            false,
                            false,
                        );
                        let total_req = (label_w + 42.0).max(120.0);
                        out.push((a, b, total_req));

                        // For messages spanning multiple participants, push
                        // intermediate pairs apart so labels have room.
                        if b - a > 1 {
                            let pill_w = label_w + 20.0;
                            let per_pair = (pill_w / (b - a - 1) as f32 + 40.0).max(140.0);
                            for i in a..b {
                                out.push((i, i + 1, per_pair));
                            }
                        }
                    }
                }
                SequenceElement::Block(block) => {
                    self.collect_sequence_pair_requirements(&block.messages, index_by_id, out);
                    for (_, branch_elements) in &block.else_branches {
                        self.collect_sequence_pair_requirements(branch_elements, index_by_id, out);
                    }
                }
                SequenceElement::Activation(_)
                | SequenceElement::Deactivation(_)
                | SequenceElement::Note { .. } => {}
            }
        }
    }

    /// Half of the widest message label plus padding, searched through nested blocks.
    fn max_message_label_half_width(&mut self, elements: &[SequenceElement]) -> f32 {
        let mut max_half_width: f32 = 0.0;
        for element in elements {
            match element {
                SequenceElement::Message(msg) => {
                    let label_w = self.measure_text_width(
                        &msg.label,
                        self.font_size * 0.85,
                        false,
                        false,
                        false,
                    );
                    max_half_width = max_half_width.max(label_w / 2.0 + EDGE_LABEL_PADDING);
                }
                SequenceElement::Block(block) => {
                    let branches = std::iter::once(&block.messages)
                        .chain(block.else_branches.iter().map(|(_, elements)| elements));
                    for branch in branches {
                        max_half_width =
                            max_half_width.max(self.max_message_label_half_width(branch));
                    }
                }
                SequenceElement::Activation(_)
                | SequenceElement::Deactivation(_)
                | SequenceElement::Note { .. } => {}
            }
        }
        max_half_width
    }

    /// Layout a class diagram.
    pub fn layout_class(&mut self, diagram: &ClassDiagram) -> (HashMap<String, Rect>, Rect) {
        let mut node_sizes: HashMap<String, (f32, f32)> = HashMap::new();
        for class in &diagram.classes {
            node_sizes.insert(class.name.clone(), self.calculate_class_size(class));
        }

        let nodes: Vec<String> = diagram.classes.iter().map(|c| c.name.clone()).collect();
        let edges: Vec<(String, String)> = diagram
            .relations
            .iter()
            .map(|rel| (rel.from.clone(), rel.to.clone()))
            .collect();

        if edges.is_empty() {
            return self.layout_grid(&nodes, &node_sizes, 40.0, 40.0, 140.0, 110.0);
        }

        let (positions, _, bbox) =
            Self::layout_layered_graph(&nodes, &edges, &node_sizes, FlowDirection::TopDown);
        (positions, bbox)
    }

    fn calculate_class_size(&mut self, class: &ClassDefinition) -> (f32, f32) {
        let header_font = self.font_size;
        let member_font = self.font_size * 0.85;
        let line_h = self.font_size * CLASS_MEMBER_LINE_HEIGHT;

        let italic = class.is_abstract || class.is_interface;
        let mut max_width = self
            .measure_text_width(&class.title(), header_font, false, true, italic)
            .max(120.0);

        let member_rows = class
            .attributes
            .iter()
            .map(ClassAttribute::display)
            .chain(class.methods.iter().map(ClassMethod::display));
        for text in member_rows {
            max_width =
                max_width.max(self.measure_text_width(&text, member_font, true, false, false));
        }
        let attr_lines = class.attributes.len();
        let method_lines = class.methods.len();

        let width = (max_width + NODE_PADDING_H * 2.0).max(180.0);

        let mut height = self.font_size + 16.0;
        // The render always advances by font_size + 4.0 before the first attribute,
        // so account for that even when there are no attributes.
        height += self.font_size + 4.0;
        height += attr_lines as f32 * line_h;
        if method_lines > 0 {
            // Divider gap + first method offset + method lines
            height += 4.0 + self.font_size + 2.0;
            height += method_lines as f32 * line_h;
        }
        // Breathing room for descenders in the last member row.
        height += 14.0;
        height = height.max(64.0);

        (width, height)
    }

    /// Layout a state diagram.
    pub fn layout_state(&mut self, diagram: &StateDiagram) -> (HashMap<String, Rect>, Rect) {
        let child_state_ids = diagram.nested_state_ids();
        let states_by_id = diagram.states_by_id();

        // Parsing rejects empty diagrams and keeps nesting acyclic, so at
        // least one state is top-level.
        let target_states: Vec<&State> = diagram
            .states
            .iter()
            .filter(|state| !child_state_ids.contains(state.id.as_str()))
            .collect();

        let nodes: Vec<String> = target_states.iter().map(|s| s.id.clone()).collect();
        let node_id_set: HashSet<&str> = nodes.iter().map(String::as_str).collect();
        let mut node_sizes: HashMap<String, (f32, f32)> = HashMap::new();
        for state in &target_states {
            node_sizes.insert(state.id.clone(), self.state_size(state, &states_by_id));
        }

        let edges: Vec<(String, String)> = diagram
            .transitions
            .iter()
            .filter(|t| {
                node_id_set.contains(t.from.as_str()) && node_id_set.contains(t.to.as_str())
            })
            .map(|t| (t.from.clone(), t.to.clone()))
            .collect();

        if edges.is_empty() {
            return self.layout_grid(&nodes, &node_sizes, 40.0, 40.0, 120.0, 95.0);
        }

        let (positions, _, bbox) =
            Self::layout_layered_graph(&nodes, &edges, &node_sizes, FlowDirection::TopDown);
        (positions, bbox)
    }

    pub fn state_size(
        &mut self,
        state: &State,
        states_by_id: &HashMap<&str, &State>,
    ) -> (f32, f32) {
        if state.is_start || state.is_end {
            return (24.0, 24.0);
        }

        let label_w = self.measure_text_width(&state.label, self.font_size, false, false, false);
        let base_width = (label_w + NODE_PADDING_H * 2.0).max(120.0);
        let base_height = (self.font_size * 2.2).max(40.0);

        if !state.is_composite {
            return (base_width, base_height);
        }

        let child_sizes: Vec<(f32, f32)> = state
            .child_state_ids()
            .map(|id| self.state_size(states_by_id[id], states_by_id))
            .collect();

        if child_sizes.is_empty() {
            return (base_width, base_height);
        }

        let header_h = self.font_size * 2.0 + 16.0;

        let max_child_w: f32 = child_sizes.iter().map(|(w, _)| *w).fold(0.0, f32::max);
        let total_child_h: f32 = child_sizes.iter().map(|(_, h)| *h).sum::<f32>()
            + STATE_CHILD_GAP * (child_sizes.len() - 1) as f32;

        let width = base_width.max(max_child_w + STATE_INNER_PAD * 2.0 + STATE_ROUTE_LANE * 2.0);
        let height = header_h + total_child_h + STATE_INNER_PAD * 2.0;

        (width, height)
    }

    /// Layout an ER diagram.
    pub fn layout_er(&mut self, diagram: &ErDiagram) -> (HashMap<String, Rect>, Rect) {
        let mut seen = std::collections::HashSet::new();
        let nodes: Vec<String> = diagram
            .entities
            .iter()
            .filter(|e| seen.insert(e.name.clone()))
            .map(|e| e.name.clone())
            .collect();
        let edges: Vec<(String, String)> = diagram
            .relationships
            .iter()
            .map(|r| (r.from.clone(), r.to.clone()))
            .collect();

        let mut node_sizes: HashMap<String, (f32, f32)> = HashMap::new();
        for entity in &diagram.entities {
            node_sizes.insert(entity.name.clone(), self.calculate_er_size(entity));
        }

        if edges.is_empty() {
            return self.layout_grid(&nodes, &node_sizes, 40.0, 40.0, 180.0, 140.0);
        }

        let (positions, _, bbox) =
            Self::layout_layered_graph(&nodes, &edges, &node_sizes, FlowDirection::LeftRight);
        (positions, bbox)
    }

    fn calculate_er_size(&mut self, entity: &ErEntity) -> (f32, f32) {
        let title_font = self.font_size;
        let attr_font = self.font_size * ER_ATTRIBUTE_FONT_SCALE;

        let mut max_w = self.measure_text_width(&entity.name, title_font, false, true, false);
        for attr in &entity.attributes {
            max_w =
                max_w.max(self.measure_text_width(&attr.display(), attr_font, false, false, false));
        }

        let width = (max_w + NODE_PADDING_H * 2.0).max(150.0);
        let divider_space = if entity.attributes.is_empty() {
            0.0
        } else {
            self.font_size * 0.5 + 4.0
        };
        let height =
            (34.0 + divider_space + entity.attributes.len() as f32 * (self.font_size * 1.3) + 10.0)
                .max(56.0);
        (width, height)
    }

    fn layout_grid(
        &self,
        nodes: &[String],
        node_sizes: &HashMap<String, (f32, f32)>,
        start_x: f32,
        start_y: f32,
        spacing_x: f32,
        spacing_y: f32,
    ) -> (HashMap<String, Rect>, Rect) {
        let mut positions: HashMap<String, Rect> = HashMap::new();
        let cols = (nodes.len() as f32).sqrt().ceil() as usize;

        let mut row_heights: Vec<f32> = vec![0.0; nodes.len().div_ceil(cols)];
        let mut col_widths: Vec<f32> = vec![0.0; cols];
        for (idx, node_id) in nodes.iter().enumerate() {
            let (w, h) = node_sizes[node_id];
            row_heights[idx / cols] = row_heights[idx / cols].max(h);
            col_widths[idx % cols] = col_widths[idx % cols].max(w);
        }

        for (idx, node_id) in nodes.iter().enumerate() {
            let col = idx % cols;
            let row = idx / cols;
            let (w, h) = node_sizes[node_id];
            let y = start_y + row_heights[..row].iter().sum::<f32>() + row as f32 * spacing_y;
            let x = start_x + col_widths[..col].iter().sum::<f32>() + col as f32 * spacing_x;
            positions.insert(node_id.clone(), Rect::new(x, y, w, h));
        }

        let bbox = Self::calculate_bbox(&positions);
        (positions, bbox)
    }

    /// Layered layout: BFS ranks, dummy nodes on edges spanning several ranks,
    /// barycenter ordering, then median alignment. The core works in rank
    /// coordinates (x along a rank, y from rank to rank) that `RankFrame`
    /// transposes for left-right diagrams.
    fn layout_layered_graph(
        nodes: &[String],
        edges: &[(String, String)],
        node_sizes: &HashMap<String, (f32, f32)>,
        direction: FlowDirection,
    ) -> (HashMap<String, Rect>, EdgeWaypoints, Rect) {
        let frame = RankFrame::for_direction(direction);
        let (base_x, base_y) = (30.0, 30.0);

        let (incoming, outgoing) = adjacency(nodes, edges);
        let ranks = assign_ranks(nodes, &incoming, &outgoing);

        let mut all_ids: Vec<String> = nodes.to_vec();
        let mut all_sizes = node_sizes.clone();
        let mut all_ranks: HashMap<String, usize> =
            ranks.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let mut dummy_chains: HashMap<(String, String), Vec<String>> = HashMap::new();
        let mut augmented_edges: Vec<(String, String)> = Vec::new();
        for (from, to) in edges {
            let (Some(&from_rank), Some(&to_rank)) =
                (ranks.get(from.as_str()), ranks.get(to.as_str()))
            else {
                continue;
            };
            if to_rank <= from_rank + 1 {
                augmented_edges.push((from.clone(), to.clone()));
                continue;
            }
            let mut chain: Vec<String> = Vec::new();
            let mut prev = from.clone();
            for rank in (from_rank + 1)..to_rank {
                let dummy_id = format!("__d_{}_{}_{}", from, to, rank);
                all_ids.push(dummy_id.clone());
                all_sizes.insert(dummy_id.clone(), (0.0, 0.0));
                all_ranks.insert(dummy_id.clone(), rank);
                chain.push(dummy_id.clone());
                augmented_edges.push((prev, dummy_id.clone()));
                prev = dummy_id;
            }
            augmented_edges.push((prev, to.clone()));
            dummy_chains.insert((from.clone(), to.clone()), chain);
        }

        let order: HashMap<&str, usize> = all_ids
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        let (incoming, outgoing) = adjacency(&all_ids, &augmented_edges);

        let max_rank = all_ranks.values().copied().max().unwrap_or(0);
        let mut rank_nodes: Vec<Vec<&str>> = vec![Vec::new(); max_rank + 1];
        for id in &all_ids {
            rank_nodes[all_ranks[id]].push(id.as_str());
        }
        for rank in &mut rank_nodes {
            rank.sort_by_key(|id| order[id]);
        }
        for _ in 0..6 {
            for i in 1..rank_nodes.len() {
                let (before, after) = rank_nodes.split_at_mut(i);
                sort_by_barycenter(&mut after[0], &incoming, &before[i - 1], &order);
            }
            for i in (0..rank_nodes.len().saturating_sub(1)).rev() {
                let (before, after) = rank_nodes.split_at_mut(i + 1);
                sort_by_barycenter(&mut before[i], &outgoing, &after[0], &order);
            }
        }

        // Initial rank coordinates: each rank is centered on the widest one.
        let rank_widths: Vec<f32> = rank_nodes
            .iter()
            .map(|rank| {
                rank.iter()
                    .map(|id| frame.swap(all_sizes[*id]).0)
                    .sum::<f32>()
                    + frame.node_gap * rank.len().saturating_sub(1) as f32
            })
            .collect();
        let max_rank_w = rank_widths.iter().copied().fold(0.0, f32::max);
        let mut positions: HashMap<String, Rect> = HashMap::new();
        let mut y = base_y;
        for (rank, rank_w) in rank_nodes.iter().zip(&rank_widths) {
            let mut x = base_x + (max_rank_w - rank_w).max(0.0) / 2.0;
            let mut rank_h: f32 = 0.0;
            for id in rank {
                let (w, h) = frame.swap(all_sizes[*id]);
                positions.insert(id.to_string(), Rect::new(x, y, w, h));
                x += w + frame.node_gap;
                rank_h = rank_h.max(h);
            }
            y += rank_h + frame.rank_gap;
        }

        for _ in 0..4 {
            for rank in rank_nodes.iter().skip(1) {
                align_rank_to_median(rank, &incoming, &mut positions, frame.node_gap);
            }
            for rank in rank_nodes[..rank_nodes.len().saturating_sub(1)]
                .iter()
                .rev()
            {
                align_rank_to_median(rank, &outgoing, &mut positions, frame.node_gap);
            }
        }

        let mut edge_waypoints: EdgeWaypoints = dummy_chains
            .iter()
            .map(|(edge, chain)| {
                let waypoints = chain
                    .iter()
                    .map(|id| frame.swap(positions[id].center()))
                    .collect();
                (edge.clone(), waypoints)
            })
            .collect();
        for id in dummy_chains.values().flatten() {
            positions.remove(id);
        }
        for pos in positions.values_mut() {
            *pos = frame.swap_rect(*pos);
        }

        Self::normalize_positions(&mut positions, &mut edge_waypoints, base_x, base_y);

        let mut bbox = Self::calculate_bbox(&positions);
        if matches!(direction, FlowDirection::BottomUp) {
            let bottom = bbox.bottom();
            for pos in positions.values_mut() {
                pos.y = bottom - (pos.y + pos.h);
            }
            for wp in edge_waypoints.values_mut().flatten() {
                wp.1 = bottom - wp.1;
            }
            bbox = Self::calculate_bbox(&positions);
        } else if matches!(direction, FlowDirection::RightLeft) {
            let right = bbox.right();
            for pos in positions.values_mut() {
                pos.x = right - (pos.x + pos.w);
            }
            for wp in edge_waypoints.values_mut().flatten() {
                wp.0 = right - wp.0;
            }
            bbox = Self::calculate_bbox(&positions);
        }

        (positions, edge_waypoints, bbox)
    }

    fn normalize_positions(
        positions: &mut HashMap<String, Rect>,
        edge_waypoints: &mut EdgeWaypoints,
        target_x: f32,
        target_y: f32,
    ) {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        for pos in positions.values() {
            min_x = min_x.min(pos.x);
            min_y = min_y.min(pos.y);
        }
        let dx = target_x - min_x;
        let dy = target_y - min_y;
        if dx.abs() < 0.01 && dy.abs() < 0.01 {
            return;
        }
        for pos in positions.values_mut() {
            pos.x += dx;
            pos.y += dy;
        }
        for wps in edge_waypoints.values_mut() {
            for wp in wps.iter_mut() {
                wp.0 += dx;
                wp.1 += dy;
            }
        }
    }

    fn calculate_bbox(positions: &HashMap<String, Rect>) -> Rect {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;

        for pos in positions.values() {
            min_x = min_x.min(pos.x);
            min_y = min_y.min(pos.y);
            max_x = max_x.max(pos.right());
            max_y = max_y.max(pos.bottom());
        }

        Rect::new(min_x, min_y, max_x - min_x, max_y - min_y)
    }
}

fn barycenter(neighbors: &[&str], rank_pos: &HashMap<&str, usize>) -> Option<f32> {
    let mut total = 0.0;
    let mut count = 0.0;
    for neighbor in neighbors {
        if let Some(pos) = rank_pos.get(neighbor) {
            total += *pos as f32;
            count += 1.0;
        }
    }

    if count > 0.0 {
        Some(total / count)
    } else {
        None
    }
}

/// Maps physical axes to rank coordinates for `layout_layered_graph`.
/// Left-right diagrams swap the axes; the gap constants keep their physical
/// meaning, so the gap along a rank differs between the two orientations.
#[derive(Clone, Copy)]
struct RankFrame {
    transpose: bool,
    node_gap: f32,
    rank_gap: f32,
}

impl RankFrame {
    fn for_direction(direction: FlowDirection) -> Self {
        match direction {
            FlowDirection::TopDown | FlowDirection::BottomUp => Self {
                transpose: false,
                node_gap: NODE_SPACING_X,
                rank_gap: NODE_SPACING_Y,
            },
            FlowDirection::LeftRight | FlowDirection::RightLeft => Self {
                transpose: true,
                node_gap: NODE_SPACING_Y,
                rank_gap: NODE_SPACING_X,
            },
        }
    }

    fn swap(self, (a, b): (f32, f32)) -> (f32, f32) {
        if self.transpose { (b, a) } else { (a, b) }
    }

    fn swap_rect(self, r: Rect) -> Rect {
        if self.transpose {
            Rect::new(r.y, r.x, r.h, r.w)
        } else {
            r
        }
    }
}

type Adjacency<'a> = HashMap<&'a str, Vec<&'a str>>;

/// Returns `(incoming, outgoing)` neighbor lists in edge order, ignoring
/// edges whose endpoints are not in `ids`.
fn adjacency<'a>(
    ids: &'a [String],
    edges: &'a [(String, String)],
) -> (Adjacency<'a>, Adjacency<'a>) {
    let mut incoming: Adjacency = ids.iter().map(|id| (id.as_str(), Vec::new())).collect();
    let mut outgoing = incoming.clone();
    for (from, to) in edges {
        let (from, to) = (from.as_str(), to.as_str());
        if incoming.contains_key(from) && incoming.contains_key(to) {
            outgoing.get_mut(from).unwrap().push(to);
            incoming.get_mut(to).unwrap().push(from);
        }
    }
    (incoming, outgoing)
}

/// BFS from the sources (or the first node when every node has a parent);
/// each node still unranked afterwards starts another BFS on a new rank.
fn assign_ranks<'a>(
    nodes: &'a [String],
    incoming: &Adjacency<'a>,
    outgoing: &Adjacency<'a>,
) -> HashMap<&'a str, usize> {
    let mut ranks: HashMap<&str, usize> = HashMap::new();
    let mut queue: VecDeque<&str> = nodes
        .iter()
        .map(String::as_str)
        .filter(|id| incoming[id].is_empty())
        .collect();
    if queue.is_empty() {
        queue.extend(nodes.first().map(String::as_str));
    }
    for &root in &queue {
        ranks.insert(root, 0);
    }

    // Later components count up from the first BFS's deepest rank, so they
    // can share ranks with components placed before them.
    let mut next_rank: Option<usize> = None;
    let mut unranked = nodes.iter().map(String::as_str);
    loop {
        while let Some(id) = queue.pop_front() {
            let rank = ranks[id];
            for &child in &outgoing[id] {
                if !ranks.contains_key(child) {
                    ranks.insert(child, rank + 1);
                    queue.push_back(child);
                }
            }
        }
        let Some(start) = unranked.find(|id| !ranks.contains_key(id)) else {
            return ranks;
        };
        let rank = next_rank.get_or_insert_with(|| ranks.values().copied().max().unwrap_or(0));
        *rank += 1;
        ranks.insert(start, *rank);
        queue.push_back(start);
    }
}

/// Orders `rank` by the mean position of each node's neighbors in
/// `adjacent_rank`; nodes without such neighbors go last. Ties keep `order`.
fn sort_by_barycenter<'a>(
    rank: &mut [&'a str],
    neighbors: &Adjacency<'a>,
    adjacent_rank: &[&'a str],
    order: &HashMap<&str, usize>,
) {
    let rank_pos: HashMap<&str, usize> = adjacent_rank
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect();
    rank.sort_by(|a, b| {
        let bc_a = barycenter(&neighbors[a], &rank_pos);
        let bc_b = barycenter(&neighbors[b], &rank_pos);
        match (bc_a, bc_b) {
            (Some(x), Some(y)) => x.total_cmp(&y).then_with(|| order[a].cmp(&order[b])),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => order[a].cmp(&order[b]),
        }
    });
}

/// Centers each node on the median center of its `neighbors`, then pushes
/// nodes apart so consecutive nodes keep at least `gap` between them.
fn align_rank_to_median(
    rank: &[&str],
    neighbors: &Adjacency,
    positions: &mut HashMap<String, Rect>,
    gap: f32,
) {
    for id in rank {
        let mut centers: Vec<f32> = neighbors[id]
            .iter()
            .map(|n| positions[*n].center().0)
            .collect();
        if centers.is_empty() {
            continue;
        }
        centers.sort_by(f32::total_cmp);
        let median = centers[centers.len() / 2];
        let pos = positions.get_mut(*id).unwrap();
        pos.x = median - pos.w / 2.0;
    }
    let mut prev_right = f32::NEG_INFINITY;
    for id in rank {
        let pos = positions.get_mut(*id).unwrap();
        pos.x = pos.x.max(prev_right + gap);
        prev_right = pos.right();
    }
}
