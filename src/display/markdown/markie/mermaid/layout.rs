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

        self.layout_layered_graph(&nodes, &edges, &node_sizes, flowchart.direction)
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
            self.layout_layered_graph(&nodes, &edges, &node_sizes, FlowDirection::TopDown);
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

        // The implicit start state is never nested, so this is never empty.
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
            self.layout_layered_graph(&nodes, &edges, &node_sizes, FlowDirection::TopDown);
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
            self.layout_layered_graph(&nodes, &edges, &node_sizes, FlowDirection::LeftRight);
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
        for (idx, node_id) in nodes.iter().enumerate() {
            let row = idx / cols;
            let (_, h) = node_sizes[node_id];
            row_heights[row] = row_heights[row].max(h);
        }

        for (idx, node_id) in nodes.iter().enumerate() {
            let col = idx % cols;
            let row = idx / cols;
            let (w, h) = node_sizes[node_id];
            let y = start_y + row_heights[..row].iter().sum::<f32>() + row as f32 * spacing_y;
            let x = start_x + col as f32 * (w + spacing_x);
            positions.insert(node_id.clone(), Rect::new(x, y, w, h));
        }

        let bbox = Self::calculate_bbox(&positions);
        (positions, bbox)
    }

    fn layout_layered_graph(
        &self,
        nodes: &[String],
        edges: &[(String, String)],
        node_sizes: &HashMap<String, (f32, f32)>,
        direction: FlowDirection,
    ) -> (HashMap<String, Rect>, EdgeWaypoints, Rect) {
        let mut positions: HashMap<String, Rect> = HashMap::new();

        let order_index: HashMap<&str, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();

        let mut incoming_init: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut outgoing_init: HashMap<&str, Vec<&str>> = HashMap::new();

        for node in nodes {
            incoming_init.entry(node.as_str()).or_default();
            outgoing_init.entry(node.as_str()).or_default();
        }

        for (from, to) in edges {
            if !order_index.contains_key(from.as_str()) || !order_index.contains_key(to.as_str()) {
                continue;
            }
            outgoing_init
                .entry(from.as_str())
                .or_default()
                .push(to.as_str());
            incoming_init
                .entry(to.as_str())
                .or_default()
                .push(from.as_str());
        }

        let mut ranks: HashMap<&str, usize> = HashMap::new();
        let roots: Vec<&str> = nodes
            .iter()
            .map(String::as_str)
            .filter(|n| {
                incoming_init
                    .get(n)
                    .is_none_or(|parents| parents.is_empty())
            })
            .collect();

        let mut queue: VecDeque<&str> = VecDeque::new();
        if roots.is_empty() {
            if let Some(first) = nodes.first() {
                ranks.insert(first.as_str(), 0);
                queue.push_back(first.as_str());
            }
        } else {
            for root in roots {
                ranks.insert(root, 0);
                queue.push_back(root);
            }
        }

        while let Some(node) = queue.pop_front() {
            let rank = ranks[node];
            for &neighbor in &outgoing_init[node] {
                if !ranks.contains_key(neighbor) {
                    ranks.insert(neighbor, rank + 1);
                    queue.push_back(neighbor);
                }
            }
        }

        let mut max_rank = ranks.values().copied().max().unwrap_or(0);
        for node in nodes {
            if !ranks.contains_key(node.as_str()) {
                max_rank += 1;
                ranks.insert(node.as_str(), max_rank);
                queue.push_back(node.as_str());

                while let Some(cur) = queue.pop_front() {
                    let rank = ranks[cur];
                    for &neighbor in &outgoing_init[cur] {
                        if !ranks.contains_key(neighbor) {
                            ranks.insert(neighbor, rank + 1);
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
        }

        // --- Phase 2: Insert dummy nodes for long edges ---
        let mut all_node_ids: Vec<String> = nodes.to_vec();
        let mut all_sizes: HashMap<String, (f32, f32)> = node_sizes.clone();
        let mut ranks_owned: HashMap<String, usize> =
            ranks.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let mut dummy_set: HashSet<String> = HashSet::new();
        let mut edge_dummy_chains: HashMap<(String, String), Vec<String>> = HashMap::new();
        let mut augmented_edges: Vec<(String, String)> = Vec::new();

        for (from, to) in edges {
            if !order_index.contains_key(from.as_str()) || !order_index.contains_key(to.as_str()) {
                augmented_edges.push((from.clone(), to.clone()));
                continue;
            }
            let from_rank = ranks[from.as_str()];
            let to_rank = ranks[to.as_str()];

            if to_rank > from_rank + 1 {
                let mut chain: Vec<String> = Vec::new();
                let mut prev = from.clone();
                for rank in (from_rank + 1)..to_rank {
                    let dummy_id = format!("__d_{}_{}_{}", from, to, rank);
                    all_node_ids.push(dummy_id.clone());
                    all_sizes.insert(dummy_id.clone(), (0.0, 0.0));
                    ranks_owned.insert(dummy_id.clone(), rank);
                    dummy_set.insert(dummy_id.clone());
                    chain.push(dummy_id.clone());
                    augmented_edges.push((prev, dummy_id.clone()));
                    prev = dummy_id;
                }
                augmented_edges.push((prev, to.clone()));
                edge_dummy_chains.insert((from.clone(), to.clone()), chain);
            } else {
                augmented_edges.push((from.clone(), to.clone()));
            }
        }

        // Rebuild data structures with dummies
        let max_rank = ranks_owned.values().copied().max().unwrap_or(0);
        let aug_order: HashMap<&str, usize> = all_node_ids
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();

        let mut incoming: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();

        for node in &all_node_ids {
            incoming.entry(node.as_str()).or_default();
            outgoing.entry(node.as_str()).or_default();
        }

        for (from, to) in &augmented_edges {
            if aug_order.contains_key(from.as_str()) && aug_order.contains_key(to.as_str()) {
                outgoing.entry(from.as_str()).or_default().push(to.as_str());
                incoming.entry(to.as_str()).or_default().push(from.as_str());
            }
        }

        let mut rank_nodes: Vec<Vec<&str>> = vec![Vec::new(); max_rank + 1];
        for node in &all_node_ids {
            rank_nodes[ranks_owned[node.as_str()]].push(node.as_str());
        }

        for rank in &mut rank_nodes {
            rank.sort_by_key(|id| aug_order[id]);
        }

        for _ in 0..6 {
            for rank_idx in 1..rank_nodes.len() {
                let prev_rank_pos: HashMap<&str, usize> = rank_nodes[rank_idx - 1]
                    .iter()
                    .enumerate()
                    .map(|(i, id)| (*id, i))
                    .collect();

                rank_nodes[rank_idx].sort_by(|a, b| {
                    let bc_a = incoming
                        .get(a)
                        .and_then(|parents| barycenter(parents, &prev_rank_pos));
                    let bc_b = incoming
                        .get(b)
                        .and_then(|parents| barycenter(parents, &prev_rank_pos));

                    match (bc_a, bc_b) {
                        (Some(x), Some(y)) => x
                            .partial_cmp(&y)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| aug_order[a].cmp(&aug_order[b])),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => aug_order[a].cmp(&aug_order[b]),
                    }
                });
            }

            for rank_idx in (0..rank_nodes.len().saturating_sub(1)).rev() {
                let next_rank_pos: HashMap<&str, usize> = rank_nodes[rank_idx + 1]
                    .iter()
                    .enumerate()
                    .map(|(i, id)| (*id, i))
                    .collect();

                rank_nodes[rank_idx].sort_by(|a, b| {
                    let bc_a = outgoing
                        .get(a)
                        .and_then(|children| barycenter(children, &next_rank_pos));
                    let bc_b = outgoing
                        .get(b)
                        .and_then(|children| barycenter(children, &next_rank_pos));

                    match (bc_a, bc_b) {
                        (Some(x), Some(y)) => x
                            .partial_cmp(&y)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| aug_order[a].cmp(&aug_order[b])),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => aug_order[a].cmp(&aug_order[b]),
                    }
                });
            }
        }

        let vertical = matches!(direction, FlowDirection::TopDown | FlowDirection::BottomUp);
        let base_x = 30.0;
        let base_y = 30.0;

        if vertical {
            let rank_widths: Vec<f32> = rank_nodes
                .iter()
                .map(|rank| {
                    if rank.is_empty() {
                        0.0
                    } else {
                        rank.iter().map(|id| all_sizes[*id].0).sum::<f32>()
                            + NODE_SPACING_X * rank.len().saturating_sub(1) as f32
                    }
                })
                .collect();

            let max_rank_w = rank_widths.iter().copied().fold(0.0, f32::max);
            let mut y = base_y;

            for (rank_idx, rank) in rank_nodes.iter().enumerate() {
                let mut x = base_x + (max_rank_w - rank_widths[rank_idx]).max(0.0) / 2.0;
                let mut rank_max_h: f32 = 0.0;

                for node_id in rank {
                    let (w, h) = all_sizes[*node_id];
                    positions.insert((*node_id).to_string(), Rect::new(x, y, w, h));
                    x += w + NODE_SPACING_X;
                    rank_max_h = rank_max_h.max(h);
                }

                y += rank_max_h + NODE_SPACING_Y;
            }

            // Phase 3: Coordinate refinement
            for _ in 0..4 {
                // Forward pass
                for rank in rank_nodes.iter().skip(1) {
                    for node_id in rank {
                        if let Some(neighbors) = incoming.get(node_id) {
                            let mut centers: Vec<f32> = neighbors
                                .iter()
                                .filter_map(|n| positions.get(*n))
                                .map(|p| p.x + p.w / 2.0)
                                .collect();
                            if !centers.is_empty() {
                                centers.sort_by(|a, b| {
                                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                                });
                                let median = centers[centers.len() / 2];
                                let w = all_sizes[*node_id].0;
                                if let Some(pos) = positions.get_mut(*node_id) {
                                    pos.x = median - w / 2.0;
                                }
                            }
                        }
                    }
                    // Enforce minimum spacing
                    let mut prev_right = f32::NEG_INFINITY;
                    for node_id in rank {
                        if let Some(pos) = positions.get_mut(*node_id) {
                            pos.x = pos.x.max(prev_right + NODE_SPACING_X);
                            prev_right = pos.x + pos.w;
                        }
                    }
                }

                // Backward pass
                for rank_idx in (0..rank_nodes.len().saturating_sub(1)).rev() {
                    for node_id in &rank_nodes[rank_idx] {
                        if let Some(neighbors) = outgoing.get(node_id) {
                            let mut centers: Vec<f32> = neighbors
                                .iter()
                                .filter_map(|n| positions.get(*n))
                                .map(|p| p.x + p.w / 2.0)
                                .collect();
                            if !centers.is_empty() {
                                centers.sort_by(|a, b| {
                                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                                });
                                let median = centers[centers.len() / 2];
                                let w = all_sizes[*node_id].0;
                                if let Some(pos) = positions.get_mut(*node_id) {
                                    pos.x = median - w / 2.0;
                                }
                            }
                        }
                    }
                    let mut prev_right = f32::NEG_INFINITY;
                    for node_id in &rank_nodes[rank_idx] {
                        if let Some(pos) = positions.get_mut(*node_id) {
                            pos.x = pos.x.max(prev_right + NODE_SPACING_X);
                            prev_right = pos.x + pos.w;
                        }
                    }
                }
            }
        } else {
            let rank_heights: Vec<f32> = rank_nodes
                .iter()
                .map(|rank| {
                    if rank.is_empty() {
                        0.0
                    } else {
                        rank.iter().map(|id| all_sizes[*id].1).sum::<f32>()
                            + NODE_SPACING_Y * rank.len().saturating_sub(1) as f32
                    }
                })
                .collect();

            let max_rank_h = rank_heights.iter().copied().fold(0.0, f32::max);
            let mut x = base_x;

            for (rank_idx, rank) in rank_nodes.iter().enumerate() {
                let mut y = base_y + (max_rank_h - rank_heights[rank_idx]).max(0.0) / 2.0;
                let mut rank_max_w: f32 = 0.0;

                for node_id in rank {
                    let (w, h) = all_sizes[*node_id];
                    positions.insert((*node_id).to_string(), Rect::new(x, y, w, h));
                    y += h + NODE_SPACING_Y;
                    rank_max_w = rank_max_w.max(w);
                }

                x += rank_max_w + NODE_SPACING_X;
            }

            // Phase 3: Coordinate refinement (horizontal)
            for _ in 0..4 {
                // Forward pass
                for rank in rank_nodes.iter().skip(1) {
                    for node_id in rank {
                        if let Some(neighbors) = incoming.get(node_id) {
                            let mut centers: Vec<f32> = neighbors
                                .iter()
                                .filter_map(|n| positions.get(*n))
                                .map(|p| p.y + p.h / 2.0)
                                .collect();
                            if !centers.is_empty() {
                                centers.sort_by(|a, b| {
                                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                                });
                                let median = centers[centers.len() / 2];
                                let h = all_sizes[*node_id].1;
                                if let Some(pos) = positions.get_mut(*node_id) {
                                    pos.y = median - h / 2.0;
                                }
                            }
                        }
                    }
                    // Enforce minimum spacing
                    let mut prev_bottom = f32::NEG_INFINITY;
                    for node_id in rank {
                        if let Some(pos) = positions.get_mut(*node_id) {
                            pos.y = pos.y.max(prev_bottom + NODE_SPACING_Y);
                            prev_bottom = pos.y + pos.h;
                        }
                    }
                }

                // Backward pass
                for rank_idx in (0..rank_nodes.len().saturating_sub(1)).rev() {
                    for node_id in &rank_nodes[rank_idx] {
                        if let Some(neighbors) = outgoing.get(node_id) {
                            let mut centers: Vec<f32> = neighbors
                                .iter()
                                .filter_map(|n| positions.get(*n))
                                .map(|p| p.y + p.h / 2.0)
                                .collect();
                            if !centers.is_empty() {
                                centers.sort_by(|a, b| {
                                    a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                                });
                                let median = centers[centers.len() / 2];
                                let h = all_sizes[*node_id].1;
                                if let Some(pos) = positions.get_mut(*node_id) {
                                    pos.y = median - h / 2.0;
                                }
                            }
                        }
                    }
                    let mut prev_bottom = f32::NEG_INFINITY;
                    for node_id in &rank_nodes[rank_idx] {
                        if let Some(pos) = positions.get_mut(*node_id) {
                            pos.y = pos.y.max(prev_bottom + NODE_SPACING_Y);
                            prev_bottom = pos.y + pos.h;
                        }
                    }
                }
            }
        }

        // Extract waypoints from dummy positions
        let mut edge_waypoints: EdgeWaypoints = HashMap::new();
        for ((from, to), chain) in &edge_dummy_chains {
            let waypoints: Vec<(f32, f32)> = chain
                .iter()
                .filter_map(|dummy_id| positions.get(dummy_id))
                .map(|pos| pos.center())
                .collect();
            if !waypoints.is_empty() {
                edge_waypoints.insert((from.clone(), to.clone()), waypoints);
            }
        }

        // Remove dummy positions
        for dummy_id in &dummy_set {
            positions.remove(dummy_id);
        }

        // Normalize positions so diagram starts near origin
        Self::normalize_positions(&mut positions, &mut edge_waypoints, base_x, base_y);

        // Direction flip
        let mut bbox = Self::calculate_bbox(&positions);

        if matches!(direction, FlowDirection::BottomUp) {
            let bottom = bbox.bottom();
            for pos in positions.values_mut() {
                pos.y = bottom - (pos.y + pos.h);
            }
            for wps in edge_waypoints.values_mut() {
                for wp in wps.iter_mut() {
                    wp.1 = bottom - wp.1;
                }
            }
            bbox = Self::calculate_bbox(&positions);
        } else if matches!(direction, FlowDirection::RightLeft) {
            let right = bbox.right();
            for pos in positions.values_mut() {
                pos.x = right - (pos.x + pos.w);
            }
            for wps in edge_waypoints.values_mut() {
                for wp in wps.iter_mut() {
                    wp.0 = right - wp.0;
                }
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
