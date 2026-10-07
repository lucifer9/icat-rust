use std::collections::{HashMap, HashSet};

/// Node shapes in flowcharts
#[derive(Debug, Clone, PartialEq)]
pub enum NodeShape {
    Rect,
    RoundedRect,
    Stadium,
    Subroutine,
    Cylinder,
    Circle,
    DoubleCircle,
    Rhombus,
    Hexagon,
    Parallelogram,
    ParallelogramAlt,
    Trapezoid,
    TrapezoidAlt,
}

/// Edge styles
#[derive(Debug, Clone, PartialEq)]
pub enum EdgeStyle {
    Solid,
    Dotted,
    Thick,
}

/// Arrow types
#[derive(Debug, Clone, PartialEq)]
pub enum ArrowType {
    Arrow,
    Circle,
    Cross,
    None,
}

/// A node in a flowchart
#[derive(Debug, Clone)]
pub struct FlowchartNode {
    pub id: String,
    pub label: String,
    pub shape: NodeShape,
}

/// An edge connecting two nodes
#[derive(Debug, Clone)]
pub struct FlowchartEdge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub style: EdgeStyle,
    pub arrow_head: ArrowType,
    pub arrow_tail: ArrowType,
}

/// Direction of flowchart
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlowDirection {
    TopDown,
    BottomUp,
    LeftRight,
    RightLeft,
}

/// A complete flowchart diagram
#[derive(Debug, Clone)]
pub struct Flowchart {
    pub direction: FlowDirection,
    pub nodes: Vec<FlowchartNode>,
    pub edges: Vec<FlowchartEdge>,
    pub subgraphs: Vec<Subgraph>,
}

/// A subgraph (grouped nodes)
#[derive(Debug, Clone)]
pub struct Subgraph {
    pub title: String,
    pub nodes: Vec<String>,
}

// ============================================
// Sequence Diagram Types
// ============================================

#[derive(Debug, Clone)]
pub struct Participant {
    pub id: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageType {
    Solid,
    Dotted,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessageKind {
    Sync,
    Async,
    Reply,
}

#[derive(Debug, Clone)]
pub struct SequenceMessage {
    pub from: String,
    pub to: String,
    pub label: String,
    pub msg_type: MessageType,
    pub kind: MessageKind,
}

#[derive(Debug, Clone)]
pub struct Activation {
    pub participant: String,
}

#[derive(Debug, Clone)]
pub enum SequenceBlockType {
    Alt,
    Opt,
    Loop,
    Par,
    Critical,
}

#[derive(Debug, Clone)]
pub struct SequenceBlock {
    pub block_type: SequenceBlockType,
    pub label: String,
    pub messages: Vec<SequenceElement>,
    pub else_branches: Vec<(String, Vec<SequenceElement>)>,
}

#[derive(Debug, Clone)]
pub enum SequenceElement {
    Message(SequenceMessage),
    Activation(Activation),
    Deactivation(Activation),
    Note {
        participant: String,
        position: String,
        text: String,
    },
    Block(SequenceBlock),
}

#[derive(Debug, Clone)]
pub struct SequenceDiagram {
    pub participants: Vec<Participant>,
    pub elements: Vec<SequenceElement>,
}

// ============================================
// Class Diagram Types
// ============================================

#[derive(Debug, Clone, PartialEq)]
pub enum Visibility {
    Public,
    Private,
    Protected,
    Package,
}

impl Visibility {
    pub fn from_symbol(symbol: char) -> Option<Self> {
        match symbol {
            '+' => Some(Self::Public),
            '-' => Some(Self::Private),
            '#' => Some(Self::Protected),
            '~' => Some(Self::Package),
            _ => None,
        }
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Public => "+",
            Self::Private => "-",
            Self::Protected => "#",
            Self::Package => "~",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClassMember {
    pub visibility: Visibility,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ClassAttribute {
    pub member: ClassMember,
    pub type_annotation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClassMethod {
    pub member: ClassMember,
    pub parameters: Vec<(String, Option<String>)>,
    pub return_type: Option<String>,
}

impl ClassAttribute {
    /// Text shown for the attribute row, e.g. `+ name: String`.
    pub fn display(&self) -> String {
        let vis = self.member.visibility.symbol();
        match &self.type_annotation {
            Some(ty) => format!("{} {}: {}", vis, self.member.name, ty),
            None => format!("{} {}", vis, self.member.name),
        }
    }
}

impl ClassMethod {
    /// Text shown for the method row, e.g. `+ run(n: int): bool`.
    pub fn display(&self) -> String {
        let vis = self.member.visibility.symbol();
        let params: Vec<String> = self
            .parameters
            .iter()
            .map(|(name, ty)| match ty {
                Some(ty) => format!("{}: {}", name, ty),
                None => name.clone(),
            })
            .collect();
        let params = params.join(", ");
        match &self.return_type {
            Some(ret) => format!("{} {}({}): {}", vis, self.member.name, params, ret),
            None => format!("{} {}({})", vis, self.member.name, params),
        }
    }
}

/// Decoration at one end of a class relation, drawn at the class it is written
/// next to: `A <|-- B` marks A, `A --> B` marks B.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClassMarker {
    None,
    /// `<` or `>`
    Arrow,
    /// `<|` or `|>`
    Triangle,
    /// `*`
    FilledDiamond,
    /// `o`
    HollowDiamond,
    /// `()`, a lollipop interface
    Lollipop,
}

#[derive(Debug, Clone)]
pub struct ClassRelation {
    pub from: String,
    pub to: String,
    pub from_marker: ClassMarker,
    pub to_marker: ClassMarker,
    /// `..` instead of `--`.
    pub dashed: bool,
    pub from_cardinality: Option<String>,
    pub to_cardinality: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClassDefinition {
    pub name: String,
    pub stereotype: Option<String>,
    pub attributes: Vec<ClassAttribute>,
    pub methods: Vec<ClassMethod>,
    pub is_abstract: bool,
    pub is_interface: bool,
}

impl ClassDefinition {
    /// Header text, prefixed with the stereotype when there is one.
    pub fn title(&self) -> String {
        let stereotype = match &self.stereotype {
            Some(stereotype) => Some(stereotype.as_str()),
            None if self.is_interface => Some("interface"),
            None => None,
        };
        match stereotype {
            Some(stereotype) => format!("<<{}>> {}", stereotype, self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClassDiagram {
    pub classes: Vec<ClassDefinition>,
    pub relations: Vec<ClassRelation>,
}

// ============================================
// State Diagram Types
// ============================================

#[derive(Debug, Clone)]
pub struct State {
    pub id: String,
    pub label: String,
    pub is_start: bool,
    pub is_end: bool,
    pub is_composite: bool,
    pub children: Vec<StateElement>,
}

#[derive(Debug, Clone)]
pub struct StateTransition {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub enum StateElement {
    /// Id of a nested state; the state itself lives in `StateDiagram::states`.
    State(String),
    Transition(StateTransition),
    Note {
        state: String,
        text: String,
    },
}

impl State {
    pub fn child_state_ids(&self) -> impl Iterator<Item = &str> {
        self.children.iter().filter_map(|child| match child {
            StateElement::State(id) => Some(id.as_str()),
            _ => None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct StateDiagram {
    pub states: Vec<State>,
    pub transitions: Vec<StateTransition>,
}

impl StateDiagram {
    /// Ids of states drawn inside a composite state rather than at the top level.
    pub fn nested_state_ids(&self) -> HashSet<&str> {
        self.states
            .iter()
            .flat_map(State::child_state_ids)
            .collect()
    }

    pub fn states_by_id(&self) -> HashMap<&str, &State> {
        self.states.iter().map(|s| (s.id.as_str(), s)).collect()
    }
}

// ============================================
// ER Diagram Types
// ============================================

#[derive(Debug, Clone, PartialEq)]
pub enum ErCardinality {
    ZeroOrOne,
    ExactlyOne,
    ZeroOrMore,
    OneOrMore,
}

#[derive(Debug, Clone)]
pub struct ErAttribute {
    pub name: String,
    pub is_key: bool,
}

impl ErAttribute {
    /// Text shown for the attribute row; key attributes keep Mermaid's `*` marker.
    pub fn display(&self) -> String {
        if self.is_key {
            format!("*{}", self.name)
        } else {
            self.name.clone()
        }
    }
}

#[derive(Debug, Clone)]
pub struct ErEntity {
    pub name: String,
    pub attributes: Vec<ErAttribute>,
}

#[derive(Debug, Clone)]
pub struct ErRelationship {
    pub from: String,
    pub to: String,
    pub from_cardinality: ErCardinality,
    pub to_cardinality: ErCardinality,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ErDiagram {
    pub entities: Vec<ErEntity>,
    pub relationships: Vec<ErRelationship>,
}
