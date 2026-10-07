use super::types::*;

#[derive(Debug, Clone)]
pub enum MermaidDiagram {
    Flowchart(Flowchart),
    Sequence(SequenceDiagram),
    ClassDiagram(ClassDiagram),
    StateDiagram(StateDiagram),
    ErDiagram(ErDiagram),
}

/// Parse a mermaid diagram from source text.
///
/// Unsupported diagram types and diagrams without any nodes are errors so the
/// caller can show the source instead of an empty picture.
pub fn parse_mermaid(input: &str) -> Result<MermaidDiagram, String> {
    let input = skip_preamble(input.trim());
    let first_line = input.lines().next().unwrap_or("");
    let kind = first_line.split_whitespace().next().unwrap_or("");

    let diagram = if kind == "graph" || kind.starts_with("flowchart") {
        MermaidDiagram::Flowchart(parse_flowchart(input))
    } else if kind == "sequenceDiagram" {
        MermaidDiagram::Sequence(parse_sequence(input))
    } else if kind.starts_with("classDiagram") {
        let diagram = parse_class(input).map_err(|e| format!("Class parse error: {}", e))?;
        MermaidDiagram::ClassDiagram(diagram)
    } else if kind.starts_with("stateDiagram") {
        let diagram = parse_state(input).map_err(|e| format!("State parse error: {}", e))?;
        MermaidDiagram::StateDiagram(diagram)
    } else if kind == "erDiagram" {
        MermaidDiagram::ErDiagram(parse_er(input))
    } else {
        return Err(format!("unsupported Mermaid diagram type: {kind}"));
    };

    if diagram.is_empty() {
        return Err(format!("Mermaid {kind} diagram has nothing to draw"));
    }
    Ok(diagram)
}

/// Skip `%%` comment/directive lines and a `---` front-matter block before the
/// diagram header; every parser expects the header on its first line.
fn skip_preamble(mut input: &str) -> &str {
    if let Some(rest) = input.strip_prefix("---")
        && let Some(end) = rest.find("\n---")
    {
        input = rest[end + 4..].trim_start();
    }
    while input.starts_with("%%") {
        input = input
            .split_once('\n')
            .map_or("", |(_, rest)| rest.trim_start());
    }
    input
}

impl MermaidDiagram {
    fn is_empty(&self) -> bool {
        match self {
            Self::Flowchart(fc) => fc.nodes.is_empty(),
            Self::Sequence(seq) => seq.participants.is_empty(),
            Self::ClassDiagram(cls) => cls.classes.is_empty(),
            Self::StateDiagram(st) => st.states.is_empty(),
            Self::ErDiagram(er) => er.entities.is_empty(),
        }
    }
}

// ============================================
// FLOWCHART PARSER
// ============================================

fn parse_flowchart(input: &str) -> Flowchart {
    let mut lines = input.lines().peekable();

    // Parse direction from first line
    let first_line = lines.next().unwrap_or("");
    let direction = parse_flow_direction(first_line);

    let mut nodes: Vec<FlowchartNode> = Vec::new();
    let mut edges: Vec<FlowchartEdge> = Vec::new();
    let mut subgraphs: Vec<Subgraph> = Vec::new();
    let mut current_subgraph: Option<Subgraph> = None;
    let mut node_labels: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Skip comments
        if line.starts_with("%%") {
            continue;
        }

        // Subgraph start
        if line.starts_with("subgraph ") {
            let raw_title = line.strip_prefix("subgraph ").unwrap_or("").trim();
            let title = if let Some(bracket_pos) = raw_title.find("[\"") {
                let after = &raw_title[bracket_pos + 2..];
                if let Some(end) = after.find("\"]") {
                    after[..end].to_string()
                } else {
                    raw_title.to_string()
                }
            } else {
                raw_title.to_string()
            };
            current_subgraph = Some(Subgraph {
                title,
                nodes: Vec::new(),
            });
            continue;
        }

        // Subgraph end
        if line == "end" {
            if let Some(sg) = current_subgraph.take() {
                subgraphs.push(sg);
            }
            continue;
        }

        // Try to parse as edge or node definition
        if let Some(parsed) = parse_edge_line(line) {
            // Register nodes if not already present (with full info including label and shape)
            if !node_labels.contains_key(&parsed.from.id) {
                nodes.push(FlowchartNode {
                    id: parsed.from.id.clone(),
                    label: parsed.from.label.clone(),
                    shape: parsed.from.shape,
                });
                node_labels.insert(parsed.from.id.clone(), parsed.from.label.clone());
            }
            if !node_labels.contains_key(&parsed.to.id) {
                nodes.push(FlowchartNode {
                    id: parsed.to.id.clone(),
                    label: parsed.to.label.clone(),
                    shape: parsed.to.shape,
                });
                node_labels.insert(parsed.to.id.clone(), parsed.to.label.clone());
            }

            // Add edge endpoint nodes to current subgraph if inside one
            if let Some(ref mut sg) = current_subgraph {
                if !sg.nodes.contains(&parsed.from.id) {
                    sg.nodes.push(parsed.from.id.clone());
                }
                if !sg.nodes.contains(&parsed.to.id) {
                    sg.nodes.push(parsed.to.id.clone());
                }
            }

            edges.push(FlowchartEdge {
                from: parsed.from.id,
                to: parsed.to.id,
                label: parsed.label,
                style: parsed.style,
                arrow_head: parsed.arrow_head,
                arrow_tail: parsed.arrow_tail,
            });
        } else if let Some((id, label, shape)) = extract_shaped_node(line) {
            nodes.push(FlowchartNode {
                id: id.clone(),
                label: label.clone(),
                shape,
            });
            node_labels.insert(id.clone(), label);

            // Add to current subgraph if in one
            if let Some(ref mut sg) = current_subgraph {
                sg.nodes.push(id);
            }
        }
    }

    Flowchart {
        direction,
        nodes,
        edges,
        subgraphs,
    }
}

fn parse_flow_direction(line: &str) -> FlowDirection {
    let line = line.to_lowercase();
    if line.contains("tb") || line.contains("td") {
        FlowDirection::TopDown
    } else if line.contains("bt") {
        FlowDirection::BottomUp
    } else if line.contains("lr") {
        FlowDirection::LeftRight
    } else if line.contains("rl") {
        FlowDirection::RightLeft
    } else {
        FlowDirection::TopDown
    }
}

struct ParsedNodeInfo {
    id: String,
    label: String,
    shape: NodeShape,
}

struct ParsedEdgeLine {
    from: ParsedNodeInfo,
    to: ParsedNodeInfo,
    label: Option<String>,
    style: EdgeStyle,
    arrow_head: ArrowType,
    arrow_tail: ArrowType,
}

fn parse_edge_line(line: &str) -> Option<ParsedEdgeLine> {
    // Edge patterns (order matters - longer patterns first)
    let patterns = [
        ("<==>", EdgeStyle::Thick, ArrowType::Arrow, ArrowType::Arrow),
        ("<-->", EdgeStyle::Solid, ArrowType::Arrow, ArrowType::Arrow),
        (
            "<.->",
            EdgeStyle::Dotted,
            ArrowType::Arrow,
            ArrowType::Arrow,
        ),
        ("x==x", EdgeStyle::Thick, ArrowType::Cross, ArrowType::Cross),
        (
            "o==o",
            EdgeStyle::Thick,
            ArrowType::Circle,
            ArrowType::Circle,
        ),
        ("x--x", EdgeStyle::Solid, ArrowType::Cross, ArrowType::Cross),
        (
            "o--o",
            EdgeStyle::Solid,
            ArrowType::Circle,
            ArrowType::Circle,
        ),
        (
            "x-.x",
            EdgeStyle::Dotted,
            ArrowType::Cross,
            ArrowType::Cross,
        ),
        (
            "o-.o",
            EdgeStyle::Dotted,
            ArrowType::Circle,
            ArrowType::Circle,
        ),
        ("<==", EdgeStyle::Thick, ArrowType::None, ArrowType::Arrow),
        ("<--", EdgeStyle::Solid, ArrowType::None, ArrowType::Arrow),
        ("<-.", EdgeStyle::Dotted, ArrowType::None, ArrowType::Arrow),
        (
            "o-->",
            EdgeStyle::Solid,
            ArrowType::Arrow,
            ArrowType::Circle,
        ),
        (
            "--o>",
            EdgeStyle::Solid,
            ArrowType::Arrow,
            ArrowType::Circle,
        ),
        (
            "o==>",
            EdgeStyle::Thick,
            ArrowType::Arrow,
            ArrowType::Circle,
        ),
        (
            "o-.->",
            EdgeStyle::Dotted,
            ArrowType::Arrow,
            ArrowType::Circle,
        ),
        ("==o", EdgeStyle::Thick, ArrowType::Circle, ArrowType::None),
        ("==x", EdgeStyle::Thick, ArrowType::Cross, ArrowType::None),
        ("o==", EdgeStyle::Thick, ArrowType::None, ArrowType::Circle),
        ("x==", EdgeStyle::Thick, ArrowType::None, ArrowType::Cross),
        ("==>", EdgeStyle::Thick, ArrowType::Arrow, ArrowType::None),
        ("--x", EdgeStyle::Solid, ArrowType::Cross, ArrowType::None),
        ("x--", EdgeStyle::Solid, ArrowType::None, ArrowType::Cross),
        ("--o", EdgeStyle::Solid, ArrowType::Circle, ArrowType::None),
        ("o--", EdgeStyle::Solid, ArrowType::None, ArrowType::Circle),
        ("-.->", EdgeStyle::Dotted, ArrowType::Arrow, ArrowType::None),
        ("-.x", EdgeStyle::Dotted, ArrowType::Cross, ArrowType::None),
        ("x-.", EdgeStyle::Dotted, ArrowType::None, ArrowType::Cross),
        ("-.o", EdgeStyle::Dotted, ArrowType::Circle, ArrowType::None),
        ("o-.", EdgeStyle::Dotted, ArrowType::None, ArrowType::Circle),
        ("-.-", EdgeStyle::Dotted, ArrowType::None, ArrowType::None),
        ("-->", EdgeStyle::Solid, ArrowType::Arrow, ArrowType::None),
        ("---", EdgeStyle::Solid, ArrowType::None, ArrowType::None),
        ("->>", EdgeStyle::Solid, ArrowType::Arrow, ArrowType::Arrow),
        ("->", EdgeStyle::Solid, ArrowType::Arrow, ArrowType::None),
        ("--", EdgeStyle::Solid, ArrowType::None, ArrowType::None),
    ];

    for (pattern, style, head, tail) in &patterns {
        if let Some(pos) = line.find(pattern) {
            let from_part = line[..pos].trim();
            let rest = &line[pos + pattern.len()..];

            // Parse optional label
            let (to_part, label) = if let Some(stripped) = rest.strip_prefix('|') {
                // Label before target: A -->|label| B
                if let Some(end_label) = stripped.find('|') {
                    let label_text = stripped[..end_label].trim();
                    let after_label = stripped[end_label + 1..].trim();
                    (after_label, Some(label_text.to_string()))
                } else {
                    (rest.trim(), None)
                }
            } else {
                // No label, just target
                (rest.trim(), None)
            };

            // Extract full node info (id, label, shape)
            let from_info = extract_node_info(from_part)?;
            let to_info = extract_node_info(to_part)?;

            return Some(ParsedEdgeLine {
                from: ParsedNodeInfo {
                    id: from_info.0,
                    label: from_info.1,
                    shape: from_info.2,
                },
                to: ParsedNodeInfo {
                    id: to_info.0,
                    label: to_info.1,
                    shape: to_info.2,
                },
                label,
                style: style.clone(),
                arrow_head: head.clone(),
                arrow_tail: tail.clone(),
            });
        }
    }

    None
}

fn extract_node_id(part: &str) -> Option<String> {
    let part = part.trim();

    // An unterminated shape like `A[Label` still names node `A`.
    if let Some(pos) = part.find(['[', '(', '{', '<']) {
        return Some(part[..pos].trim().to_string());
    }

    // Simple node id - alphanumeric and underscores
    let id: String = part
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();

    if id.is_empty() { None } else { Some(id) }
}

/// Extract node ID, label, and shape from a part of an edge definition
fn extract_node_info(part: &str) -> Option<(String, String, NodeShape)> {
    if let Some(shaped) = extract_shaped_node(part) {
        return Some(shaped);
    }

    // Simple node - just an ID
    let id = extract_node_id(part)?;
    Some((id.clone(), id, NodeShape::RoundedRect))
}

/// Parse a node written with an explicit shape, such as `id[Label]` or `id{Label}`.
fn extract_shaped_node(part: &str) -> Option<(String, String, NodeShape)> {
    let part = part.trim();

    // Order matters: check longer patterns first
    let patterns: &[(&str, &str, NodeShape)] = &[
        ("(((", ")))", NodeShape::DoubleCircle),
        ("[[", "]]", NodeShape::Subroutine),
        ("((", "))", NodeShape::Circle),
        ("[(", ")]", NodeShape::Cylinder),
        ("([", "])", NodeShape::Stadium),
        ("[/", "/]", NodeShape::Parallelogram),
        ("[\\", "\\]", NodeShape::ParallelogramAlt),
        ("[/", "\\]", NodeShape::Trapezoid),
        ("[\\", "/]", NodeShape::TrapezoidAlt),
        ("{{", "}}", NodeShape::Hexagon),
        ("[", "]", NodeShape::Rect),
        ("(", ")", NodeShape::RoundedRect),
        ("{", "}", NodeShape::Rhombus),
    ];

    for (open, close, shape) in patterns {
        if let Some(pos) = part.find(open) {
            let after_open = &part[pos + open.len()..];
            if let Some(end_pos) = after_open.find(close) {
                let id = part[..pos].trim().to_string();
                let label = normalize_flowchart_label(&after_open[..end_pos]);

                if !id.is_empty() {
                    return Some((id, label, shape.clone()));
                }
            }
        }
    }

    None
}

fn normalize_flowchart_label(raw: &str) -> String {
    let mut label = raw.trim().to_string();

    // Mermaid line-break tags should become actual newlines before layout/measurement.
    label = label
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n");

    // Labels like A["Text"] should render as Text, not "Text".
    if let Some(inner) = label
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        label = inner.trim().to_string();
    }

    label
}

// ============================================
// SEQUENCE DIAGRAM PARSER
// ============================================

fn parse_sequence(input: &str) -> SequenceDiagram {
    let lines: Vec<String> = input
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("%%"))
        .map(str::to_string)
        .collect();

    // Mermaid orders participants by first appearance, whether declared with
    // `participant`/`actor` or implied by a message endpoint.
    let mut participants: Vec<Participant> = Vec::new();
    for line in &lines {
        let declaration = line
            .strip_prefix("participant ")
            .or_else(|| line.strip_prefix("actor "));
        if let Some(rest) = declaration {
            let (id, alias) = match rest.split_once(" as ") {
                Some((id, alias)) => (id.trim(), Some(alias.trim().to_string())),
                None => (rest.trim(), None),
            };
            match participants.iter_mut().find(|p| p.id == id) {
                Some(existing) => existing.alias = alias.or(existing.alias.take()),
                None => participants.push(Participant {
                    id: id.to_string(),
                    alias,
                }),
            }
        } else if parse_sequence_note(line).is_none()
            && let Some((msg, _)) = parse_sequence_message(line)
        {
            for id in [msg.from, msg.to] {
                if !participants.iter().any(|p| p.id == id) {
                    participants.push(Participant { id, alias: None });
                }
            }
        }
    }

    let mut index = 0;
    let elements = parse_sequence_elements(&lines, &mut index);

    SequenceDiagram {
        participants,
        elements,
    }
}

fn parse_sequence_elements(lines: &[String], index: &mut usize) -> Vec<SequenceElement> {
    let mut elements = Vec::new();

    while *index < lines.len() {
        let line = lines[*index].trim();

        if line == "end" || line == "else" || line.starts_with("else ") {
            break;
        }

        if line.starts_with("participant ") || line.starts_with("actor ") {
            *index += 1;
            continue;
        }

        // Notes first: their text may contain arrows such as `a->b`.
        if let Some(note) = parse_sequence_note(line) {
            elements.push(note);
            *index += 1;
            continue;
        }

        if let Some((msg, activation)) = parse_sequence_message(line) {
            elements.push(SequenceElement::Message(msg));
            elements.extend(activation);
            *index += 1;
            continue;
        }

        if let Some(bt) = parse_sequence_block_type(line) {
            let rest = line.split_once(' ').map(|(_, r)| r).unwrap_or("");
            let label = if let Some(colon_pos) = rest.find(':') {
                rest[colon_pos + 1..].trim().to_string()
            } else {
                rest.trim().to_string()
            };

            *index += 1;
            let messages = parse_sequence_elements(lines, index);

            let mut else_branches = Vec::new();
            while *index < lines.len() {
                let branch_line = lines[*index].trim();
                if branch_line == "else" || branch_line.starts_with("else ") {
                    let branch_label = branch_line
                        .strip_prefix("else")
                        .map(str::trim)
                        .unwrap_or("")
                        .to_string();
                    *index += 1;
                    let branch_messages = parse_sequence_elements(lines, index);
                    else_branches.push((branch_label, branch_messages));
                } else {
                    break;
                }
            }

            if *index < lines.len() && lines[*index].trim() == "end" {
                *index += 1;
            }

            elements.push(SequenceElement::Block(SequenceBlock {
                block_type: bt,
                label,
                messages,
                else_branches,
            }));
            continue;
        }

        if line.starts_with("activate ") {
            let participant = line.strip_prefix("activate ").unwrap_or("").trim();
            elements.push(SequenceElement::Activation(Activation {
                participant: participant.to_string(),
            }));
            *index += 1;
            continue;
        }

        if line.starts_with("deactivate ") {
            let participant = line.strip_prefix("deactivate ").unwrap_or("").trim();
            elements.push(SequenceElement::Deactivation(Activation {
                participant: participant.to_string(),
            }));
            *index += 1;
            continue;
        }

        *index += 1;
    }

    elements
}

fn parse_sequence_block_type(line: &str) -> Option<SequenceBlockType> {
    if line.starts_with("alt ") {
        Some(SequenceBlockType::Alt)
    } else if line.starts_with("opt ") {
        Some(SequenceBlockType::Opt)
    } else if line.starts_with("loop ") {
        Some(SequenceBlockType::Loop)
    } else if line.starts_with("par ") {
        Some(SequenceBlockType::Par)
    } else if line.starts_with("critical ") {
        Some(SequenceBlockType::Critical)
    } else {
        None
    }
}

fn parse_sequence_note(line: &str) -> Option<SequenceElement> {
    if !line.starts_with("Note ") {
        return None;
    }

    let rest = line.strip_prefix("Note ").unwrap_or("").trim();

    let (position, after_prefix) = if let Some(after) = rest.strip_prefix("right of ") {
        ("right", after)
    } else if let Some(after) = rest.strip_prefix("left of ") {
        ("left", after)
    } else {
        let after = rest.strip_prefix("over ")?;
        ("over", after)
    };

    let colon_pos = after_prefix.find(':')?;
    let participant = after_prefix[..colon_pos].trim();
    let text = after_prefix[colon_pos + 1..].trim();
    if participant.is_empty() {
        return None;
    }

    Some(SequenceElement::Note {
        participant: participant.to_string(),
        position: position.to_string(),
        text: text.to_string(),
    })
}

/// Parse `From->>To: Label`, plus the activation change that a `+`/`-` prefix on
/// the receiver requests (`+` activates the receiver, `-` deactivates the sender).
fn parse_sequence_message(line: &str) -> Option<(SequenceMessage, Option<SequenceElement>)> {
    // Order matters: longer patterns first to avoid partial matches
    let patterns = [
        ("-->>", MessageType::Dotted, MessageKind::Reply),
        ("->>", MessageType::Solid, MessageKind::Sync),
        ("--x", MessageType::Dotted, MessageKind::Sync),
        ("--)", MessageType::Dotted, MessageKind::Async),
        ("-->", MessageType::Dotted, MessageKind::Sync),
        ("->", MessageType::Solid, MessageKind::Sync),
        ("-x", MessageType::Solid, MessageKind::Sync),
        ("-)", MessageType::Solid, MessageKind::Async),
    ];

    for (pattern, msg_type, kind) in &patterns {
        if let Some(pos) = line.find(pattern) {
            let from = line[..pos].trim().to_string();
            let rest = &line[pos + pattern.len()..];

            // Parse "To: Label" or just "To"
            let (to, label) = match rest.split_once(':') {
                Some((to, label)) => (to.trim(), label.trim().to_string()),
                None => (rest.trim(), String::new()),
            };

            let (to, activation) = if let Some(to) = to.strip_prefix('+') {
                let to = to.trim().to_string();
                let activation = SequenceElement::Activation(Activation {
                    participant: to.clone(),
                });
                (to, Some(activation))
            } else if let Some(to) = to.strip_prefix('-') {
                let deactivation = SequenceElement::Deactivation(Activation {
                    participant: from.clone(),
                });
                (to.trim().to_string(), Some(deactivation))
            } else {
                (to.to_string(), None)
            };

            let message = SequenceMessage {
                from,
                to,
                label,
                msg_type: msg_type.clone(),
                kind: kind.clone(),
            };
            return Some((message, activation));
        }
    }

    None
}

// ============================================
// CLASS DIAGRAM PARSER
// ============================================

fn parse_class(input: &str) -> Result<ClassDiagram, String> {
    let mut lines = input.lines().skip(1); // Skip "classDiagram"

    let mut classes: Vec<ClassDefinition> = Vec::new();
    let mut relations: Vec<ClassRelation> = Vec::new();
    let mut current_class: Option<ClassDefinition> = None;

    for (line_idx, line) in (&mut lines).enumerate() {
        let line_num = line_idx + 2; // +2: skip first line + 1-indexed
        let line = line.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }

        // Class definition end
        if line == "}" {
            if let Some(cls) = current_class.take() {
                classes.push(cls);
            }
            continue;
        }

        // Class definition start
        if line.starts_with("class ") {
            let rest = line.strip_prefix("class ").unwrap_or("");

            // Check for stereotype
            let (name, stereotype) = if rest.starts_with("<<") {
                let end = rest
                    .find(">>")
                    .ok_or_else(|| format!("line {}: missing '>>' in stereotype", line_num))?;
                let st = &rest[2..end];
                let after = rest[end + 2..].trim();
                (after.to_string(), Some(st.trim().to_string()))
            } else {
                (rest.trim().to_string(), None)
            };

            // Check if it's a one-liner or has body
            if let Some(name) = name.strip_suffix('{') {
                current_class = Some(empty_class(name.trim().to_string(), stereotype));
            } else {
                classes.push(empty_class(name, stereotype));
            }
            continue;
        }

        // If inside a class body
        if let Some(ref mut cls) = current_class {
            // Stereotype annotation inside body (e.g. <<abstract>>, <<interface>>)
            if line.starts_with("<<") {
                if let Some(end) = line.find(">>") {
                    let stereo = line[2..end].trim().to_ascii_lowercase();
                    cls.stereotype = Some(line[2..end].trim().to_string());
                    if stereo == "abstract" {
                        cls.is_abstract = true;
                    } else if stereo == "interface" {
                        cls.is_interface = true;
                    }
                }
                continue;
            }

            // Attribute or method
            if let Some(vis) = line.chars().next().and_then(Visibility::from_symbol) {
                let member = line[1..].trim();

                // Check if method (has parentheses)
                if member.contains('(') {
                    if let Some(method) = parse_class_method(vis, member) {
                        cls.methods.push(method);
                    }
                } else {
                    cls.attributes.push(parse_class_attribute(vis, member));
                }
            }
            continue;
        }

        // Relation
        if let Some(rel) = parse_class_relation(line) {
            relations.push(rel);
        }
    }

    // Relations may name classes that are never declared with `class`.
    for rel in &relations {
        for name in [&rel.from, &rel.to] {
            if !classes.iter().any(|c| &c.name == name) {
                classes.push(empty_class(name.clone(), None));
            }
        }
    }

    Ok(ClassDiagram { classes, relations })
}

fn empty_class(name: String, stereotype: Option<String>) -> ClassDefinition {
    ClassDefinition {
        name,
        stereotype,
        attributes: Vec::new(),
        methods: Vec::new(),
        is_abstract: false,
        is_interface: false,
    }
}

fn parse_class_attribute(vis: Visibility, member: &str) -> ClassAttribute {
    let (name, type_annotation) = match member.split_once(':') {
        Some((name, ty)) => (name, Some(ty.trim().to_string())),
        None => (member, None),
    };

    ClassAttribute {
        member: ClassMember {
            visibility: vis,
            name: name.trim().to_string(),
        },
        type_annotation,
    }
}

fn parse_class_method(vis: Visibility, member: &str) -> Option<ClassMethod> {
    let paren_pos = member.find('(')?;
    let name = member[..paren_pos].trim();

    let close_paren = member.find(')')?;
    let params_str = &member[paren_pos + 1..close_paren];

    let return_type = if close_paren + 1 < member.len() {
        let after = member[close_paren + 1..].trim();
        after
            .strip_prefix(':')
            .map(|stripped| stripped.trim().to_string())
    } else {
        None
    };

    let parameters = if params_str.is_empty() {
        Vec::new()
    } else {
        params_str
            .split(',')
            .map(|p| match p.split_once(':') {
                Some((name, ty)) => (name.trim().to_string(), Some(ty.trim().to_string())),
                None => (p.trim().to_string(), None),
            })
            .collect()
    };

    Some(ClassMethod {
        member: ClassMember {
            visibility: vis,
            name: name.to_string(),
        },
        parameters,
        return_type,
    })
}

/// Parse `A "1" <|--* "many" B : label`: optional quoted cardinalities around
/// a `--` or `..` line with an optional marker at either end.
fn parse_class_relation(line: &str) -> Option<ClassRelation> {
    let (body, label) = match find_unquoted(line, ":") {
        Some(pos) => (&line[..pos], Some(line[pos + 1..].trim())),
        None => (line, None),
    };
    let link = [find_unquoted(body, "--"), find_unquoted(body, "..")]
        .into_iter()
        .flatten()
        .min()?;
    let (left, from_marker) = strip_left_marker(body[..link].trim_end());
    let (right, to_marker) = strip_right_marker(body[link + 2..].trim_start());
    let (from, from_cardinality) = split_trailing_quoted(left.trim_end());
    let (to_cardinality, to) = split_leading_quoted(right.trim_start());
    let to = to.trim_end();
    if !is_class_name(from) || !is_class_name(to) {
        return None;
    }

    Some(ClassRelation {
        from: from.to_string(),
        to: to.to_string(),
        from_marker,
        to_marker,
        dashed: body[link..].starts_with(".."),
        from_cardinality,
        to_cardinality,
        label: label
            .map(|l| l.trim_matches('"').trim().to_string())
            .filter(|l| !l.is_empty()),
    })
}

fn strip_left_marker(s: &str) -> (&str, ClassMarker) {
    let markers = [
        ("<|", ClassMarker::Triangle),
        ("<", ClassMarker::Arrow),
        ("*", ClassMarker::FilledDiamond),
        ("()", ClassMarker::Lollipop),
    ];
    for (token, marker) in markers {
        if let Some(rest) = s.strip_suffix(token) {
            return (rest, marker);
        }
    }
    // `o` is a marker only when it stands apart: `Foo--Bar` links class `Foo`.
    match s.strip_suffix('o') {
        Some(rest) if rest.is_empty() || rest.ends_with([' ', '\t', '"']) => {
            (rest, ClassMarker::HollowDiamond)
        }
        _ => (s, ClassMarker::None),
    }
}

fn strip_right_marker(s: &str) -> (&str, ClassMarker) {
    let markers = [
        ("|>", ClassMarker::Triangle),
        (">", ClassMarker::Arrow),
        ("*", ClassMarker::FilledDiamond),
        ("()", ClassMarker::Lollipop),
    ];
    for (token, marker) in markers {
        if let Some(rest) = s.strip_prefix(token) {
            return (rest, marker);
        }
    }
    match s.strip_prefix('o') {
        Some(rest) if rest.starts_with([' ', '\t', '"']) => (rest, ClassMarker::HollowDiamond),
        _ => (s, ClassMarker::None),
    }
}

/// Split `Name "card"` into the name and the quoted cardinality.
fn split_trailing_quoted(s: &str) -> (&str, Option<String>) {
    if let Some(inner) = s.strip_suffix('"')
        && let Some(open) = inner.rfind('"')
    {
        return (
            inner[..open].trim_end(),
            Some(inner[open + 1..].to_string()),
        );
    }
    (s, None)
}

/// Split `"card" Name` into the quoted cardinality and the name.
fn split_leading_quoted(s: &str) -> (Option<String>, &str) {
    if let Some(inner) = s.strip_prefix('"')
        && let Some(close) = inner.find('"')
    {
        return (
            Some(inner[..close].to_string()),
            inner[close + 1..].trim_start(),
        );
    }
    (None, s)
}

fn is_class_name(s: &str) -> bool {
    !s.is_empty()
        && !s
            .contains(|c: char| c.is_whitespace() || matches!(c, '"' | '<' | '>' | '|' | '(' | ')'))
}

/// Byte offset of the first `needle` outside double quotes.
fn find_unquoted(s: &str, needle: &str) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in s.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && s[i..].starts_with(needle) {
            return Some(i);
        }
    }
    None
}

// ============================================
// STATE DIAGRAM PARSER
// ============================================

fn parse_state(input: &str) -> Result<StateDiagram, String> {
    const START_STATE_ID: &str = "__start__";
    const END_STATE_ID: &str = "__end__";

    let mut lines = input.lines().skip(1); // Skip "stateDiagram"

    let mut states: Vec<State> = Vec::new();
    let mut transitions: Vec<StateTransition> = Vec::new();
    let mut composite_stack: Vec<String> = Vec::new();

    for (line_idx, line) in (&mut lines).enumerate() {
        let line_num = line_idx + 2; // +2: skip first line + 1-indexed
        let line = line.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }

        if line == "}" {
            composite_stack.pop();
            continue;
        }

        if let Some((state_id, text)) = parse_state_note(line) {
            add_state_note(&mut states, &state_id, text);
            continue;
        }

        // State definition
        if line.starts_with("state ") {
            let rest = line.strip_prefix("state ").unwrap_or("");

            let (id, label, is_composite) =
                parse_state_definition(rest).map_err(|e| format!("line {}: {}", line_num, e))?;
            ensure_state(&mut states, &id, &label, false, false, is_composite);

            if let Some(parent_id) = composite_stack.last() {
                add_state_child_state(&mut states, parent_id, &id);
            }

            if is_composite {
                composite_stack.push(id);
            }
            continue;
        }

        // Transition
        if line.contains("-->") || line.contains("->") {
            let arrow = if line.contains("-->") { "-->" } else { "->" };
            if let Some(pos) = line.find(arrow) {
                let from = line[..pos].trim().to_string();
                let rest = &line[pos + arrow.len()..];

                let (to, label) = if let Some(colon_pos) = rest.find(':') {
                    (
                        rest[..colon_pos].trim().to_string(),
                        Some(rest[colon_pos + 1..].trim().to_string()),
                    )
                } else {
                    (rest.trim().to_string(), None)
                };

                // Each composite owns its `[*]` pseudo-states, as in Mermaid.
                let scoped = |base: &str| match composite_stack.last() {
                    Some(parent) => format!("{base}{parent}"),
                    None => base.to_string(),
                };
                let normalized_from = if from == "[*]" {
                    scoped(START_STATE_ID)
                } else {
                    from.clone()
                };
                let normalized_to = if to == "[*]" {
                    scoped(END_STATE_ID)
                } else {
                    to.clone()
                };

                // Add end state if needed
                if to == "[*]" {
                    ensure_state(&mut states, &normalized_to, "[*]", false, true, false);
                }

                // Add states from transition if not already present
                if from != "[*]" {
                    ensure_state(&mut states, &normalized_from, "", false, false, false);
                } else {
                    ensure_state(&mut states, &normalized_from, "[*]", true, false, false);
                }
                if to != "[*]" {
                    ensure_state(&mut states, &normalized_to, "", false, false, false);
                }

                let transition = StateTransition {
                    from: normalized_from,
                    to: normalized_to,
                    label,
                };

                if let Some(parent_id) = composite_stack.last() {
                    add_state_child_state(&mut states, parent_id, &transition.from);
                    add_state_child_state(&mut states, parent_id, &transition.to);
                    add_state_child_transition(&mut states, parent_id, &transition);
                }

                if composite_stack.is_empty() {
                    transitions.push(transition);
                }
            }
        }
    }

    Ok(StateDiagram {
        states,
        transitions,
    })
}

fn parse_state_definition(rest: &str) -> Result<(String, String, bool), String> {
    let mut value = rest.trim();
    let mut is_composite = false;

    if value.ends_with('{') {
        is_composite = true;
        value = value[..value.len() - 1].trim();
    }

    if value.is_empty() {
        return Err("Missing state definition".to_string());
    }

    if let Some((left, right)) = value.split_once(" as ") {
        let id = right.trim();
        if id.is_empty() {
            return Err("Missing state alias identifier".to_string());
        }
        let label = left.trim().trim_matches('"');
        return Ok((id.to_string(), label.to_string(), is_composite));
    }

    if let Some(label_start) = value.find('"') {
        let label_end = value[label_start + 1..]
            .find('"')
            .ok_or("Missing closing quote in state label")?
            + label_start
            + 1;
        let id = value[..label_start].trim();
        if id.is_empty() {
            return Err("Missing state identifier".to_string());
        }
        let label = value[label_start + 1..label_end].to_string();
        return Ok((id.to_string(), label, is_composite));
    }

    // A bare id names the state without labeling it.
    Ok((value.to_string(), String::new(), is_composite))
}

fn parse_state_note(line: &str) -> Option<(String, String)> {
    let lower = line.to_ascii_lowercase();
    let prefixes = ["note right of ", "note left of ", "note over "];

    for prefix in prefixes {
        if lower.starts_with(prefix) {
            let after = line[prefix.len()..].trim();
            let colon_pos = after.find(':')?;
            let state_id = after[..colon_pos].trim();
            let text = after[colon_pos + 1..].trim();
            if state_id.is_empty() {
                return None;
            }
            return Some((state_id.to_string(), text.to_string()));
        }
    }

    None
}

/// Adds the state `id` or merges flags into it. An empty `label` keeps the
/// existing label, or labels a new state with its id.
fn ensure_state(
    states: &mut Vec<State>,
    id: &str,
    label: &str,
    is_start: bool,
    is_end: bool,
    is_composite: bool,
) {
    if let Some(state) = states.iter_mut().find(|state| state.id == id) {
        if !label.is_empty() {
            state.label = label.to_string();
        }
        state.is_start |= is_start;
        state.is_end |= is_end;
        state.is_composite |= is_composite;
        return;
    }

    states.push(State {
        id: id.to_string(),
        label: if label.is_empty() {
            id.to_string()
        } else {
            label.to_string()
        },
        is_start,
        is_end,
        is_composite,
        children: Vec::new(),
    });
}

fn add_state_child_state(states: &mut [State], parent_id: &str, child_id: &str) {
    // Nesting a state inside its own descendant would make layout recurse forever.
    if parent_id == child_id || is_descendant(states, child_id, parent_id) {
        return;
    }

    if let Some(parent) = states.iter_mut().find(|state| state.id == parent_id) {
        parent.is_composite = true;
        if !parent.child_state_ids().any(|id| id == child_id) {
            parent
                .children
                .push(StateElement::State(child_id.to_string()));
        }
    }
}

/// Whether `id` is nested, at any depth, inside `ancestor_id`.
fn is_descendant(states: &[State], ancestor_id: &str, id: &str) -> bool {
    let Some(ancestor) = states.iter().find(|state| state.id == ancestor_id) else {
        return false;
    };
    ancestor
        .child_state_ids()
        .any(|child| child == id || is_descendant(states, child, id))
}

fn add_state_child_transition(states: &mut [State], parent_id: &str, transition: &StateTransition) {
    if let Some(parent) = states.iter_mut().find(|state| state.id == parent_id) {
        parent.is_composite = true;
        let has_transition = parent.children.iter().any(|element| {
            matches!(
                element,
                StateElement::Transition(existing)
                    if existing.from == transition.from
                        && existing.to == transition.to
                        && existing.label == transition.label
            )
        });
        if !has_transition {
            parent
                .children
                .push(StateElement::Transition(transition.clone()));
        }
    }
}

fn add_state_note(states: &mut Vec<State>, state_id: &str, text: String) {
    ensure_state(states, state_id, state_id, false, false, false);
    let target_state = states
        .iter_mut()
        .find(|s| s.id == state_id)
        .expect("ensure_state just added the state");
    let has_note = target_state.children.iter().any(|element| {
        matches!(
            element,
            StateElement::Note {
                state,
                text: existing_text,
            } if state == state_id && existing_text == &text
        )
    });
    if !has_note {
        target_state.children.push(StateElement::Note {
            state: state_id.to_string(),
            text,
        });
    }
}

// ============================================
// ER DIAGRAM PARSER
// ============================================

fn parse_er(input: &str) -> ErDiagram {
    let mut lines = input.lines().skip(1); // Skip "erDiagram"

    let mut entities: Vec<ErEntity> = Vec::new();
    let mut relationships: Vec<ErRelationship> = Vec::new();
    let mut current_entity: Option<String> = None;
    let mut current_attributes: Vec<ErAttribute> = Vec::new();

    for raw_line in &mut lines {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with("%%") {
            continue;
        }

        // Inside an entity block
        if current_entity.is_some() {
            if trimmed == "}" {
                let entity_name = current_entity.take().unwrap();
                entities.push(ErEntity {
                    name: entity_name,
                    attributes: std::mem::take(&mut current_attributes),
                });
                continue;
            }

            let is_indented = raw_line
                .chars()
                .next()
                .map(|c| c.is_whitespace())
                .unwrap_or(false);

            if is_indented {
                let attr_line = trimmed;
                let is_key = attr_line.starts_with('*');
                let name = if is_key {
                    attr_line[1..].trim()
                } else {
                    attr_line
                };

                current_attributes.push(ErAttribute {
                    name: name.to_string(),
                    is_key,
                });
                continue;
            }

            // Unexpected unindented line while inside a block: close the block and
            // re-process this line as a top-level statement.
            let entity_name = current_entity.take().unwrap();
            entities.push(ErEntity {
                name: entity_name,
                attributes: std::mem::take(&mut current_attributes),
            });
            // fallthrough
        }

        if trimmed == "}" {
            continue;
        }

        // Relationship
        if let Some(rel) = parse_er_relationship(trimmed) {
            relationships.push(rel);
            continue;
        }

        // Entity definition (block)
        if trimmed.contains('{') {
            let name = trimmed.trim_end_matches('{').trim().to_string();
            current_entity = Some(name);
            current_attributes.clear();
            continue;
        }

        // Simple entity declaration
        entities.push(ErEntity {
            name: trimmed.to_string(),
            attributes: Vec::new(),
        });
    }

    // Save last entity
    if let Some(entity_name) = current_entity {
        entities.push(ErEntity {
            name: entity_name,
            attributes: current_attributes,
        });
    }

    // Auto-create entities referenced in relationships but not explicitly declared
    let mut existing: std::collections::HashSet<&str> =
        entities.iter().map(|e| e.name.as_str()).collect();
    let mut new_entities = Vec::new();
    for rel in &relationships {
        for name in [&rel.from, &rel.to] {
            if existing.insert(name.as_str()) {
                new_entities.push(ErEntity {
                    name: name.clone(),
                    attributes: Vec::new(),
                });
            }
        }
    }
    entities.extend(new_entities);

    ErDiagram {
        entities,
        relationships,
    }
}

fn parse_er_relationship(line: &str) -> Option<ErRelationship> {
    // A relationship is `<left cardinality><-- or ..><right cardinality>`, e.g. `||--o{`.
    let (pos, from_cardinality, to_cardinality) = line
        .match_indices("--")
        .chain(line.match_indices(".."))
        .find_map(|(pos, _)| {
            let left = er_cardinality(line.get(pos.checked_sub(2)?..pos)?)?;
            let right = er_cardinality(line.get(pos + 2..pos + 4)?)?;
            Some((pos, left, right))
        })?;

    let from = line[..pos - 2].trim().to_string();
    let rest = line[pos + 4..].trim();

    let (to, label) = if let Some((to_part, label_part)) = rest.split_once(':') {
        let to = to_part.trim().to_string();
        let label = label_part.trim().trim_matches('"').trim().to_string();
        let label = if label.is_empty() { None } else { Some(label) };
        (to, label)
    } else if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        (stripped[..end].to_string(), None)
    } else {
        (rest.to_string(), None)
    };

    Some(ErRelationship {
        from,
        to,
        from_cardinality,
        to_cardinality,
        label,
    })
}

/// Crow's-foot markers; like Mermaid's lexer, either orientation is accepted on both sides.
fn er_cardinality(marker: &str) -> Option<ErCardinality> {
    match marker {
        "||" => Some(ErCardinality::ExactlyOne),
        "|o" | "o|" => Some(ErCardinality::ZeroOrOne),
        "}o" | "o{" => Some(ErCardinality::ZeroOrMore),
        "}|" | "|{" => Some(ErCardinality::OneOrMore),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flowchart(src: &str) -> Flowchart {
        match parse_mermaid(src).unwrap() {
            MermaidDiagram::Flowchart(fc) => fc,
            other => panic!("expected flowchart, got {other:?}"),
        }
    }

    fn sequence(src: &str) -> SequenceDiagram {
        match parse_mermaid(src).unwrap() {
            MermaidDiagram::Sequence(seq) => seq,
            other => panic!("expected sequence diagram, got {other:?}"),
        }
    }

    fn class(src: &str) -> ClassDiagram {
        match parse_mermaid(src).unwrap() {
            MermaidDiagram::ClassDiagram(cls) => cls,
            other => panic!("expected class diagram, got {other:?}"),
        }
    }

    fn state(src: &str) -> StateDiagram {
        match parse_mermaid(src).unwrap() {
            MermaidDiagram::StateDiagram(st) => st,
            other => panic!("expected state diagram, got {other:?}"),
        }
    }

    fn er(src: &str) -> ErDiagram {
        match parse_mermaid(src).unwrap() {
            MermaidDiagram::ErDiagram(er) => er,
            other => panic!("expected ER diagram, got {other:?}"),
        }
    }

    fn edge<'a>(fc: &'a Flowchart, from: &str, to: &str) -> &'a FlowchartEdge {
        fc.edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .unwrap_or_else(|| panic!("missing edge {from} -> {to}"))
    }

    fn messages(elements: &[SequenceElement]) -> Vec<&SequenceMessage> {
        elements
            .iter()
            .filter_map(|e| match e {
                SequenceElement::Message(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    fn participant_ids(seq: &SequenceDiagram) -> Vec<&str> {
        seq.participants.iter().map(|p| p.id.as_str()).collect()
    }

    #[test]
    fn test_parse_simple_flowchart() {
        let fc = flowchart("\nflowchart TD\n    A --> B\n");
        assert_eq!(fc.nodes.len(), 2);
        assert_eq!(fc.edges.len(), 1);
        assert_eq!(fc.nodes[0].id, "A");
        assert_eq!(fc.nodes[0].label, "A");
        assert_eq!(fc.nodes[1].id, "B");
        assert_eq!(fc.edges[0].from, "A");
        assert_eq!(fc.edges[0].to, "B");
    }

    #[test]
    fn test_parse_flowchart_with_labels() {
        let fc = flowchart(
            r#"
flowchart TD
    A[Start] --> B{Decision?}
    B -->|Yes| C[Continue]
    B -->|No| D[Stop]
"#,
        );
        assert_eq!(fc.nodes.len(), 4);

        let node = |id: &str| fc.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(node("A").label, "Start");
        assert_eq!(node("A").shape, NodeShape::Rect);
        assert_eq!(node("B").label, "Decision?");
        assert_eq!(node("B").shape, NodeShape::Rhombus);
        assert_eq!(node("C").label, "Continue");
        assert_eq!(node("D").label, "Stop");

        assert_eq!(edge(&fc, "B", "C").label.as_deref(), Some("Yes"));
        assert_eq!(edge(&fc, "B", "D").label.as_deref(), Some("No"));
    }

    #[test]
    fn test_parse_flowchart_shapes() {
        let fc = flowchart(
            r#"
flowchart LR
    A([Stadium]) --> B[[Subroutine]]
    B --> C[(Database)]
    C --> D((Circle))
    D --> E{Diamond}
    F[/Lean/]
"#,
        );
        let expected = [
            ("A", "Stadium", NodeShape::Stadium),
            ("B", "Subroutine", NodeShape::Subroutine),
            ("C", "Database", NodeShape::Cylinder),
            ("D", "Circle", NodeShape::Circle),
            ("E", "Diamond", NodeShape::Rhombus),
            ("F", "Lean", NodeShape::Parallelogram),
        ];
        assert_eq!(fc.nodes.len(), expected.len());
        for (id, label, shape) in expected {
            let node = fc.nodes.iter().find(|n| n.id == id).unwrap();
            assert_eq!(node.label, label, "label of {id}");
            assert_eq!(node.shape, shape, "shape of {id}");
        }
    }

    #[test]
    fn test_parse_flowchart_normalizes_quoted_labels() {
        let fc = flowchart(
            r#"
flowchart TD
    A["Start Here"] --> B["Ship<br>Now"]
    C["Build<br/>Render"]
"#,
        );
        let label = |id: &str| &fc.nodes.iter().find(|n| n.id == id).unwrap().label;
        assert_eq!(label("A"), "Start Here");
        assert_eq!(label("B"), "Ship\nNow");
        assert_eq!(label("C"), "Build\nRender");
    }

    #[test]
    fn test_parse_sequence_messages() {
        let seq = sequence(
            r#"
sequenceDiagram
    participant Alice
    participant Bob
    Alice->>Bob: Hello
    Bob-->>Alice: Hi there
"#,
        );
        assert_eq!(participant_ids(&seq), ["Alice", "Bob"]);

        let msgs = messages(&seq.elements);
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            (msgs[0].from.as_str(), msgs[0].to.as_str()),
            ("Alice", "Bob")
        );
        assert_eq!(msgs[0].label, "Hello");
        assert_eq!(msgs[0].kind, MessageKind::Sync);
        assert_eq!(
            (msgs[1].from.as_str(), msgs[1].to.as_str()),
            ("Bob", "Alice")
        );
        assert_eq!(msgs[1].label, "Hi there");
        assert_eq!(msgs[1].kind, MessageKind::Reply);
    }

    #[test]
    fn test_parse_sequence_with_aliases() {
        let seq = sequence(
            r#"
sequenceDiagram
    participant U as User
    participant S as Server
    U->>S: Request
"#,
        );
        assert_eq!(participant_ids(&seq), ["U", "S"]);
        assert_eq!(seq.participants[0].alias.as_deref(), Some("User"));
        assert_eq!(seq.participants[1].alias.as_deref(), Some("Server"));
    }

    #[test]
    fn sequence_participants_come_from_actors_and_messages_in_order() {
        let seq = sequence(
            r#"
sequenceDiagram
    actor C as Carol
    Alice->>John: Hi
    loop poll
        John-->>Dave: ping
    end
    Note right of John: retry->later
    participant Alice as Alice Smith
"#,
        );
        assert_eq!(participant_ids(&seq), ["C", "Alice", "John", "Dave"]);
        assert!(seq.elements.iter().any(|e| matches!(
            e,
            SequenceElement::Note { participant, text, .. }
                if participant == "John" && text == "retry->later"
        )));
        assert_eq!(seq.participants[0].alias.as_deref(), Some("Carol"));
        assert_eq!(seq.participants[1].alias.as_deref(), Some("Alice Smith"));
    }

    #[test]
    fn sequence_activation_shorthand_targets_participant() {
        let seq = sequence(
            "sequenceDiagram\n    participant A\n    participant B\n    A->>+B: Hello\n    B-->>-A: Hi\n",
        );
        assert_eq!(participant_ids(&seq), ["A", "B"]);

        let msgs = messages(&seq.elements);
        assert_eq!((msgs[0].from.as_str(), msgs[0].to.as_str()), ("A", "B"));
        assert_eq!(msgs[0].label, "Hello");
        assert_eq!((msgs[1].from.as_str(), msgs[1].to.as_str()), ("B", "A"));
        assert_eq!(msgs[1].label, "Hi");

        // `+` activates the receiver after the message, `-` deactivates the sender.
        assert!(matches!(
            &seq.elements[1],
            SequenceElement::Activation(a) if a.participant == "B"
        ));
        assert!(matches!(
            &seq.elements[3],
            SequenceElement::Deactivation(a) if a.participant == "B"
        ));
    }

    #[test]
    fn sequence_arrow_variants_parse_endpoints() {
        let seq = sequence(
            "sequenceDiagram\n    A-)B: async\n    A--)B: dotted async\n    A-xB: lost\n    A--xB: dotted lost\n",
        );
        assert_eq!(participant_ids(&seq), ["A", "B"]);
        let msgs = messages(&seq.elements);
        let parsed: Vec<_> = msgs
            .iter()
            .map(|m| (m.from.as_str(), m.to.as_str(), &m.msg_type, &m.kind))
            .collect();
        assert_eq!(
            parsed,
            [
                ("A", "B", &MessageType::Solid, &MessageKind::Async),
                ("A", "B", &MessageType::Dotted, &MessageKind::Async),
                ("A", "B", &MessageType::Solid, &MessageKind::Sync),
                ("A", "B", &MessageType::Dotted, &MessageKind::Sync),
            ]
        );
    }

    #[test]
    fn test_parse_sequence_note_positions() {
        assert!(matches!(
            parse_sequence_note("Note left of Bob: hi"),
            Some(SequenceElement::Note { position, participant, text })
                if position == "left" && participant == "Bob" && text == "hi"
        ));
        assert!(matches!(
            parse_sequence_note("Note over Alice: spanning"),
            Some(SequenceElement::Note { position, participant, .. })
                if position == "over" && participant == "Alice"
        ));
        assert!(parse_sequence_note("Note beside Alice: nope").is_none());
        assert!(parse_sequence_note("Note over : no participant").is_none());
        assert!(parse_sequence_note("Note over Alice no colon").is_none());
    }

    #[test]
    fn test_parse_sequence_notes_and_blocks() {
        let seq = sequence(
            r#"
sequenceDiagram
    participant Alice
    participant Bob
    Note right of Alice: Start here
    alt success path
        Alice->>Bob: do work
    else retry
        Bob-->>Alice: try again
    end
"#,
        );
        assert!(seq.elements.iter().any(|e| matches!(
            e,
            SequenceElement::Note { position, participant, text }
                if position == "right" && participant == "Alice" && text == "Start here"
        )));

        let block = seq
            .elements
            .iter()
            .find_map(|e| match e {
                SequenceElement::Block(block) => Some(block),
                _ => None,
            })
            .expect("missing alt block");
        assert_eq!(block.label, "success path");
        assert_eq!(block.messages.len(), 1);
        assert_eq!(block.else_branches.len(), 1);
        assert_eq!(block.else_branches[0].0, "retry");
        assert_eq!(block.else_branches[0].1.len(), 1);
    }

    #[test]
    fn test_parse_flowchart_bidirectional_arrows_precedence() {
        let fc = flowchart("flowchart TD\n    A <==> B\n    C <--> D\n    E <.-> F\n");
        assert_eq!(fc.edges.len(), 3);
        for (from, to, style) in [
            ("A", "B", EdgeStyle::Thick),
            ("C", "D", EdgeStyle::Solid),
            ("E", "F", EdgeStyle::Dotted),
        ] {
            let e = edge(&fc, from, to);
            assert_eq!(e.style, style);
            assert_eq!(e.arrow_head, ArrowType::Arrow);
            assert_eq!(e.arrow_tail, ArrowType::Arrow);
        }
    }

    #[test]
    fn test_parse_class_diagram() {
        let cls = class(
            r#"
classDiagram
    class Animal {
        +String name
        -age: int
        +makeSound()
        #eat(food: Food, amount) bool
    }
"#,
        );
        assert_eq!(cls.classes.len(), 1);
        let animal = &cls.classes[0];
        assert_eq!(animal.name, "Animal");

        // Only `name: Type` declares a type; `Type name` stays a plain name.
        let attrs: Vec<_> = animal
            .attributes
            .iter()
            .map(|a| {
                (
                    &a.member.visibility,
                    a.member.name.as_str(),
                    a.type_annotation.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            attrs,
            [
                (&Visibility::Public, "String name", None),
                (&Visibility::Private, "age", Some("int")),
            ]
        );

        assert_eq!(animal.methods.len(), 2);
        assert_eq!(animal.methods[0].member.name, "makeSound");
        assert!(animal.methods[0].parameters.is_empty());
        assert_eq!(animal.methods[0].return_type, None);
        let eat = &animal.methods[1];
        assert_eq!(eat.member.visibility, Visibility::Protected);
        assert_eq!(eat.member.name, "eat");
        assert_eq!(
            eat.parameters,
            [
                ("food".to_string(), Some("Food".to_string())),
                ("amount".to_string(), None),
            ]
        );
        assert_eq!(eat.return_type, None);
    }

    #[test]
    fn test_parse_class_relations_with_labels() {
        let cls = class(
            r#"
classDiagram
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
    User ..> AuditLog : writes
"#,
        );
        assert_eq!(cls.classes.len(), 3);
        assert_eq!(cls.relations.len(), 2);

        let relation = |to: &str| {
            cls.relations
                .iter()
                .find(|r| r.from == "User" && r.to == to)
                .unwrap_or_else(|| panic!("missing relation to {to}"))
        };
        assert_eq!(relation("Session").label.as_deref(), Some("creates"));
        assert_eq!(relation("AuditLog").label.as_deref(), Some("writes"));
        assert_eq!(relation("AuditLog").to_marker, ClassMarker::Arrow);
        assert!(relation("AuditLog").dashed);
    }

    #[test]
    fn class_relations_follow_mermaid_relation_grammar() {
        use ClassMarker::*;
        // (line, from, from marker, dashed, to marker, to)
        let cases = [
            ("A <|-- B", "A", Triangle, false, None, "B"),
            ("A --|> B", "A", None, false, Triangle, "B"),
            ("A *-- B", "A", FilledDiamond, false, None, "B"),
            ("A --* B", "A", None, false, FilledDiamond, "B"),
            ("A o-- B", "A", HollowDiamond, false, None, "B"),
            ("A --o B", "A", None, false, HollowDiamond, "B"),
            ("A <-- B", "A", Arrow, false, None, "B"),
            ("A -- B", "A", None, false, None, "B"),
            ("A .. B", "A", None, true, None, "B"),
            ("A <.. B", "A", Arrow, true, None, "B"),
            ("A <|.. B", "A", Triangle, true, None, "B"),
            ("A ..|> B", "A", None, true, Triangle, "B"),
            ("A <|--|> B", "A", Triangle, false, Triangle, "B"),
            ("A *--* B", "A", FilledDiamond, false, FilledDiamond, "B"),
            ("bar ()-- foo", "bar", Lollipop, false, None, "foo"),
            ("foo --() bar", "foo", None, false, Lollipop, "bar"),
            ("Foo--Bar", "Foo", None, false, None, "Bar"),
            ("Foo-Bar --> Baz", "Foo-Bar", None, false, Arrow, "Baz"),
        ];
        for (line, from, from_marker, dashed, to_marker, to) in cases {
            let cls = class(&format!("classDiagram\n    {line}"));
            let names: Vec<&str> = cls.classes.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(names, [from, to], "{line}");
            let rel = &cls.relations[0];
            assert_eq!(
                (rel.from_marker, rel.dashed, rel.to_marker),
                (from_marker, dashed, to_marker),
                "{line}"
            );
        }
    }

    #[test]
    fn class_relation_cardinalities_and_labels_stay_out_of_class_names() {
        let cls = class(
            "classDiagram\n    Customer \"1\" --> \"0..*\" Ticket : buys\n    A ..> B : see -- note",
        );
        let names: Vec<&str> = cls.classes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Customer", "Ticket", "A", "B"]);
        let buys = &cls.relations[0];
        assert_eq!(buys.from_cardinality.as_deref(), Some("1"));
        assert_eq!(buys.to_cardinality.as_deref(), Some("0..*"));
        assert_eq!(buys.label.as_deref(), Some("buys"));
        assert_eq!(cls.relations[1].label.as_deref(), Some("see -- note"));
    }

    #[test]
    fn class_relations_create_missing_classes() {
        let cls = class(
            "classDiagram\n    Animal <|-- Duck\n    Animal <|-- Fish\n    class Pond\n    Pond o-- Duck\n",
        );
        let names: Vec<&str> = cls.classes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Pond", "Animal", "Duck", "Fish"]);
        assert_eq!(cls.relations.len(), 3);
    }

    #[test]
    fn test_parse_er_entity_blocks_and_relationship_labels() {
        let er = er(r#"
erDiagram
    USER ||--o{ ORDER : places
    ORDER ||--|{ ORDER_ITEM : contains

    USER {
        string id
        string email
    }

    ORDER {
        string id
    }

    ORDER_ITEM {
        string order_id
    }
"#);
        assert_eq!(er.entities.len(), 3);
        assert_eq!(er.relationships.len(), 2);

        let user = er
            .entities
            .iter()
            .find(|e| e.name == "USER")
            .expect("missing USER");
        assert_eq!(user.attributes.len(), 2);
        assert_eq!(user.attributes[0].name, "string id");
        assert_eq!(user.attributes[1].name, "string email");

        let places = er
            .relationships
            .iter()
            .find(|r| r.from == "USER" && r.to == "ORDER")
            .expect("missing places relationship");
        assert_eq!(places.label.as_deref(), Some("places"));
    }

    #[test]
    fn er_relationship_cardinalities_compose_both_sides() {
        let er = er(
            "erDiagram\n    A ||--o| B : has\n    C }o..o| D\n    E |o--|{ F\n    G }|..|| H\n    I-X }o--o{ J-Y\n",
        );
        use ErCardinality::*;
        let parsed: Vec<_> = er
            .relationships
            .iter()
            .map(|r| {
                (
                    r.from.as_str(),
                    r.to.as_str(),
                    &r.from_cardinality,
                    &r.to_cardinality,
                )
            })
            .collect();
        assert_eq!(
            parsed,
            [
                ("A", "B", &ExactlyOne, &ZeroOrOne),
                ("C", "D", &ZeroOrMore, &ZeroOrOne),
                ("E", "F", &ZeroOrOne, &OneOrMore),
                ("G", "H", &OneOrMore, &ExactlyOne),
                ("I-X", "J-Y", &ZeroOrMore, &ZeroOrMore),
            ]
        );
        assert_eq!(er.relationships[0].label.as_deref(), Some("has"));
        let names: Vec<&str> = er.entities.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["A", "B", "C", "D", "E", "F", "G", "H", "I-X", "J-Y"]
        );
    }

    #[test]
    fn test_parse_state_diagram() {
        let st = state(
            "\nstateDiagram\n    [*] --> Idle\n    Idle --> Processing\n    Processing --> [*]\n",
        );
        assert!(st.states.iter().any(|s| s.is_start));
        assert!(st.states.iter().any(|s| s.is_end));
        assert!(
            st.transitions
                .iter()
                .any(|t| t.from == "Idle" && t.to == "Processing")
        );
    }

    #[test]
    fn composite_pseudo_states_belong_to_their_composite() {
        let st = state(
            "stateDiagram-v2\n[*] --> Comp\nstate Comp {\n    [*] --> X\n    X --> [*]\n}\nComp --> [*]",
        );
        let by_id = st.states_by_id();
        let comp: Vec<&State> = by_id["Comp"]
            .child_state_ids()
            .map(|id| by_id[id])
            .collect();
        let inner_start = comp.iter().find(|s| s.is_start).expect("Comp start");
        let inner_end = comp.iter().find(|s| s.is_end).expect("Comp end");
        for t in &st.transitions {
            assert_ne!(
                t.from, inner_start.id,
                "top-level transition from Comp's start"
            );
            assert_ne!(t.to, inner_end.id, "top-level transition to Comp's end");
        }
        assert!(
            st.transitions
                .iter()
                .any(|t| t.to == "Comp" && by_id[t.from.as_str()].is_start)
        );
        assert!(
            st.transitions
                .iter()
                .any(|t| t.from == "Comp" && by_id[t.to.as_str()].is_end)
        );
    }

    #[test]
    fn state_alias_label_survives_later_references() {
        let label_of = |src: &str| {
            let st = state(src);
            let ll = st.states.iter().find(|s| s.id == "LL").unwrap();
            ll.label.clone()
        };
        for src in [
            "stateDiagram-v2\nstate \"Long label\" as LL\nA --> LL\nLL --> A",
            "stateDiagram-v2\nA --> LL\nstate \"Long label\" as LL\nLL --> A",
            "stateDiagram-v2\nstate \"Long label\" as LL\nstate LL {\n    X --> Y\n}",
        ] {
            assert_eq!(label_of(src), "Long label", "{src:?}");
        }
        assert_eq!(label_of("stateDiagram-v2\nA --> LL"), "LL");
    }

    #[test]
    fn test_parse_state_composite_with_note_children() {
        let st = state(
            r#"
stateDiagram
    state Parent {
        state Child
        Child --> Child: loop
    }
    Note right of Child: child note
"#,
        );
        let parent = st.states.iter().find(|s| s.id == "Parent").unwrap();
        assert!(parent.is_composite);
        assert!(
            parent
                .children
                .iter()
                .any(|e| matches!(e, StateElement::State(id) if id == "Child"))
        );
        assert!(parent.children.iter().any(
            |e| matches!(e, StateElement::Transition(t) if t.from == "Child" && t.to == "Child")
        ));

        let child = st.states.iter().find(|s| s.id == "Child").unwrap();
        assert!(child.children.iter().any(|e| matches!(
            e,
            StateElement::Note { state, text } if state == "Child" && text == "child note"
        )));
    }

    #[test]
    fn test_parse_flowchart_cross_and_circle_arrows() {
        let fc = flowchart(
            "flowchart TD\n    A --x B\n    C x-- D\n    E --o F\n    G o-- H\n    I x--x J\n    K o--o L\n",
        );
        assert_eq!(fc.edges.len(), 6);
        for (from, to, head, tail) in [
            ("A", "B", ArrowType::Cross, ArrowType::None),
            ("C", "D", ArrowType::None, ArrowType::Cross),
            ("E", "F", ArrowType::Circle, ArrowType::None),
            ("G", "H", ArrowType::None, ArrowType::Circle),
            ("I", "J", ArrowType::Cross, ArrowType::Cross),
            ("K", "L", ArrowType::Circle, ArrowType::Circle),
        ] {
            let e = edge(&fc, from, to);
            assert_eq!(e.arrow_head, head, "{from} -> {to}");
            assert_eq!(e.arrow_tail, tail, "{from} -> {to}");
        }
    }

    #[test]
    fn test_parse_flowchart_dotted_cross_and_circle_arrows() {
        let fc = flowchart("flowchart TD\n    A -.x B\n    C x-. D\n    E -.o F\n    G o-. H\n");
        assert_eq!(fc.edges.len(), 4);
        for (from, to, head, tail) in [
            ("A", "B", ArrowType::Cross, ArrowType::None),
            ("C", "D", ArrowType::None, ArrowType::Cross),
            ("E", "F", ArrowType::Circle, ArrowType::None),
            ("G", "H", ArrowType::None, ArrowType::Circle),
        ] {
            let e = edge(&fc, from, to);
            assert_eq!(e.style, EdgeStyle::Dotted, "{from} -> {to}");
            assert_eq!(e.arrow_head, head, "{from} -> {to}");
            assert_eq!(e.arrow_tail, tail, "{from} -> {to}");
        }
    }

    #[test]
    fn test_parse_flowchart_open_arrow_variant() {
        let fc = flowchart("flowchart TD\n    A --o> B\n");
        assert_eq!(fc.edges.len(), 1);
        let e = edge(&fc, "A", "B");
        assert_eq!(e.arrow_head, ArrowType::Arrow);
        assert_eq!(e.arrow_tail, ArrowType::Circle);
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    fn parse_err(src: &str) -> String {
        match parse_mermaid(src) {
            Ok(diagram) => panic!("expected an error, got {diagram:?}"),
            Err(err) => err,
        }
    }

    #[test]
    fn test_parse_class_missing_stereotype_end() {
        let err = parse_err("classDiagram\nclass <<abstract Animal");
        assert!(err.contains("missing '>>' in stereotype"), "{err}");
    }

    #[test]
    fn test_parse_state_missing_definition() {
        let err = parse_err("stateDiagram\nstate {");
        assert!(err.contains("Missing state definition"), "{err}");
    }

    #[test]
    fn test_parse_state_missing_closing_quote() {
        let err = parse_err("stateDiagram\nstate \"Label");
        assert!(
            err.contains("Missing closing quote in state label"),
            "{err}"
        );
    }

    #[test]
    fn test_parse_state_missing_identifier() {
        let err = parse_err("stateDiagram\nstate \"Label\"");
        assert!(err.contains("Missing state identifier"), "{err}");
    }

    #[test]
    fn unsupported_diagram_type_is_rejected() {
        let err = parse_err("pie title Pets\n  \"Dogs\" : 50");
        assert_eq!(err, "unsupported Mermaid diagram type: pie");
        let err = parse_err("unknownDiagram\n  A --> B");
        assert_eq!(err, "unsupported Mermaid diagram type: unknownDiagram");
    }

    #[test]
    fn diagrams_without_content_are_rejected() {
        for src in [
            "flowchart LR\n  ??? invalid syntax ???",
            "graph TD",
            "sequenceDiagram\n  %% nothing here",
            "classDiagram",
            "stateDiagram-v2\n",
            "erDiagram",
        ] {
            parse_err(src);
        }
    }

    #[test]
    fn directives_and_front_matter_before_header_are_skipped() {
        for src in [
            "%%{init: {'theme': 'dark'}}%%\nflowchart TD\n  A --> B",
            "---\ntitle: Demo\n---\n%% comment\nsequenceDiagram\n  A->>B: hi",
        ] {
            assert!(parse_mermaid(src).is_ok(), "{src}");
        }
    }

    #[test]
    fn diagrams_with_only_nodes_are_accepted() {
        for src in [
            "flowchart TD\n  A[Alone]",
            "classDiagram\n  class Solo",
            "stateDiagram\n  state Solo",
            "erDiagram\n  SOLO",
            "sequenceDiagram\n  participant Solo",
        ] {
            assert!(parse_mermaid(src).is_ok(), "{src}");
        }
    }
}
