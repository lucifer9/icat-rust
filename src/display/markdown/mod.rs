pub mod fonts;
mod markie;
mod math;
mod mermaid;

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, PhysicalGlyph, Renderer, Shaping, Style,
    SwashCache, UnderlineStyle, Weight, render_decoration,
};
use image::{DynamicImage, ImageBuffer, Rgba, imageops};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use syntect::highlighting::{FontStyle, Style as SyntaxStyle};
use tiny_skia::{Paint as SkPaint, Pixmap, Rect as SkRect, Transform};

use crate::display::MarkdownOptions;
use crate::imgutil;
use crate::term::{self, Size};

const DEFAULT_MARKDOWN_WIDTH: u32 = 1024;
const DEFAULT_MARKDOWN_MARGIN: u32 = 48;
pub const DEFAULT_MARKDOWN_FONT_PT: f64 = 24.0;
pub const MIN_MARKDOWN_WIDTH: u32 = 480;
const MARKDOWN_CHUNK_HEIGHT: u32 = 8192;
const CODE_PADDING: u32 = 10;

// Text sizes as multiples of the body font size. HEADING_SCALE is indexed by
// heading level 1..=5; deeper levels reuse the last entry.
const HEADING_SCALE: [f64; 5] = [1.9, 1.6, 1.4, 1.25, 1.15];
const CODE_FONT_EM: f64 = 0.95;
const LINE_HEIGHT_EM: f32 = 1.4;
// Baseline offset of a text run within an inline line.
const INLINE_BASELINE_EM: f32 = 0.9;

// Vertical spacing between blocks, in multiples of the body font size.
const HEADING_SPACE_ABOVE_EM: f64 = 0.75;
const HEADING_SPACE_BELOW_EM: f64 = 0.5;
const IMAGE_GAP_EM: f64 = 0.6;
const PARAGRAPH_GAP_EM: f64 = 0.9;
const LIST_ITEM_GAP_TIGHT_EM: f64 = 0.6;
const LIST_ITEM_GAP_LOOSE_EM: f64 = 0.9;
const LIST_GAP_EM: f64 = 0.7;
const QUOTE_GAP_EM: f64 = 0.9;
const TABLE_GAP_EM: f64 = 0.5;
const TABLE_PADDING_EM: f64 = 0.6;
const TABLE_MIN_PADDING: f64 = 8.0;

// Fixed pixel geometry.
const LIST_MARKER_WIDTH: u32 = 32;
const LIST_INDENT: u32 = 40;
// Added to the font size to get the minimum height of a list item.
const LIST_ITEM_MIN_EXTRA: u32 = 8;
const QUOTE_INDENT: u32 = 48;
const QUOTE_BAR_WIDTH: u32 = 4;
const EMPTY_QUOTE_ADVANCE: u32 = 4;
const RULE_OFFSET: u32 = 4;
const RULE_ADVANCE: u32 = 12;
const RULE_THICKNESS: u32 = 2;
// Extra background below the bottom code padding.
const CODE_BOTTOM_EXTRA: u32 = 6;
const CODE_BLOCK_GAP: u32 = 6;
const TABLE_BORDER: u32 = 1;
const TABLE_MIN_COL_WIDTH: u32 = 60;
// Minimum document height below the top margin.
const MIN_BODY_HEIGHT: u32 = 50;

const QUOTE_BAR_COLOR: Rgba<u8> = Rgba([180, 180, 180, 255]);
const TABLE_BORDER_COLOR: Rgba<u8> = Rgba([200, 200, 200, 255]);
const TABLE_HEADER_BG: Rgba<u8> = Rgba([240, 240, 240, 255]);
const CODE_BG: [u8; 4] = [245, 245, 245, 255];
const RULE_COLOR: [u8; 4] = [220, 220, 220, 255];
const CODE_TEXT_COLOR: Color = Color::rgb(50, 50, 50);

// Cached syntax highlighting sets (loaded once, reused across calls)
static SYNTAX_SET: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<syntect::highlighting::ThemeSet> = OnceLock::new();

// Cached font system and glyph cache (expensive to initialise; reused across render calls)
struct FontState {
    font_system: FontSystem,
    swash: SwashCache,
    warning: Option<String>,
}

static FONT_STATE: OnceLock<Mutex<FontState>> = OnceLock::new();

pub fn markdown_with_options(
    path: &str,
    size: Size,
    tmux: bool,
    opts: MarkdownOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let raw = imgutil::read_source(path).map_err(|err| {
        let label = if path.is_empty() { "<stdin>" } else { path };
        format!("failed to read Markdown {label}: {err}")
    })?;
    markdown_from_bytes_impl(&raw, markdown_base_dir(path), opts, size, tmux)
}

pub fn markdown_from_bytes_with_options(
    data: &[u8],
    size: Size,
    tmux: bool,
    opts: MarkdownOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    markdown_from_bytes_impl(data, PathBuf::new(), opts, size, tmux)
}

fn markdown_from_bytes_impl(
    data: &[u8],
    base_dir: PathBuf,
    opts: MarkdownOptions,
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let page_height = markdown_page_height(size);
    let width = markdown_render_width(size.pixel_width);
    let blocks = parse_markdown_blocks(data);

    // Layout runs once over the whole document to learn its height; pages are
    // then drawn on demand into page-sized pixmaps.
    let layout = with_fonts(|font_system, _| {
        layout_document(&blocks, &base_dir, font_system, width, opts.font_size_pt)
    })?;

    let total_pages = markdown_total_pages(layout.height, page_height);
    let interactive = opts.page.is_none() && total_pages > 1 && std::io::stdout().is_terminal();
    let mut current = opts.page.unwrap_or(1).clamp(1, total_pages);
    loop {
        let image = with_fonts(|font_system, swash| {
            render_page(&layout, font_system, swash, current, page_height)
        })?;
        send_rendered_markdown(&image, size, tmux)?;
        if !interactive {
            return Ok(());
        }

        let prompt = if current < total_pages {
            format!(
                "-- Markdown page {current}/{total_pages} -- Enter next, number+Enter jump, q+Enter quit: "
            )
        } else {
            format!("-- Markdown page {current}/{total_pages} -- Enter/q quit, number+Enter jump: ")
        };
        let input = match term::read_interactive_line(&prompt) {
            Ok(s) => s,
            Err(_) => return Ok(()),
        };
        match markdown_pager_action(current, total_pages, &input) {
            PagerAction::ShowPage(next) => current = next,
            PagerAction::Quit => return Ok(()),
        }
    }
}

// Runs `f` with the shared font system and glyph cache, printing the font
// warning once on first use.
fn with_fonts<R>(f: impl FnOnce(&mut FontSystem, &mut SwashCache) -> R) -> R {
    let state = FONT_STATE.get_or_init(|| {
        let res = fonts::resolve_fonts();
        Mutex::new(FontState {
            font_system: res.font_system,
            swash: SwashCache::new(),
            warning: res.warning,
        })
    });
    let mut guard = state.lock().unwrap();
    if let Some(w) = guard.warning.take() {
        eprintln!("{w}");
    }
    let FontState {
        font_system, swash, ..
    } = &mut *guard;
    f(font_system, swash)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PagerAction {
    ShowPage(usize),
    Quit,
}

fn markdown_pager_action(current: usize, total_pages: usize, input: &str) -> PagerAction {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        if current < total_pages {
            return PagerAction::ShowPage(current + 1);
        }
        return PagerAction::Quit;
    }
    if trimmed.eq_ignore_ascii_case("q") {
        return PagerAction::Quit;
    }
    if let Ok(n) = trimmed.parse::<usize>() {
        return PagerAction::ShowPage(n.clamp(1, total_pages));
    }
    PagerAction::ShowPage(current)
}

// ── AST types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum InlineToken {
    Text { text: String, style: InlineStyle },
    Image { path: PathBuf },
    Math { text: String, display: bool },
    SoftBreak,
    HardBreak,
}

#[derive(Debug, Clone, Copy, Default)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    mono: bool,
    color: Option<u32>, // packed 0xRRGGBB
    underline: bool,
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)]
enum Block {
    Heading {
        level: u32,
        tokens: Vec<InlineToken>,
    },
    Paragraph(Vec<InlineToken>),
    List {
        ordered: bool,
        tight: bool,
        items: Vec<Vec<InlineToken>>,
    },
    BlockQuote(Vec<Block>),
    Code {
        lang: String,
        text: String,
    },
    Math(String),
    Rule,
    Table {
        header: Vec<Vec<InlineToken>>,
        rows: Vec<Vec<Vec<InlineToken>>>,
    },
}

// ── Parser ───────────────────────────────────────────────────────────────────

fn parse_markdown_blocks(data: &[u8]) -> Vec<Block> {
    let markdown = String::from_utf8_lossy(data);
    let parser = Parser::new_ext(&markdown, Options::all() | Options::ENABLE_MATH);
    let events: Vec<Event<'_>> = parser.collect();
    let mut idx = 0;
    parse_block_list(&events, &mut idx)
}

// Consumes the closing event of a container when the cursor is on it.
fn skip_end(events: &[Event<'_>], idx: &mut usize, end: TagEnd) {
    if matches!(events.get(*idx), Some(Event::End(e)) if *e == end) {
        *idx += 1;
    }
}

fn parse_block_list(events: &[Event<'_>], idx: &mut usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    while *idx < events.len() {
        if matches!(&events[*idx], Event::End(_)) {
            break;
        }
        let before = *idx;
        if let Some(block) = parse_one_block(events, idx) {
            blocks.push(block);
        } else if *idx == before {
            *idx += 1;
        }
    }
    blocks
}

fn parse_one_block(events: &[Event<'_>], idx: &mut usize) -> Option<Block> {
    match &events[*idx] {
        Event::Start(tag @ Tag::Heading { level, .. }) => {
            *idx += 1;
            let tokens = collect_inline_tokens(events, idx, InlineStyle::default());
            skip_end(events, idx, tag.to_end());
            Some(Block::Heading {
                level: *level as u32,
                tokens,
            })
        }
        Event::Start(Tag::Paragraph) => {
            *idx += 1;
            let tokens = collect_inline_tokens(events, idx, InlineStyle::default());
            skip_end(events, idx, TagEnd::Paragraph);
            if tokens.is_empty() {
                None
            } else {
                Some(Block::Paragraph(tokens))
            }
        }
        Event::Start(tag @ Tag::BlockQuote(_)) => {
            *idx += 1;
            let children = parse_block_list(events, idx);
            skip_end(events, idx, tag.to_end());
            Some(Block::BlockQuote(children))
        }
        Event::Start(tag @ Tag::List(start)) => {
            let ordered = start.is_some();
            *idx += 1;
            let (items, tight) = collect_list_items(events, idx);
            skip_end(events, idx, tag.to_end());
            Some(Block::List {
                ordered,
                tight,
                items,
            })
        }
        Event::Start(Tag::CodeBlock(kind)) => {
            let lang = match kind {
                CodeBlockKind::Fenced(l) => l.to_string(),
                CodeBlockKind::Indented => String::new(),
            };
            *idx += 1;
            let mut text = String::new();
            while *idx < events.len() {
                match &events[*idx] {
                    Event::Text(t) => {
                        text.push_str(t);
                        *idx += 1;
                    }
                    Event::End(TagEnd::CodeBlock) => {
                        *idx += 1;
                        break;
                    }
                    _ => {
                        *idx += 1;
                    }
                }
            }
            Some(Block::Code {
                lang,
                text: text.trim_end().to_string(),
            })
        }
        Event::DisplayMath(text) => {
            *idx += 1;
            Some(Block::Math(text.to_string()))
        }
        Event::Start(Tag::Table(_)) => {
            *idx += 1;
            let (header, rows) = collect_table(events, idx);
            skip_end(events, idx, TagEnd::Table);
            Some(Block::Table { header, rows })
        }
        Event::Start(Tag::FootnoteDefinition(label)) => {
            let label = label.to_string();
            *idx += 1;
            let children = parse_block_list(events, idx);
            skip_end(events, idx, TagEnd::FootnoteDefinition);
            let text = flatten_blocks_to_text(&children).trim().to_string();
            if text.is_empty() {
                None
            } else {
                Some(Block::Paragraph(vec![plain_text_token(&format!(
                    "[{label}] {text}"
                ))]))
            }
        }
        Event::Start(Tag::DefinitionList) => {
            *idx += 1;
            let items = collect_definition_list(events, idx);
            skip_end(events, idx, TagEnd::DefinitionList);
            if items.is_empty() {
                None
            } else {
                Some(Block::List {
                    ordered: false,
                    tight: true,
                    items,
                })
            }
        }
        Event::Rule => {
            *idx += 1;
            Some(Block::Rule)
        }
        // Skip HTML blocks, footnotes, and other unknown block-level tags
        Event::Start(_) => {
            let mut depth = 1_usize;
            *idx += 1;
            while *idx < events.len() && depth > 0 {
                match &events[*idx] {
                    Event::Start(_) => depth += 1,
                    Event::End(_) => depth -= 1,
                    _ => {}
                }
                *idx += 1;
            }
            None
        }
        _ => None,
    }
}

fn collect_inline_tokens(
    events: &[Event<'_>],
    idx: &mut usize,
    style: InlineStyle,
) -> Vec<InlineToken> {
    let mut tokens = Vec::new();
    while *idx < events.len() {
        match &events[*idx] {
            Event::End(_) => break,
            Event::Text(text) => {
                tokens.push(InlineToken::Text {
                    text: text.to_string(),
                    style,
                });
                *idx += 1;
            }
            Event::Code(text) => {
                // Inline code span: monospace
                tokens.push(InlineToken::Text {
                    text: text.to_string(),
                    style: InlineStyle {
                        mono: true,
                        ..InlineStyle::default()
                    },
                });
                *idx += 1;
            }
            Event::InlineMath(text) => {
                tokens.push(InlineToken::Math {
                    text: text.to_string(),
                    display: false,
                });
                *idx += 1;
            }
            Event::DisplayMath(text) => {
                tokens.push(InlineToken::Math {
                    text: text.to_string(),
                    display: true,
                });
                *idx += 1;
            }
            Event::SoftBreak => {
                tokens.push(InlineToken::SoftBreak);
                *idx += 1;
            }
            Event::HardBreak => {
                tokens.push(InlineToken::HardBreak);
                *idx += 1;
            }
            Event::Start(Tag::Emphasis) => {
                *idx += 1;
                // Emphasis level-1: entering italic escalates bold→bold+italic
                let inner = collect_inline_tokens(
                    events,
                    idx,
                    InlineStyle {
                        italic: true,
                        ..style
                    },
                );
                skip_end(events, idx, TagEnd::Emphasis);
                tokens.extend(inner);
            }
            Event::Start(Tag::Strong) => {
                *idx += 1;
                // Emphasis level-2: entering bold escalates italic→bold+italic
                let inner = collect_inline_tokens(
                    events,
                    idx,
                    InlineStyle {
                        bold: true,
                        ..style
                    },
                );
                skip_end(events, idx, TagEnd::Strong);
                tokens.extend(inner);
            }
            Event::Start(Tag::Link { .. }) => {
                *idx += 1;
                let mut link_tokens = collect_inline_tokens(events, idx, style);
                skip_end(events, idx, TagEnd::Link);
                // Link color 0x064FBD + underline on all text spans
                for t in link_tokens.iter_mut() {
                    if let InlineToken::Text { style, .. } = t {
                        style.color = Some(0x064FBD);
                        style.underline = true;
                    }
                }
                tokens.extend(link_tokens);
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let path = PathBuf::from(dest_url.to_string());
                *idx += 1;
                // Skip alt text events
                while *idx < events.len() {
                    if matches!(&events[*idx], Event::End(TagEnd::Image)) {
                        *idx += 1;
                        break;
                    }
                    *idx += 1;
                }
                tokens.push(InlineToken::Image { path });
            }
            Event::Start(Tag::List(_)) => break,
            Event::Start(Tag::Strikethrough) => {
                *idx += 1;
                let inner = collect_inline_tokens(events, idx, style);
                skip_end(events, idx, TagEnd::Strikethrough);
                tokens.extend(inner);
            }
            _ => {
                *idx += 1;
            }
        }
    }
    tokens
}

fn plain_text_token(text: &str) -> InlineToken {
    InlineToken::Text {
        text: text.to_string(),
        style: InlineStyle::default(),
    }
}

fn collect_list_items(events: &[Event<'_>], idx: &mut usize) -> (Vec<Vec<InlineToken>>, bool) {
    let mut items = Vec::new();
    let mut tight = true;
    while *idx < events.len() {
        match &events[*idx] {
            Event::End(TagEnd::List(_)) => break,
            Event::Start(Tag::Item) => {
                *idx += 1;
                let mut item_tokens = Vec::new();
                while *idx < events.len() {
                    match &events[*idx] {
                        Event::End(TagEnd::Item) => {
                            *idx += 1;
                            break;
                        }
                        Event::Start(Tag::Paragraph) => {
                            tight = false;
                            *idx += 1;
                            if !item_tokens.is_empty() {
                                item_tokens.push(InlineToken::HardBreak);
                            }
                            let toks = collect_inline_tokens(events, idx, InlineStyle::default());
                            item_tokens.extend(toks);
                            skip_end(events, idx, TagEnd::Paragraph);
                        }
                        Event::Start(tag @ Tag::List(_)) => {
                            *idx += 1;
                            let (nested_items, _) = collect_list_items(events, idx);
                            skip_end(events, idx, tag.to_end());
                            for nested in nested_items {
                                if !item_tokens.is_empty() {
                                    item_tokens.push(InlineToken::HardBreak);
                                }
                                item_tokens.push(plain_text_token("  • "));
                                item_tokens.extend(nested);
                            }
                        }
                        _ => {
                            let before = *idx;
                            let toks = collect_inline_tokens(events, idx, InlineStyle::default());
                            item_tokens.extend(toks);
                            // collect_inline_tokens stops before unmatched End events
                            // (e.g. a blockquote nested in the item); skip them so the
                            // loop always makes progress.
                            if *idx == before {
                                *idx += 1;
                            }
                        }
                    }
                }
                items.push(item_tokens);
            }
            _ => {
                *idx += 1;
            }
        }
    }
    (items, tight)
}

fn collect_definition_list(events: &[Event<'_>], idx: &mut usize) -> Vec<Vec<InlineToken>> {
    let mut items = Vec::new();
    let mut current_title = String::new();

    while *idx < events.len() {
        match &events[*idx] {
            Event::End(TagEnd::DefinitionList) => break,
            Event::Start(Tag::DefinitionListTitle) => {
                *idx += 1;
                current_title =
                    flatten_tokens(&collect_inline_tokens(events, idx, InlineStyle::default()));
                skip_end(events, idx, TagEnd::DefinitionListTitle);
            }
            Event::Start(Tag::DefinitionListDefinition) => {
                *idx += 1;
                let blocks = parse_block_list(events, idx);
                skip_end(events, idx, TagEnd::DefinitionListDefinition);
                let definition = flatten_blocks_to_text(&blocks).trim().to_string();
                if !current_title.trim().is_empty() || !definition.is_empty() {
                    items.push(vec![plain_text_token(&format!(
                        "{} — {}",
                        current_title.trim(),
                        definition
                    ))]);
                }
            }
            _ => *idx += 1,
        }
    }
    items
}

fn collect_table(
    events: &[Event<'_>],
    idx: &mut usize,
) -> (Vec<Vec<InlineToken>>, Vec<Vec<Vec<InlineToken>>>) {
    let mut header: Vec<Vec<InlineToken>> = Vec::new();
    let mut rows: Vec<Vec<Vec<InlineToken>>> = Vec::new();
    let mut in_head = false;

    while *idx < events.len() {
        match &events[*idx] {
            Event::End(TagEnd::Table) => break,
            Event::Start(Tag::TableHead) => {
                in_head = true;
                *idx += 1;
            }
            Event::End(TagEnd::TableHead) => {
                in_head = false;
                *idx += 1;
            }
            // Header cells appear directly inside TableHead (no TableRow wrapper)
            Event::Start(Tag::TableCell) if in_head => {
                *idx += 1;
                let cell = collect_inline_tokens(events, idx, InlineStyle::default());
                skip_end(events, idx, TagEnd::TableCell);
                header.push(cell);
            }
            // Body rows
            Event::Start(Tag::TableRow) => {
                *idx += 1;
                let mut row = Vec::new();
                while *idx < events.len() {
                    match &events[*idx] {
                        Event::End(TagEnd::TableRow) => {
                            *idx += 1;
                            break;
                        }
                        Event::Start(Tag::TableCell) => {
                            *idx += 1;
                            let cell = collect_inline_tokens(events, idx, InlineStyle::default());
                            skip_end(events, idx, TagEnd::TableCell);
                            row.push(cell);
                        }
                        _ => {
                            *idx += 1;
                        }
                    }
                }
                if !row.is_empty() {
                    rows.push(row);
                }
            }
            _ => {
                *idx += 1;
            }
        }
    }
    (header, rows)
}

// ── AST utilities ─────────────────────────────────────────────────────────────

fn warn_math_failed(err: &str) {
    eprintln!(
        "Warning: failed to render math: {}",
        crate::cli::sanitize_control_chars(err)
    );
}

fn flatten_tokens(tokens: &[InlineToken]) -> String {
    let mut s = String::new();
    for t in tokens {
        match t {
            InlineToken::Text { text, .. } => s.push_str(text),
            InlineToken::SoftBreak | InlineToken::HardBreak => s.push('\n'),
            InlineToken::Image { .. } => {}
            InlineToken::Math { text, .. } => s.push_str(text),
        }
    }
    s
}

fn flatten_blocks_to_text(blocks: &[Block]) -> String {
    let mut s = String::new();
    for b in blocks {
        match b {
            Block::Paragraph(tokens) | Block::Heading { tokens, .. } => {
                s.push_str(&flatten_tokens(tokens));
                s.push('\n');
            }
            Block::BlockQuote(children) => {
                s.push_str(&flatten_blocks_to_text(children));
            }
            Block::Code { text, .. } => {
                s.push_str(text);
                s.push('\n');
            }
            Block::Math(text) => {
                s.push_str(text);
                s.push('\n');
            }
            _ => {}
        }
    }
    s
}

// ── Renderer ──────────────────────────────────────────────────────────────────

// The laid-out document: blocks positioned in document coordinates on a canvas
// `width` pixels wide and `height` pixels tall.
struct MarkdownLayout {
    blocks: Vec<RenderBlock>,
    width: u32,
    height: u32,
}

fn layout_document(
    blocks: &[Block],
    base_dir: &Path,
    font_system: &mut FontSystem,
    width: u32,
    font_size: f64,
) -> Result<MarkdownLayout, Box<dyn std::error::Error>> {
    let mut doc = DocBuilder {
        font_system,
        base_dir,
        image_cache: HashMap::new(),
        font_size,
        content_width: width.saturating_sub(DEFAULT_MARKDOWN_MARGIN * 2),
        y: DEFAULT_MARKDOWN_MARGIN,
        blocks: Vec::new(),
    };
    for block in blocks {
        match block {
            Block::Heading { level, tokens } => doc.heading(*level, tokens),
            Block::Paragraph(tokens) => doc.paragraph(tokens)?,
            Block::List {
                items,
                ordered,
                tight,
            } => doc.list(items, *ordered, *tight)?,
            Block::BlockQuote(children) => doc.block_quote(children),
            Block::Code { lang, text } => doc.code(lang, text),
            Block::Math(text) => doc.math(text, true),
            Block::Rule => doc.rule(),
            Block::Table { header, rows } => doc.table(header, rows)?,
        }
    }
    Ok(doc.finish(width))
}

// Positions blocks top to bottom; `y` is where the next block starts.
struct DocBuilder<'a> {
    font_system: &'a mut FontSystem,
    base_dir: &'a Path,
    image_cache: HashMap<PathBuf, DynamicImage>,
    font_size: f64,
    content_width: u32,
    y: u32,
    blocks: Vec<RenderBlock>,
}

impl DocBuilder<'_> {
    fn heading(&mut self, level: u32, tokens: &[InlineToken]) {
        let scale = HEADING_SCALE[level.clamp(1, 5) as usize - 1];
        let size = (self.font_size * scale) as f32;
        let bold = InlineStyle {
            bold: true,
            ..InlineStyle::default()
        };
        let text = flatten_tokens(tokens);
        let layout = layout_text(self.font_system, &text, self.content_width, size, bold);
        self.y += self.em(HEADING_SPACE_ABOVE_EM);
        let block = RenderBlock::Text {
            layout,
            x: DEFAULT_MARKDOWN_MARGIN,
            y: self.y,
        };
        self.push_advance(block, self.em(HEADING_SPACE_BELOW_EM));
    }

    fn paragraph(&mut self, tokens: &[InlineToken]) -> Result<(), Box<dyn std::error::Error>> {
        let non_empty: Vec<_> = tokens
            .iter()
            .filter(|t| !matches!(t, InlineToken::SoftBreak | InlineToken::HardBreak))
            .collect();
        match non_empty.as_slice() {
            // Solo image paragraph: render the image centered
            [InlineToken::Image { path }] => {
                let resolved = resolve_image_path(self.base_dir, path);
                if let Some(image) = load_inline_image(&resolved, &mut self.image_cache)? {
                    self.push_centered_image(scale_markdown_image_to_width(
                        &image,
                        self.content_width,
                    ));
                }
            }
            [InlineToken::Math { text, display }] => self.math(text, *display),
            _ if !flatten_tokens(tokens).trim().is_empty()
                || tokens
                    .iter()
                    .any(|t| matches!(t, InlineToken::Image { .. })) =>
            {
                let layout = self.layout_inline(tokens, self.content_width, false)?;
                let block = RenderBlock::Inline {
                    layout,
                    x: DEFAULT_MARKDOWN_MARGIN,
                    y: self.y,
                };
                self.push_advance(block, self.em(PARAGRAPH_GAP_EM));
            }
            _ => {}
        }
        Ok(())
    }

    fn list(
        &mut self,
        items: &[Vec<InlineToken>],
        ordered: bool,
        tight: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let item_gap = self.em(if tight {
            LIST_ITEM_GAP_TIGHT_EM
        } else {
            LIST_ITEM_GAP_LOOSE_EM
        });
        for (item_idx, item) in items.iter().enumerate() {
            let prefix = if ordered {
                format!("{}.", item_idx + 1)
            } else {
                "•".to_string()
            };
            let marker = layout_text(
                self.font_system,
                &prefix,
                LIST_MARKER_WIDTH,
                self.font_size as f32,
                InlineStyle::default(),
            );
            let content =
                self.layout_inline(item, self.content_width.saturating_sub(LIST_INDENT), false)?;
            let content_height = content.height;
            self.blocks.push(RenderBlock::Text {
                layout: marker,
                x: DEFAULT_MARKDOWN_MARGIN,
                y: self.y,
            });
            self.blocks.push(RenderBlock::Inline {
                layout: content,
                x: DEFAULT_MARKDOWN_MARGIN + LIST_INDENT,
                y: self.y,
            });
            self.y += content_height.max(self.font_size as u32 + LIST_ITEM_MIN_EXTRA);
            if item_idx + 1 < items.len() {
                self.y += item_gap;
            }
        }
        self.y += self.em(LIST_GAP_EM);
        Ok(())
    }

    fn block_quote(&mut self, children: &[Block]) {
        let content_text = flatten_blocks_to_text(children);
        let trimmed = content_text.trim();
        if trimmed.is_empty() {
            self.y += EMPTY_QUOTE_ADVANCE;
            return;
        }
        let layout = layout_text(
            self.font_system,
            trimmed,
            self.content_width.saturating_sub(QUOTE_INDENT),
            self.font_size as f32,
            InlineStyle::default(),
        );
        self.blocks.push(RenderBlock::Rect {
            x: DEFAULT_MARKDOWN_MARGIN,
            y: self.y,
            width: QUOTE_BAR_WIDTH,
            height: layout.height.max(self.font_size as u32),
            color: QUOTE_BAR_COLOR,
        });
        let block = RenderBlock::Text {
            layout,
            x: DEFAULT_MARKDOWN_MARGIN + QUOTE_INDENT,
            y: self.y,
        };
        self.push_advance(block, self.em(QUOTE_GAP_EM));
    }

    fn code(&mut self, lang: &str, text: &str) {
        if lang.trim().eq_ignore_ascii_case("mermaid") {
            match mermaid::render_mermaid(
                text,
                self.font_system,
                self.content_width,
                self.font_size as f32,
            ) {
                // render_mermaid sizes the diagram for content_width itself.
                Ok(image) => return self.push_centered_image(image),
                // Show the diagram source as a code block instead of
                // dropping the rest of the document.
                Err(err) => eprintln!(
                    "Warning: failed to render Mermaid diagram: {}",
                    crate::cli::sanitize_control_chars(&err)
                ),
            }
        }
        let layout = layout_code(
            self.font_system,
            lang,
            text,
            self.content_width.saturating_sub(CODE_PADDING * 2),
            (self.font_size * CODE_FONT_EM) as f32,
        );
        let block = RenderBlock::Code {
            layout,
            x: DEFAULT_MARKDOWN_MARGIN,
            y: self.y,
            width: self.content_width,
        };
        self.push_advance(block, CODE_BLOCK_GAP);
    }

    fn math(&mut self, text: &str, display: bool) {
        match math::render_math(text, self.font_system, self.font_size as f32, display) {
            Ok(rendered) => self.push_centered_image(scale_markdown_image_to_width(
                &rendered.image,
                self.content_width,
            )),
            // Show the LaTeX source instead of dropping the rest of the document.
            Err(err) => {
                warn_math_failed(&err);
                self.code("latex", text);
            }
        }
    }

    fn rule(&mut self) {
        self.blocks.push(RenderBlock::Rule {
            x: DEFAULT_MARKDOWN_MARGIN,
            y: self.y + RULE_OFFSET,
            width: self.content_width,
        });
        self.y += RULE_ADVANCE;
    }

    fn table(
        &mut self,
        header: &[Vec<InlineToken>],
        rows: &[Vec<Vec<InlineToken>>],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let n_cols = header
            .len()
            .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
        if n_cols == 0 {
            return Ok(());
        }
        let geometry = table_geometry(n_cols, self.content_width, self.font_size);
        let all_rows =
            std::iter::once((header, true)).chain(rows.iter().map(|row| (row.as_slice(), false)));
        for (row, is_header) in all_rows {
            let mut cells = Vec::with_capacity(n_cols);
            for cell in row.iter().take(n_cols) {
                cells.push(self.layout_inline(cell, geometry.cell_inner, is_header)?);
            }
            // Padding cells added for short rows do not count toward the row height.
            let content_height = cells.iter().map(|cell| cell.height).max().unwrap_or(0);
            while cells.len() < n_cols {
                cells.push(self.layout_inline(&[], geometry.cell_inner, false)?);
            }
            let row_height = content_height + geometry.padding * 2;
            self.table_row(cells, row_height, is_header, &geometry);
        }
        self.blocks.push(RenderBlock::Rect {
            x: DEFAULT_MARKDOWN_MARGIN,
            y: self.y,
            width: geometry.width,
            height: TABLE_BORDER,
            color: TABLE_BORDER_COLOR,
        });
        self.y += TABLE_BORDER + self.em(TABLE_GAP_EM);
        Ok(())
    }

    // Emits one row's top border, optional header shading, cells, and left and
    // right borders; the table's bottom border is emitted once by `table`.
    fn table_row(
        &mut self,
        cells: Vec<InlineLayout>,
        row_height: u32,
        is_header: bool,
        geometry: &TableGeometry,
    ) {
        let y = self.y;
        let border = |x: u32| RenderBlock::Rect {
            x,
            y,
            width: TABLE_BORDER,
            height: row_height + TABLE_BORDER,
            color: TABLE_BORDER_COLOR,
        };
        self.blocks.push(RenderBlock::Rect {
            x: DEFAULT_MARKDOWN_MARGIN,
            y,
            width: geometry.width,
            height: TABLE_BORDER,
            color: TABLE_BORDER_COLOR,
        });
        if is_header {
            self.blocks.push(RenderBlock::Rect {
                x: DEFAULT_MARKDOWN_MARGIN + TABLE_BORDER,
                y: y + TABLE_BORDER,
                width: geometry.width.saturating_sub(2 * TABLE_BORDER),
                height: row_height,
                color: TABLE_HEADER_BG,
            });
        }
        let mut x = DEFAULT_MARKDOWN_MARGIN;
        for layout in cells {
            self.blocks.push(border(x));
            x += TABLE_BORDER;
            self.blocks.push(RenderBlock::Inline {
                layout,
                x: x + geometry.padding,
                y: y + TABLE_BORDER + geometry.padding,
            });
            x += geometry.col_width;
        }
        self.blocks.push(border(x));
        self.y += TABLE_BORDER + row_height;
    }

    fn finish(self, width: u32) -> MarkdownLayout {
        MarkdownLayout {
            blocks: self.blocks,
            width,
            height: (self.y + DEFAULT_MARKDOWN_MARGIN)
                .max(DEFAULT_MARKDOWN_MARGIN + MIN_BODY_HEIGHT),
        }
    }

    // `k` ems in whole pixels, truncated like every other block spacing.
    fn em(&self, k: f64) -> u32 {
        (self.font_size * k) as u32
    }

    fn layout_inline(
        &mut self,
        tokens: &[InlineToken],
        width: u32,
        bold: bool,
    ) -> Result<InlineLayout, Box<dyn std::error::Error>> {
        layout_inline_tokens(
            self.font_system,
            tokens,
            width,
            self.font_size as f32,
            bold,
            self.base_dir,
            &mut self.image_cache,
        )
    }

    fn push_advance(&mut self, block: RenderBlock, gap: u32) {
        self.y += block.height() + gap;
        self.blocks.push(block);
    }

    fn push_centered_image(&mut self, image: DynamicImage) {
        let x = DEFAULT_MARKDOWN_MARGIN + self.content_width.saturating_sub(image.width()) / 2;
        let block = RenderBlock::Image {
            image,
            x,
            y: self.y,
        };
        self.push_advance(block, self.em(IMAGE_GAP_EM));
    }
}

struct TableGeometry {
    col_width: u32,
    // Width available to cell content inside the padding.
    cell_inner: u32,
    padding: u32,
    width: u32,
}

fn table_geometry(n_cols: usize, content_width: u32, font_size: f64) -> TableGeometry {
    let n_cols = n_cols as u32;
    let padding = (font_size * TABLE_PADDING_EM).max(TABLE_MIN_PADDING) as u32;
    let total_borders = TABLE_BORDER * (n_cols + 1);
    let col_width = (content_width.saturating_sub(total_borders) / n_cols).max(TABLE_MIN_COL_WIDTH);
    TableGeometry {
        col_width,
        cell_inner: col_width.saturating_sub(padding * 2),
        padding,
        width: col_width * n_cols + total_borders,
    }
}

// Draw the 1-based `page` of `layout` into a pixmap of at most `page_height`
// rows; only blocks overlapping that slice of the document are drawn.
fn render_page(
    layout: &MarkdownLayout,
    font_system: &mut FontSystem,
    swash: &mut SwashCache,
    page: usize,
    page_height: u32,
) -> Result<DynamicImage, Box<dyn std::error::Error>> {
    let y_start = (page - 1) as u32 * page_height;
    let draw_height = layout.height.saturating_sub(y_start).min(page_height);
    let width = layout.width;
    let mut pixmap = Pixmap::new(width, draw_height).ok_or("failed to allocate pixmap")?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let y_end = y_start.saturating_add(draw_height);
    for block in &layout.blocks {
        let bt = block.y_top();
        let bb = bt.saturating_add(block.height());
        if bb <= y_start || bt >= y_end {
            continue;
        }
        let visible_top = y_start.max(bt) - bt;
        let visible_bottom = y_end.min(bb) - bt;
        block.draw_with_offset(
            &mut pixmap,
            font_system,
            swash,
            y_start as i32,
            visible_top,
            visible_bottom,
        );
    }
    let data = pixmap.data().to_vec();
    let canvas = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, draw_height, data)
        .ok_or("pixmap to ImageBuffer conversion failed")?;
    Ok(DynamicImage::ImageRgba8(canvas))
}

// ── Syntax highlighting ───────────────────────────────────────────────────────

fn highlight_code_spans(lang: &str, text: &str) -> Vec<(SyntaxStyle, String)> {
    use syntect::easy::HighlightLines;
    use syntect::util::LinesWithEndings;

    let ss = SYNTAX_SET.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines);
    let ts = THEME_SET.get_or_init(syntect::highlighting::ThemeSet::load_defaults);
    let theme = &ts.themes["InspiredGitHub"];
    let syntax = ss
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut hl = HighlightLines::new(syntax, theme);
    let mut result = Vec::new();
    for line in LinesWithEndings::from(text) {
        match hl.highlight_line(line, ss) {
            Ok(ranges) => {
                result.extend(ranges.into_iter().map(|(style, s)| (style, s.to_string())));
            }
            Err(_) => {
                let plain = SyntaxStyle {
                    foreground: syntect::highlighting::Color {
                        r: 50,
                        g: 50,
                        b: 50,
                        a: 255,
                    },
                    ..SyntaxStyle::default()
                };
                result.push((plain, line.to_string()));
            }
        }
    }
    result
}

// ── Render primitives ─────────────────────────────────────────────────────────

#[derive(Debug)]
struct TextLayout {
    width: u32,
    height: u32,
    buffer: Buffer,
    color: Color,
}

#[derive(Debug)]
struct InlineLayout {
    height: u32,
    items: Vec<InlineRenderItem>,
}

#[derive(Debug)]
enum InlineRenderItem {
    Text { layout: TextLayout, x: u32, y: u32 },
    Image { image: DynamicImage, x: u32, y: u32 },
}

fn layout_text(
    font_system: &mut FontSystem,
    text: &str,
    width: u32,
    size: f32,
    style: InlineStyle,
) -> TextLayout {
    let mut buffer = Buffer::new(font_system, Metrics::new(size, size * LINE_HEIGHT_EM));
    buffer.set_size(Some(width as f32), None);
    set_buffer_text(&mut buffer, text, &inline_attrs(style));
    buffer.shape_until_scroll(font_system, false);
    text_layout_from_buffer(buffer, Color::rgb(0, 0, 0))
}

fn layout_code(
    font_system: &mut FontSystem,
    lang: &str,
    text: &str,
    width: u32,
    size: f32,
) -> TextLayout {
    let spans = highlight_code_spans(lang, text);
    let mut buffer = Buffer::new(font_system, Metrics::new(size, size * LINE_HEIGHT_EM));
    buffer.set_size(Some(width as f32), None);
    let rich: Vec<(&str, Attrs)> = spans
        .iter()
        .flat_map(|(style, s)| {
            let fg = style.foreground;
            let mut a = Attrs::new()
                .family(Family::Monospace)
                .color(Color::rgb(fg.r, fg.g, fg.b));
            if style.font_style.contains(FontStyle::BOLD) {
                a = a.weight(Weight::BOLD);
            }
            route_monospace_cjk(s, &a)
        })
        .collect();
    let default_attrs = Attrs::new().family(Family::Monospace);
    buffer.set_rich_text(rich, &default_attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    text_layout_from_buffer(buffer, CODE_TEXT_COLOR)
}

fn set_buffer_text(buffer: &mut Buffer, text: &str, attrs: &Attrs) {
    if has_monospace_cjk(text, attrs) {
        let spans = route_monospace_cjk(text, attrs);
        buffer.set_rich_text(spans, attrs, Shaping::Advanced, None);
    } else {
        buffer.set_text(text, attrs, Shaping::Advanced, None);
    }
}

// cosmic-text's monospace fallback picks any monospaced CJK face (BIZ UDGothic
// on macOS) before the configured CJK family, so code would mix two CJK
// typefaces. Send CJK runs to SansSerif, which resolve_fonts() points at the
// preferred CJK family (PingFang SC on macOS, Noto Sans CJK SC on Linux).
fn route_monospace_cjk<'t, 'a>(text: &'t str, attrs: &Attrs<'a>) -> Vec<(&'t str, Attrs<'a>)> {
    if !has_monospace_cjk(text, attrs) {
        return vec![(text, attrs.clone())];
    }
    fonts::cjk_runs(text)
        .map(|(run, cjk)| {
            let family = if cjk {
                Family::SansSerif
            } else {
                Family::Monospace
            };
            (run, attrs.clone().family(family))
        })
        .collect()
}

fn has_monospace_cjk(text: &str, attrs: &Attrs) -> bool {
    attrs.family == Family::Monospace && text.chars().any(fonts::is_cjk_char)
}

fn inline_attrs(style: InlineStyle) -> Attrs<'static> {
    let mut attrs = Attrs::new().family(if style.mono {
        Family::Monospace
    } else {
        Family::SansSerif
    });
    if style.bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if style.italic {
        attrs = attrs.style(Style::Italic);
    }
    if let Some(color) = style.color {
        attrs = attrs.color(Color::rgb(
            ((color >> 16) & 0xff) as u8,
            ((color >> 8) & 0xff) as u8,
            (color & 0xff) as u8,
        ));
    }
    if style.underline {
        attrs = attrs.underline(UnderlineStyle::Single);
    }
    attrs
}

fn text_layout_from_buffer(buffer: Buffer, color: Color) -> TextLayout {
    let mut text_width = 0_f32;
    let mut text_height = 0_f32;
    for run in buffer.layout_runs() {
        text_width = text_width.max(run.line_w);
        text_height = text_height.max(run.line_top + run.line_height);
    }
    TextLayout {
        width: text_width.ceil() as u32,
        height: text_height.ceil() as u32,
        buffer,
        color,
    }
}

fn layout_inline_tokens(
    font_system: &mut FontSystem,
    tokens: &[InlineToken],
    width: u32,
    font_size: f32,
    default_bold: bool,
    base_dir: &Path,
    image_cache: &mut HashMap<PathBuf, DynamicImage>,
) -> Result<InlineLayout, Box<dyn std::error::Error>> {
    let line_height = (font_size * LINE_HEIGHT_EM).ceil() as u32;
    let mut lines: Vec<InlineLine> = vec![InlineLine::default()];

    for token in tokens {
        match token {
            InlineToken::Text { text, style } => {
                let style = InlineStyle {
                    bold: style.bold || default_bold,
                    ..*style
                };
                push_inline_words(font_system, &mut lines, text, width, font_size, style);
            }
            InlineToken::Math { text, display } => {
                match math::render_math(text, font_system, font_size, *display) {
                    Ok(rendered) => {
                        push_inline_image(&mut lines, rendered.image, rendered.baseline, width)
                    }
                    Err(err) => {
                        warn_math_failed(&err);
                        let style = InlineStyle {
                            mono: true,
                            ..InlineStyle::default()
                        };
                        push_inline_words(font_system, &mut lines, text, width, font_size, style);
                    }
                }
            }
            InlineToken::SoftBreak => {
                let space_width = cached_space_width(font_system, font_size);
                lines.last_mut().unwrap().width += space_width;
            }
            InlineToken::HardBreak => lines.push(InlineLine::default()),
            InlineToken::Image { path } => {
                let resolved = resolve_image_path(base_dir, path);
                if let Some(image) = load_inline_image(&resolved, image_cache)? {
                    let baseline = image.height();
                    push_inline_image(&mut lines, image, baseline, width);
                }
            }
        }
    }

    let mut items = Vec::new();
    let mut y = 0_u32;
    for line in lines {
        if line.items.is_empty() {
            y += line_height;
            continue;
        }
        let baseline = line.baseline.max((font_size * INLINE_BASELINE_EM) as u32);
        let height = (baseline + line.descent).max(line_height);
        for item in line.items {
            match item {
                InlineLineItem::Text {
                    layout,
                    x,
                    baseline: item_baseline,
                } => items.push(InlineRenderItem::Text {
                    layout,
                    x,
                    y: y + baseline.saturating_sub(item_baseline),
                }),
                InlineLineItem::Image {
                    image,
                    x,
                    baseline: item_baseline,
                } => {
                    items.push(InlineRenderItem::Image {
                        image,
                        x,
                        y: y + baseline.saturating_sub(item_baseline),
                    });
                }
            }
        }
        y += height;
    }

    Ok(InlineLayout { height: y, items })
}

#[derive(Default)]
struct InlineLine {
    width: u32,
    baseline: u32,
    descent: u32,
    items: Vec<InlineLineItem>,
}

enum InlineLineItem {
    Text {
        layout: TextLayout,
        x: u32,
        baseline: u32,
    },
    Image {
        image: DynamicImage,
        x: u32,
        baseline: u32,
    },
}

// Lay out one styled text token: split it into wrap segments, measure them in a
// single shaping pass, then emit one merged buffer per line instead of one
// buffer per segment.
fn push_inline_words(
    font_system: &mut FontSystem,
    lines: &mut Vec<InlineLine>,
    text: &str,
    max_width: u32,
    font_size: f32,
    style: InlineStyle,
) {
    let segments = split_inline_segments(text, style.mono);
    if segments.is_empty() {
        return;
    }

    let widths = measure_segment_widths(font_system, &segments, font_size, style);
    let mut i = 0;
    while i < segments.len() {
        if lines.last().unwrap().width > 0
            && lines.last().unwrap().width.saturating_add(widths[i]) > max_width
        {
            lines.push(InlineLine::default());
        }
        let avail = max_width.saturating_sub(lines.last().unwrap().width);
        let start = i;
        let mut total = widths[i];
        i += 1;
        while i < segments.len() && total.saturating_add(widths[i]) <= avail {
            total += widths[i];
            i += 1;
        }
        let segment: String = segments[start..i].concat();
        // The segment was measured to fit on one line, so lay it out unbounded;
        // a lone word wider than the content width keeps wrapping internally.
        let layout_width = if i - start == 1 && widths[start] > max_width {
            max_width
        } else {
            u32::MAX / 2
        };
        let layout = layout_text(font_system, &segment, layout_width, font_size, style);
        let line = lines.last_mut().unwrap();
        let x = line.width;
        line.width = line.width.saturating_add(layout.width);
        let baseline = (font_size * INLINE_BASELINE_EM) as u32;
        line.baseline = line.baseline.max(baseline);
        line.descent = line.descent.max(layout.height.saturating_sub(baseline));
        line.items.push(InlineLineItem::Text {
            layout,
            x,
            baseline,
        });
    }
}

fn split_inline_segments(text: &str, preserve_whitespace: bool) -> Vec<String> {
    if preserve_whitespace {
        let mut segments = Vec::new();
        let mut current = String::new();
        let mut in_whitespace = false;
        for ch in text.chars() {
            if ch.is_whitespace() {
                current.push(ch);
                in_whitespace = true;
            } else {
                if in_whitespace {
                    segments.push(std::mem::take(&mut current));
                }
                current.push(ch);
                in_whitespace = false;
            }
        }
        if !current.is_empty() {
            segments.push(current);
        }
        return segments;
    }

    // Markdown collapses ASCII prose whitespace to one space. Keep Unicode
    // spacing characters intact because they can carry width or no-break semantics.
    text.split_inclusive(|ch: char| ch.is_ascii_whitespace())
        .enumerate()
        .filter_map(|(idx, part)| {
            let part = if idx == 0 {
                part
            } else {
                part.trim_start_matches(|ch: char| ch.is_ascii_whitespace())
            };
            if part.is_empty() {
                return None;
            }
            Some(
                match part.strip_suffix(|ch: char| ch.is_ascii_whitespace()) {
                    Some(stripped) => format!("{stripped} "),
                    None => part.to_string(),
                },
            )
        })
        .collect()
}

// Measure every segment with one shaping pass, then assign each glyph's advance
// to the segment owning its byte range.
fn measure_segment_widths(
    font_system: &mut FontSystem,
    segments: &[String],
    font_size: f32,
    style: InlineStyle,
) -> Vec<u32> {
    let joined: String = segments.concat();
    let mut buffer = Buffer::new(
        font_system,
        Metrics::new(font_size, font_size * LINE_HEIGHT_EM),
    );
    buffer.set_size(None, None);
    set_buffer_text(&mut buffer, &joined, &inline_attrs(style));
    buffer.shape_until_scroll(font_system, false);

    let mut starts = Vec::with_capacity(segments.len());
    let mut offset = 0;
    for segment in segments {
        starts.push(offset);
        offset += segment.len();
    }

    let mut widths = vec![0.0_f32; segments.len()];
    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            let segment_idx = starts.partition_point(|&s| s <= glyph.start) - 1;
            widths[segment_idx] += glyph.w;
        }
    }
    widths.into_iter().map(|w| w.ceil() as u32).collect()
}

// Space width only depends on the font size; cache it instead of re-measuring
// for every paragraph and table cell.
static SPACE_WIDTH_CACHE: OnceLock<Mutex<HashMap<u32, u32>>> = OnceLock::new();

fn cached_space_width(font_system: &mut FontSystem, font_size: f32) -> u32 {
    let cache = SPACE_WIDTH_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = font_size.to_bits();
    if let Some(width) = cache.lock().unwrap().get(&key) {
        return *width;
    }
    let width = layout_text(
        font_system,
        " ",
        u32::MAX / 2,
        font_size,
        InlineStyle::default(),
    )
    .width;
    cache.lock().unwrap().insert(key, width);
    width
}

fn push_inline_image(
    lines: &mut Vec<InlineLine>,
    mut image: DynamicImage,
    mut baseline: u32,
    max_width: u32,
) {
    if image.width() > max_width && max_width > 0 {
        let original_height = image.height().max(1);
        image = scale_markdown_image_to_width(&image, max_width);
        baseline = ((baseline as f32 * image.height() as f32 / original_height as f32).round()
            as u32)
            .clamp(1, image.height().max(1));
    }

    if lines.last().unwrap().width > 0
        && lines.last().unwrap().width.saturating_add(image.width()) > max_width
    {
        lines.push(InlineLine::default());
    }
    let line = lines.last_mut().unwrap();
    let x = line.width;
    line.width = line.width.saturating_add(image.width());
    line.baseline = line.baseline.max(baseline);
    line.descent = line.descent.max(image.height().saturating_sub(baseline));
    line.items
        .push(InlineLineItem::Image { image, x, baseline });
}

#[derive(Debug)]
enum RenderBlock {
    Text {
        layout: TextLayout,
        x: u32,
        y: u32,
    },
    Inline {
        layout: InlineLayout,
        x: u32,
        y: u32,
    },
    Code {
        layout: TextLayout,
        x: u32,
        y: u32,
        width: u32,
    },
    Image {
        image: DynamicImage,
        x: u32,
        y: u32,
    },
    Rule {
        x: u32,
        y: u32,
        width: u32,
    },
    Rect {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: Rgba<u8>,
    },
}

impl RenderBlock {
    fn height(&self) -> u32 {
        match self {
            Self::Text { layout, .. } => layout.height,
            Self::Inline { layout, .. } => layout.height,
            Self::Code { layout, .. } => layout.height + CODE_PADDING * 2 + CODE_BOTTOM_EXTRA,
            Self::Image { image, .. } => image.height(),
            Self::Rule { .. } => RULE_THICKNESS,
            Self::Rect { height, .. } => *height,
        }
    }

    fn y_top(&self) -> u32 {
        match self {
            Self::Text { y, .. }
            | Self::Inline { y, .. }
            | Self::Code { y, .. }
            | Self::Image { y, .. }
            | Self::Rule { y, .. }
            | Self::Rect { y, .. } => *y,
        }
    }

    // Draw the block into `pixmap`, shifting all y-coordinates up by `y_offset`
    // (document y → pixmap y).  Handles partial visibility when a block straddles
    // a page boundary.
    fn draw_with_offset(
        &self,
        pixmap: &mut Pixmap,
        font_system: &mut FontSystem,
        swash: &mut SwashCache,
        y_offset: i32,
        visible_top: u32,
        visible_bottom: u32,
    ) {
        match self {
            Self::Text { layout, x, y } => {
                let mut ctx = MarkdownDrawContext {
                    pixmap,
                    font_system,
                    swash,
                };
                draw_text_layout(
                    &mut ctx,
                    layout,
                    *x as i32,
                    *y as i32 - y_offset,
                    visible_top,
                    visible_bottom,
                );
            }
            Self::Inline { layout, x, y } => {
                let mut ctx = MarkdownDrawContext {
                    pixmap,
                    font_system,
                    swash,
                };
                draw_inline_layout(
                    &mut ctx,
                    layout,
                    *x as i32,
                    *y as i32 - y_offset,
                    visible_top,
                    visible_bottom,
                );
            }
            Self::Code {
                layout,
                x,
                y,
                width,
            } => {
                let y_adj = *y as i32 - y_offset;
                fill_rect(
                    pixmap,
                    *x as i32,
                    y_adj,
                    *width,
                    layout.height + CODE_PADDING * 2 + CODE_BOTTOM_EXTRA,
                    CODE_BG,
                );
                let text_top = CODE_PADDING as i32;
                let text_visible_top = visible_top as i32 - text_top;
                let text_visible_bottom = visible_bottom as i32 - text_top;
                if text_visible_bottom > 0 && text_visible_top < layout.height as i32 {
                    let mut ctx = MarkdownDrawContext {
                        pixmap,
                        font_system,
                        swash,
                    };
                    draw_text_layout(
                        &mut ctx,
                        layout,
                        (*x + CODE_PADDING) as i32,
                        y_adj + text_top,
                        text_visible_top.max(0) as u32,
                        text_visible_bottom.min(layout.height as i32).max(0) as u32,
                    );
                }
            }
            Self::Image { image, x, y } => {
                sk_overlay_image_slice(
                    pixmap,
                    image,
                    *x as i32,
                    *y as i32 - y_offset,
                    visible_top,
                    visible_bottom,
                );
            }
            Self::Rule { x, y, width } => {
                fill_rect(
                    pixmap,
                    *x as i32,
                    *y as i32 - y_offset,
                    *width,
                    RULE_THICKNESS,
                    RULE_COLOR,
                );
            }
            Self::Rect {
                x,
                y,
                width,
                height,
                color,
            } => {
                fill_rect(
                    pixmap,
                    *x as i32,
                    *y as i32 - y_offset,
                    *width,
                    *height,
                    color.0,
                );
            }
        }
    }
}

struct MarkdownDrawContext<'a> {
    pixmap: &'a mut Pixmap,
    font_system: &'a mut FontSystem,
    swash: &'a mut SwashCache,
}

fn draw_text_layout(
    ctx: &mut MarkdownDrawContext<'_>,
    layout: &TextLayout,
    x: i32,
    y: i32,
    visible_top: u32,
    visible_bottom: u32,
) {
    if visible_top >= visible_bottom {
        return;
    }
    let mut renderer = TextPixmapRenderer {
        pixmap: &mut *ctx.pixmap,
        font_system: &mut *ctx.font_system,
        swash: &mut *ctx.swash,
        x,
        y,
    };
    let runs = layout
        .buffer
        .layout_runs()
        .skip_while(|run| run.line_top + run.line_height <= visible_top as f32)
        .take_while(|run| run.line_top < visible_bottom as f32);
    for run in runs {
        for glyph in run.glyphs {
            if glyph_is_missing(renderer.font_system, glyph) {
                continue;
            }
            let physical_glyph = glyph.physical((0.0, run.line_y), 1.0);
            let glyph_color = glyph.color_opt.unwrap_or(layout.color);
            renderer.glyph(physical_glyph, glyph_color);
        }
        render_decoration(&mut renderer, &run, layout.color);
    }
}

fn draw_inline_layout(
    ctx: &mut MarkdownDrawContext<'_>,
    layout: &InlineLayout,
    x: i32,
    y: i32,
    visible_top: u32,
    visible_bottom: u32,
) {
    if visible_top >= visible_bottom {
        return;
    }
    for item in &layout.items {
        match item {
            InlineRenderItem::Text {
                layout,
                x: item_x,
                y: item_y,
            } => {
                let item_top = *item_y;
                let item_bottom = item_top.saturating_add(layout.height);
                if item_bottom <= visible_top || item_top >= visible_bottom {
                    continue;
                }
                draw_text_layout(
                    ctx,
                    layout,
                    x + *item_x as i32,
                    y + *item_y as i32,
                    visible_top.saturating_sub(item_top),
                    visible_bottom.min(item_bottom) - item_top,
                );
            }
            InlineRenderItem::Image {
                image,
                x: item_x,
                y: item_y,
            } => {
                let item_top = *item_y;
                let item_bottom = item_top.saturating_add(image.height());
                if item_bottom <= visible_top || item_top >= visible_bottom {
                    continue;
                }
                sk_overlay_image_slice(
                    &mut *ctx.pixmap,
                    image,
                    x + *item_x as i32,
                    y + *item_y as i32,
                    visible_top.saturating_sub(item_top),
                    visible_bottom.min(item_bottom) - item_top,
                );
            }
        }
    }
}

fn glyph_is_missing(font_system: &FontSystem, glyph: &cosmic_text::LayoutGlyph) -> bool {
    glyph.glyph_id == 0
        && font_system
            .db()
            .face(glyph.font_id)
            .is_some_and(|face| !face.post_script_name.contains("Emoji"))
}

struct TextPixmapRenderer<'a> {
    pixmap: &'a mut Pixmap,
    font_system: &'a mut FontSystem,
    swash: &'a mut SwashCache,
    x: i32,
    y: i32,
}

impl Renderer for TextPixmapRenderer<'_> {
    fn rectangle(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        fill_rect(
            self.pixmap,
            self.x + x,
            self.y + y,
            width,
            height,
            color.as_rgba(),
        );
    }

    // Blends straight into the pixel buffer: a 1x1 fill_rect per glyph pixel
    // would build a tiny-skia pipeline for every pixel.
    fn glyph(&mut self, physical_glyph: PhysicalGlyph, color: Color) {
        let base_x = self.x + physical_glyph.x;
        let base_y = self.y + physical_glyph.y;
        let width = self.pixmap.width() as i32;
        let height = self.pixmap.height() as i32;
        let data = self.pixmap.data_mut();
        self.swash.with_pixels(
            self.font_system,
            physical_glyph.cache_key,
            color,
            |gx, gy, pixel_color| {
                let px = base_x + gx;
                let py = base_y + gy;
                if px < 0 || py < 0 || px >= width || py >= height {
                    return;
                }
                let idx = (py as usize * width as usize + px as usize) * 4;
                let dst: &mut [u8; 4] = (&mut data[idx..idx + 4]).try_into().unwrap();
                blend_source_over(dst, pixel_color.as_rgba());
            },
        );
    }
}

/// Matches tiny-skia 0.12 lowp SourceOver for a solid colour (see fill_rect).
/// `dst` is a premultiplied pixmap pixel; `rgba` is an unpremultiplied colour.
fn blend_source_over(dst: &mut [u8; 4], rgba: [u8; 4]) {
    let alpha = rgba[3];
    match alpha {
        0 => {}
        255 => *dst = rgba,
        _ => {
            // tiny-skia premultiplies in f32, rounds to u16, then blends each
            // channel with div255(v) = (v + 255) >> 8.
            let a = alpha as f32 / 255.0;
            let premul = |c: u8| ((c as f32 / 255.0 * a) * 255.0 + 0.5) as u16;
            let src = [
                premul(rgba[0]),
                premul(rgba[1]),
                premul(rgba[2]),
                (a * 255.0 + 0.5) as u16,
            ];
            let inv = 255 - src[3];
            for (d, s) in dst.iter_mut().zip(src) {
                *d = (s + ((*d as u16 * inv + 255) >> 8)) as u8;
            }
        }
    }
}

// tiny-skia clips the rectangle to the pixmap, so callers may pass blocks that
// straddle or lie outside the current page slice.
fn fill_rect(pixmap: &mut Pixmap, x: i32, y: i32, width: u32, height: u32, rgba: [u8; 4]) {
    let [r, g, b, a] = rgba;
    // Glyph rasterization reports fully transparent pixels too; skip them cheaply.
    if a == 0 {
        return;
    }
    let Some(rect) = SkRect::from_xywh(x as f32, y as f32, width as f32, height as f32) else {
        return;
    };
    let mut paint = SkPaint::default();
    paint.set_color_rgba8(r, g, b, a);
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

// Overlay only the visible source rows for images that straddle page boundaries.
fn sk_overlay_image_slice(
    pixmap: &mut Pixmap,
    image: &DynamicImage,
    x_off: i32,
    y_off: i32,
    visible_top: u32,
    visible_bottom: u32,
) {
    if visible_top >= visible_bottom {
        return;
    }
    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let iw = image.width();
    let ih = image.height();
    let src_y_start = visible_top.min(ih);
    let src_y_end = visible_bottom.min(ih);
    if src_y_start >= src_y_end || x_off >= pw {
        return;
    }
    let src_x_start = if x_off < 0 {
        (-x_off).min(iw as i32) as u32
    } else {
        0
    };
    let src_x_end = if x_off < pw {
        iw.min((pw - x_off) as u32)
    } else {
        0
    };
    if src_x_start >= src_x_end {
        return;
    }
    // Avoid DynamicImage::get_pixel's per-pixel enum dispatch: borrow the RGBA8
    // buffer directly (converting once only when the image is another format).
    let converted;
    let rgba = match image.as_rgba8() {
        Some(buffer) => buffer,
        None => {
            converted = image.to_rgba8();
            &converted
        }
    };
    let src_data = rgba.as_raw();
    let data = pixmap.data_mut();
    for src_y in src_y_start..src_y_end {
        let py = y_off + src_y as i32;
        if py < 0 || py >= ph {
            continue;
        }
        for src_x in src_x_start..src_x_end {
            let px = x_off + src_x as i32;
            if px < 0 || px >= pw {
                continue;
            }
            let src_idx = ((src_y * iw + src_x) * 4) as usize;
            let src = &src_data[src_idx..src_idx + 4];
            let a = src[3];
            if a == 0 {
                continue;
            }
            let idx = ((py * pw + px) * 4) as usize;
            if a == 255 {
                data[idx] = src[0];
                data[idx + 1] = src[1];
                data[idx + 2] = src[2];
                data[idx + 3] = 255;
            } else {
                let fa = a as u32;
                let ia = 255 - fa;
                data[idx] = ((src[0] as u32 * fa + data[idx] as u32 * ia) / 255) as u8;
                data[idx + 1] = ((src[1] as u32 * fa + data[idx + 1] as u32 * ia) / 255) as u8;
                data[idx + 2] = ((src[2] as u32 * fa + data[idx + 2] as u32 * ia) / 255) as u8;
                data[idx + 3] = 255;
            }
        }
    }
}

// ── Image helpers ─────────────────────────────────────────────────────────────

fn resolve_image_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

fn load_inline_image(
    path: &Path,
    cache: &mut HashMap<PathBuf, DynamicImage>,
) -> Result<Option<DynamicImage>, Box<dyn std::error::Error>> {
    if let Some(image) = cache.get(path) {
        return Ok(Some(image.clone()));
    }
    if !path.exists() {
        return Ok(None);
    }
    let data = imgutil::read_file(path)?;
    let image = imgutil::decode_with_limits(&data)?;
    cache.insert(path.to_path_buf(), image.clone());
    Ok(Some(image))
}

fn send_rendered_markdown(
    image: &DynamicImage,
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for rect in markdown_chunk_rects(image.width(), image.height()) {
        let mut chunk = image.crop_imm(rect.0, rect.1, rect.2, rect.3);
        let (width, height) =
            imgutil::fit_within(chunk.width(), chunk.height(), size.pixel_width, u32::MAX);
        if (width, height) != (chunk.width(), chunk.height()) {
            chunk = imgutil::scale(&chunk, width, height);
        }
        let png = imgutil::encode_png(&chunk)?;
        crate::kitty::send_static_image(&png, width, height, size, tmux)?;
    }
    Ok(())
}

fn scale_markdown_image_to_width(image: &DynamicImage, max_width: u32) -> DynamicImage {
    if image.width() <= max_width {
        image.clone()
    } else {
        image.resize(max_width, u32::MAX, imageops::FilterType::Triangle)
    }
}

// ── Public helpers ────────────────────────────────────────────────────────────

pub fn markdown_render_width(max_pixel_width: u32) -> u32 {
    if max_pixel_width == 0 {
        return DEFAULT_MARKDOWN_WIDTH;
    }
    max_pixel_width.clamp(MIN_MARKDOWN_WIDTH, DEFAULT_MARKDOWN_WIDTH)
}

pub fn markdown_base_dir(path: &str) -> PathBuf {
    Path::new(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

pub fn markdown_page_height(size: Size) -> u32 {
    size.image_area_height()
}

pub fn markdown_total_pages(total_height: u32, page_height: u32) -> usize {
    if total_height == 0 || page_height == 0 {
        1
    } else {
        total_height.div_ceil(page_height) as usize
    }
}

pub fn markdown_chunk_max_height(width: u32) -> u32 {
    ((imgutil::MAX_PIXELS / width as u64) as u32).clamp(1, MARKDOWN_CHUNK_HEIGHT)
}

pub fn markdown_chunk_rects(width: u32, height: u32) -> Vec<(u32, u32, u32, u32)> {
    let max_height = markdown_chunk_max_height(width);
    if height <= max_height {
        return vec![(0, 0, width, height)];
    }
    let mut rects = Vec::new();
    let mut top = 0;
    while top < height {
        let chunk_height = (height - top).min(max_height);
        rects.push((0, top, width, chunk_height));
        top += chunk_height;
    }
    rects
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImage, GenericImageView, ImageEncoder};
    use std::collections::BTreeSet;

    // Lays out `data` at the default font size and draws the whole document as
    // a single page.
    fn render_markdown(
        data: &[u8],
        base_dir: &Path,
        max_pixel_width: u32,
    ) -> Result<DynamicImage, Box<dyn std::error::Error>> {
        render_markdown_page(
            data,
            base_dir,
            max_pixel_width,
            DEFAULT_MARKDOWN_FONT_PT,
            1,
            None,
        )
    }

    // Lays out `data` and draws one page through the production pager slicing;
    // `page_height: None` draws the whole document as one page.
    fn render_markdown_page(
        data: &[u8],
        base_dir: &Path,
        max_pixel_width: u32,
        font_size: f64,
        page: usize,
        page_height: Option<u32>,
    ) -> Result<DynamicImage, Box<dyn std::error::Error>> {
        let blocks = parse_markdown_blocks(data);
        with_fonts(|font_system, swash| {
            let layout = layout_document(
                &blocks,
                base_dir,
                font_system,
                markdown_render_width(max_pixel_width),
                font_size,
            )?;
            let page_height = page_height.unwrap_or(layout.height);
            render_page(&layout, font_system, swash, page, page_height)
        })
    }

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

        assert!(found, "rendered Markdown should contain visible pixels");
        (left, top, right, bottom)
    }

    fn assert_rendered_content(image: &DynamicImage) {
        let (left, top, right, bottom) = non_white_bounds(image);
        assert!(
            right > left + 8 && bottom > top + 8,
            "visible content must occupy an area"
        );
        assert!(
            left > 0 && top > 0 && right + 1 < image.width() && bottom + 1 < image.height(),
            "content must fit inside the canvas: ({left}, {top})..({right}, {bottom}) in {}x{}",
            image.width(),
            image.height()
        );
    }

    #[test]
    fn render_markdown_produces_image() {
        let image = render_markdown(
            b"# Hello\n\n- one\n- two\n\n```go\nfmt.Println(\"hi\")\n```\n",
            Path::new(""),
            800,
        )
        .unwrap();
        assert_rendered_content(&image);
        assert_eq!(image.width(), 800);
    }

    #[test]
    fn render_markdown_produces_image_with_chinese_text() {
        let image = render_markdown(
            "# 中文\n\n| 银行 | 金额 |\n| --- | --- |\n| 建行 | 3381.96 |\n".as_bytes(),
            Path::new(""),
            800,
        )
        .unwrap();
        assert_rendered_content(&image);
        assert_eq!(image.width(), 800);
        assert!(image.height() > 0);
    }

    #[test]
    fn render_markdown_with_font_size_affects_layout() {
        let data = b"# Title\n\nParagraph text that wraps enough to make font size visible in layout.\n\n- one\n- two\n";
        let default = render_markdown(data, Path::new(""), 800).unwrap();
        let large = render_markdown_page(
            data,
            Path::new(""),
            800,
            DEFAULT_MARKDOWN_FONT_PT * 1.5,
            1,
            None,
        )
        .unwrap();
        assert!(large.height() > default.height());
    }

    #[test]
    fn render_markdown_with_math() {
        let md = br#"Inline $\sqrt[3]{x^3 + y^3}$ and $\binom{n}{k}$

$$
\begin{bmatrix}
a & b \\
c & d
\end{bmatrix}
$$
"#;
        let image = render_markdown(md, Path::new(""), 800).unwrap();
        assert_rendered_content(&image);
        assert_eq!(image.width(), 800);
        assert!(image.height() > 120);
    }

    #[test]
    fn render_markdown_with_mermaid() {
        let md = b"```mermaid\nflowchart TD\n    A[Start] --> B{Decision}\n    B -->|Yes| C[Continue]\n    B -->|No| D[Retry]\n```\n";
        let image = render_markdown(md, Path::new(""), 800).unwrap();
        assert_rendered_content(&image);
        assert_eq!(image.width(), 800);
        assert!(image.height() > 180);
    }

    #[test]
    fn render_markdown_with_array_math_uses_table_layout() {
        let md = br#"$$
\begin{array}{cc}
1 & 2 \\
3 & 4
\end{array}
$$
"#;
        let image = render_markdown(md, Path::new(""), 800).unwrap();

        assert_rendered_content(&image);
        assert_eq!(image.width(), 800);
        assert!(
            image.height() > 150,
            "array math should keep its two-dimensional layout"
        );
    }

    #[test]
    fn display_math_is_centered_in_markdown_canvas() {
        let md = br#"$$
\begin{bmatrix}
a & b \\
c & d
\end{bmatrix}
$$
"#;
        let image = render_markdown(md, Path::new(""), 800).unwrap();
        let (left, _, right, _) = non_white_bounds(&image);
        let ink_center = (left + right) as i32 / 2;
        let canvas_center = image.width() as i32 / 2;

        assert!(
            (ink_center - canvas_center).abs() <= 36,
            "display math should be centered: ink=({left}..{right}), canvas_width={}",
            image.width()
        );
    }

    #[test]
    fn long_inline_math_stays_inside_markdown_canvas() {
        let numerator = (1..=36)
            .map(|i| format!("a_{{{i}}}"))
            .collect::<Vec<_>>()
            .join(" + ");
        let denominator = (1..=36)
            .map(|i| format!("b_{{{i}}}"))
            .collect::<Vec<_>>()
            .join(" + ");
        let markdown = format!("Inline $\\sqrt{{\\frac{{{numerator}}}{{{denominator}}}}}$ after\n");
        let image = render_markdown(markdown.as_bytes(), Path::new(""), 800).unwrap();
        let (left, top, right, bottom) = non_white_bounds(&image);

        assert!(left > 0 && top > 0);
        assert!(
            right + 1 < image.width(),
            "long inline math should not be clipped at the right edge: bounds=({left},{top},{right},{bottom}), size={}x{}",
            image.width(),
            image.height()
        );
    }

    #[test]
    fn render_page_matches_non_first_code_slice() {
        let lines = (0..80)
            .map(|i| format!("let value_{i} = {i};\n"))
            .collect::<String>();
        let markdown = format!("```rust\n{lines}```\n");
        let page_height = 160;
        let full = render_markdown(markdown.as_bytes(), Path::new(""), 800).unwrap();
        assert!(full.height() > page_height * 2);

        let page = render_markdown_page(
            markdown.as_bytes(),
            Path::new(""),
            800,
            DEFAULT_MARKDOWN_FONT_PT,
            2,
            Some(page_height),
        )
        .unwrap();
        let expected = full.crop_imm(0, page_height, full.width(), page_height);

        assert_eq!(page.dimensions(), expected.dimensions());
        assert_eq!(page.to_rgba8().as_raw(), expected.to_rgba8().as_raw());
    }

    #[test]
    fn render_page_matches_non_first_image_slice() {
        let dir = tempfile::tempdir().unwrap();
        let image_path = dir.path().join("tall.png");
        let mut image = DynamicImage::new_rgba8(64, 360);
        for y in 0..image.height() {
            let color = if y < 120 {
                Rgba([220, 40, 40, 255])
            } else if y < 240 {
                Rgba([40, 160, 60, 255])
            } else {
                Rgba([40, 80, 220, 255])
            };
            for x in 0..image.width() {
                image.put_pixel(x, y, color);
            }
        }
        std::fs::write(&image_path, imgutil::encode_png(&image).unwrap()).unwrap();

        let markdown = b"![alt](tall.png)\n";
        let page_height = 160;
        let full = render_markdown(markdown, dir.path(), 800).unwrap();
        assert!(full.height() > page_height * 2);

        let page = render_markdown_page(
            markdown,
            dir.path(),
            800,
            DEFAULT_MARKDOWN_FONT_PT,
            2,
            Some(page_height),
        )
        .unwrap();
        let expected = full.crop_imm(0, page_height, full.width(), page_height);

        assert_eq!(page.dimensions(), expected.dimensions());
        assert_eq!(page.to_rgba8().as_raw(), expected.to_rgba8().as_raw());
    }

    #[test]
    fn markdown_chunk_rects_split_oversized_image() {
        let rects = markdown_chunk_rects(1024, markdown_chunk_max_height(1024) * 2 + 17);
        assert!(rects.len() >= 2);
        for rect in &rects {
            assert!(imgutil::check_limits(rect.2, rect.3));
        }
        assert_eq!(rects.first().unwrap().1, 0);
        assert_eq!(
            rects.last().unwrap().1 + rects.last().unwrap().3,
            markdown_chunk_max_height(1024) * 2 + 17
        );
    }

    #[test]
    fn markdown_pager_enter_advances_until_last_page_then_quits() {
        assert_eq!(markdown_pager_action(1, 3, "\n"), PagerAction::ShowPage(2));
        assert_eq!(markdown_pager_action(2, 3, " \n"), PagerAction::ShowPage(3));
        assert_eq!(markdown_pager_action(3, 3, "\n"), PagerAction::Quit);
        assert_eq!(markdown_pager_action(3, 3, " \n"), PagerAction::Quit);
    }

    #[test]
    fn markdown_pager_accepts_jump_and_quit_commands() {
        assert_eq!(markdown_pager_action(2, 5, "q\n"), PagerAction::Quit);
        assert_eq!(markdown_pager_action(2, 5, "Q\n"), PagerAction::Quit);
        assert_eq!(markdown_pager_action(2, 5, "4\n"), PagerAction::ShowPage(4));
        assert_eq!(
            markdown_pager_action(2, 5, "99\n"),
            PagerAction::ShowPage(5)
        );
        assert_eq!(
            markdown_pager_action(2, 5, "bad\n"),
            PagerAction::ShowPage(2)
        );
    }

    #[test]
    fn render_markdown_solo_image_uses_base_dir() {
        let dir = tempfile::tempdir().unwrap();
        let image_path = dir.path().join("inline.png");
        let mut image = DynamicImage::new_rgba8(32, 24);
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        std::fs::write(&image_path, imgutil::encode_png(&image).unwrap()).unwrap();
        let rendered = render_markdown(b"![alt](inline.png)\n", dir.path(), 800).unwrap();
        assert!(
            rendered
                .to_rgba8()
                .pixels()
                .any(|pixel| pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60),
            "the relative solo image should be rendered"
        );
    }

    // macOS file systems reject non-UTF-8 names, so this only runs on Linux.
    #[cfg(target_os = "linux")]
    #[test]
    fn render_markdown_reads_image_under_non_utf8_base_dir() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let base_dir = dir.path().join(std::ffi::OsStr::from_bytes(b"bad\xff"));
        std::fs::create_dir(&base_dir).unwrap();
        let mut image = DynamicImage::new_rgba8(32, 24);
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        std::fs::write(
            base_dir.join("inline.png"),
            imgutil::encode_png(&image).unwrap(),
        )
        .unwrap();
        let rendered = render_markdown(b"![alt](inline.png)\n", &base_dir, 800).unwrap();
        assert!(
            rendered
                .to_rgba8()
                .pixels()
                .any(|pixel| pixel[0] > 200 && pixel[1] < 60 && pixel[2] < 60),
            "the image under a non-UTF-8 directory should be rendered"
        );
    }

    #[test]
    fn render_markdown_inline_image_mixed_with_text() {
        let dir = tempfile::tempdir().unwrap();
        let image_path = dir.path().join("inline.png");
        let image = image::RgbImage::from_pixel(24, 16, image::Rgb([255, 0, 0]));
        let mut encoded = Vec::new();
        image::codecs::png::PngEncoder::new(&mut encoded)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        std::fs::write(&image_path, encoded).unwrap();

        let rendered =
            render_markdown(b"before ![alt](inline.png) after\n", dir.path(), 800).unwrap();
        let rgba = rendered.to_rgba8();
        let has_red = rgba
            .pixels()
            .any(|p| p[0] > 200 && p[1] < 60 && p[2] < 60 && p[3] > 200);
        assert!(has_red, "inline image mixed with text should be rendered");
    }

    #[test]
    fn render_markdown_inline_image_missing_file_keeps_text() {
        let dir = tempfile::tempdir().unwrap();
        let rendered =
            render_markdown(b"before ![alt](missing.png) after\n", dir.path(), 800).unwrap();
        let before = render_markdown(b"before\n", dir.path(), 800).unwrap();
        let after = render_markdown(b"after\n", dir.path(), 800).unwrap();
        let (left, _, right, _) = non_white_bounds(&rendered);
        let (before_left, _, before_right, _) = non_white_bounds(&before);
        let (after_left, _, after_right, _) = non_white_bounds(&after);
        let rendered_width = right - left + 1;
        let widest_single_word = (before_right - before_left + 1).max(after_right - after_left + 1);
        assert!(
            rendered_width > widest_single_word,
            "both text runs around a missing inline image should remain visible"
        );
    }

    #[test]
    fn render_markdown_oversized_word_wraps_within_canvas() {
        let word = "w".repeat(200);
        let markdown = format!("start {word} end\n");
        let image = render_markdown(markdown.as_bytes(), Path::new(""), 800).unwrap();
        let (_, _, right, _) = non_white_bounds(&image);

        assert!(
            right + 1 < image.width(),
            "an unbreakable word should wrap at glyph level instead of overflowing: right={right}"
        );
        // Its glyph-wrapped lines make the paragraph taller than a one-liner.
        let one_liner = render_markdown(b"start w end\n", Path::new(""), 800).unwrap();
        assert!(image.height() > one_liner.height());
    }

    #[test]
    fn render_markdown_inline_code_preserves_whitespace() {
        let tabbed = render_markdown(b"`a\tb`\n", Path::new(""), 800).unwrap();
        let double_spaced = render_markdown(b"`a  b`\n", Path::new(""), 800).unwrap();
        let single_spaced = render_markdown(b"`a b`\n", Path::new(""), 800).unwrap();
        let (tab_left, _, tab_right, _) = non_white_bounds(&tabbed);
        let (double_left, _, double_right, _) = non_white_bounds(&double_spaced);
        let (single_left, _, single_right, _) = non_white_bounds(&single_spaced);
        let single_width = single_right - single_left;

        assert!(
            tab_right - tab_left > single_width,
            "a tab inside inline code should occupy more space than one ordinary space"
        );
        assert!(
            double_right - double_left > single_width,
            "repeated spaces inside inline code should not collapse"
        );
    }

    #[test]
    fn render_markdown_preserves_unicode_space_width() {
        let em_spaced = render_markdown("a\u{2003}b\n".as_bytes(), Path::new(""), 800).unwrap();
        let ordinarily_spaced = render_markdown(b"a b\n", Path::new(""), 800).unwrap();
        let (em_left, _, em_right, _) = non_white_bounds(&em_spaced);
        let (ordinary_left, _, ordinary_right, _) = non_white_bounds(&ordinarily_spaced);

        assert!(
            em_right - em_left > ordinary_right - ordinary_left,
            "an em space should not be collapsed to an ordinary Markdown space"
        );
    }

    #[test]
    fn render_markdown_italic_text_differs_from_plain_text() {
        let italic = render_markdown(b"*italic sample*\n", Path::new(""), 800).unwrap();
        let plain = render_markdown(b"italic sample\n", Path::new(""), 800).unwrap();

        assert_ne!(
            italic.to_rgba8().as_raw(),
            plain.to_rgba8().as_raw(),
            "italic Markdown should produce different glyphs from plain text"
        );
    }

    #[test]
    fn render_markdown_link_draws_underline() {
        let image =
            render_markdown(b"[click here](https://example.com)\n", Path::new(""), 800).unwrap();
        let rgba = image.to_rgba8();
        // The underline is a solid link-blue rectangle spanning the text, which
        // produces a much longer horizontal run than any anti-aliased glyph.
        let mut longest_run = 0_u32;
        for y in 0..rgba.height() {
            let mut run = 0_u32;
            for x in 0..rgba.width() {
                let p = rgba.get_pixel(x, y);
                if p[0] == 0x06 && p[1] == 0x4F && p[2] == 0xBD {
                    run += 1;
                    longest_run = longest_run.max(run);
                } else {
                    run = 0;
                }
            }
        }
        assert!(
            longest_run >= 30,
            "link should have a continuous underline, longest blue run: {longest_run}"
        );
    }

    #[test]
    fn parse_markdown_continues_after_definition_list() {
        let md = b"Term\n: Definition\n\n## Next section\n\nContent after definition list.\n";
        let blocks = parse_markdown_blocks(md);

        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, Block::Heading { tokens, .. } if flatten_tokens(tokens) == "Next section")),
            "parser should not stop at definition list: {blocks:?}"
        );
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, Block::Paragraph(tokens) if flatten_tokens(tokens).contains("Content after"))),
            "parser should include content after definition list: {blocks:?}"
        );
    }

    #[test]
    fn parse_markdown_list_item_with_block_child_terminates() {
        // Regression: a blockquote (or any block element) nested in a list item
        // used to leave an unconsumed End event and spin collect_list_items forever.
        let md = b"- item\n\n  > quote\n\nAfter list.\n";
        let blocks = parse_markdown_blocks(md);

        assert!(
            blocks.iter().any(|block| matches!(
                block,
                Block::List { items, .. } if items.iter().any(|item| {
                    let text = flatten_tokens(item);
                    text.contains("item") && text.contains("quote")
                })
            )),
            "the list should retain both its paragraph and nested quote text: {blocks:?}"
        );
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, Block::Paragraph(tokens) if flatten_tokens(tokens) == "After list.")),
            "parser should continue past the list: {blocks:?}"
        );
    }

    #[test]
    fn parse_markdown_list_item_with_code_block_terminates() {
        // The other hang trigger: a fenced code block nested in a list item.
        let md = b"- item\n\n  ```rust\n  let x = 1;\n  ```\n\nAfter list.\n";
        let blocks = parse_markdown_blocks(md);

        assert!(
            blocks.iter().any(|block| matches!(
                block,
                Block::List { items, .. } if items.iter().any(|item| {
                    let text = flatten_tokens(item);
                    text.contains("item") && text.contains("let x = 1;")
                })
            )),
            "the list should retain its paragraph and nested code text: {blocks:?}"
        );
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, Block::Paragraph(tokens) if flatten_tokens(tokens) == "After list.")),
            "parser should continue past the list: {blocks:?}"
        );
    }

    #[test]
    fn parse_markdown_continues_after_nested_list() {
        let md = b"- parent\n  - child\n- sibling\n\nAfter list.\n";
        let blocks = parse_markdown_blocks(md);

        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, Block::Paragraph(tokens) if flatten_tokens(tokens) == "After list.")),
            "parser should include content after nested list: {blocks:?}"
        );
    }

    #[test]
    fn render_markdown_malformed_inline_markup_completes() {
        // Unclosed inline constructs must be consumed, not hang or error: the
        // trailing paragraph proves the parser made it to the end of input.
        let md = b"Unclosed **bold and *italic\n\nUnclosed [link text\n\nUnclosed `code span\n\nUnclosed $x^2+y^2 math\n\nStill here.\n";
        let image = render_markdown(md, Path::new(""), 800).unwrap();
        assert_eq!(image.width(), 800);
        assert!(image.height() > 0);

        let blocks = parse_markdown_blocks(md);
        assert!(
            blocks
                .iter()
                .any(|block| matches!(block, Block::Paragraph(tokens) if flatten_tokens(tokens) == "Still here.")),
            "parser should reach content after malformed inline markup: {blocks:?}"
        );
    }

    #[test]
    fn bold_nested_in_italic_renders_bold_italic() {
        let nested = render_markdown(b"*outer **inner** end*\n", Path::new(""), 800).unwrap();
        let bold_only = render_markdown(b"*outer* **inner** *end*\n", Path::new(""), 800).unwrap();
        let italic_only = render_markdown(b"*outer inner end*\n", Path::new(""), 800).unwrap();

        assert!(
            nested.to_rgba8() != bold_only.to_rgba8(),
            "bold nested in italic should keep the italic style"
        );
        assert!(
            nested.to_rgba8() != italic_only.to_rgba8(),
            "bold nested in italic should keep the bold weight"
        );
    }

    #[test]
    fn render_markdown_tight_list_smaller_than_loose() {
        let tight_md = b"- alpha\n- beta\n- gamma\n";
        let loose_md = b"- alpha\n\n- beta\n\n- gamma\n";
        let tight = render_markdown(tight_md, Path::new(""), 800).unwrap();
        let loose = render_markdown(loose_md, Path::new(""), 800).unwrap();
        assert!(
            loose.height() > tight.height(),
            "loose list ({}) should be taller than tight list ({})",
            loose.height(),
            tight.height()
        );
    }

    #[test]
    fn render_markdown_with_blockquote() {
        let md = b"# Title\n\n> This is a blockquote with some text.\n\nNormal paragraph.\n";
        let image = render_markdown(md, Path::new(""), 800).unwrap();
        assert_eq!(image.width(), 800);
        // The quote bar is a solid 4px-wide (180,180,180) rectangle at the left margin.
        let rgba = image.to_rgba8();
        let found_bar = (0..rgba.height()).any(|y| {
            let p = rgba.get_pixel(DEFAULT_MARKDOWN_MARGIN, y);
            p[0] == 180 && p[1] == 180 && p[2] == 180
        });
        assert!(found_bar, "blockquote should draw its vertical bar");
    }

    #[test]
    fn render_markdown_with_table() {
        let md = b"| Name | Value |\n|------|-------|\n| foo  | 42    |\n| bar  | 99    |\n";
        let one_row = render_markdown(
            b"| Name | Value |\n|------|-------|\n| foo  | 42    |\n",
            Path::new(""),
            800,
        )
        .unwrap();
        let two_rows = render_markdown(md, Path::new(""), 800).unwrap();
        assert!(
            two_rows.height() > one_row.height(),
            "two rows ({}) should be taller than one row ({})",
            two_rows.height(),
            one_row.height()
        );
    }

    #[test]
    fn rust_code_highlighting_loads_theme_and_multiple_styles() {
        let source = "fn main() { let value = 42; }\n";
        let spans = highlight_code_spans("rust", source);
        let rendered_text: String = spans.iter().map(|(_, text)| text.as_str()).collect();
        let colors = spans
            .iter()
            .map(|(style, _)| (style.foreground.r, style.foreground.g, style.foreground.b))
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(rendered_text, source);
        assert!(
            colors.len() > 1,
            "Rust highlighting should produce multiple foreground colors: {spans:?}"
        );
    }

    #[test]
    fn mermaid_render_error_falls_back_to_code_block() {
        for diagram in [
            "stateDiagram\n    state \"unterminated",
            "pie title Pets\n    \"Dogs\" : 386",
            "flowchart TD\n    A --> B\n    B -> C",
        ] {
            assert_falls_back_to_code_block(&format!("```mermaid\n{diagram}\n```"));
        }
    }

    #[test]
    fn math_render_error_falls_back_to_source() {
        let unknown = r"\begin{unknown}x\end{unknown}";
        assert_falls_back_to_code_block(&format!("$$\n{unknown}\n$$"));

        // Inline math falls back to its source text within the paragraph.
        let md = format!("Before ${unknown}$ after.\n");
        let image = render_markdown(md.as_bytes(), Path::new(""), 800).unwrap();
        let plain = render_markdown(b"Before  after.\n", Path::new(""), 800).unwrap();
        assert!(
            ink_columns(&image) > ink_columns(&plain),
            "the inline LaTeX source should render as text"
        );
    }

    fn ink_columns(image: &DynamicImage) -> usize {
        let rgba = image.to_rgba8();
        (0..rgba.width())
            .filter(|&x| {
                (0..rgba.height()).any(|y| {
                    let p = rgba.get_pixel(x, y);
                    p[0] < 160 && p[1] < 160 && p[2] < 160
                })
            })
            .count()
    }

    /// Renders `block` followed by a paragraph and checks that the block shows
    /// up as a code block with text and the paragraph still renders.
    fn assert_falls_back_to_code_block(block: &str) {
        let md = format!("{block}\n\nAfter block.\n");
        let image = render_markdown(md.as_bytes(), Path::new(""), 800).unwrap();
        let rgba = image.to_rgba8();
        let code_rows: Vec<u32> = (0..rgba.height())
            .filter(|&y| {
                let p = rgba.get_pixel(DEFAULT_MARKDOWN_MARGIN + 2, y);
                p[0] == 245 && p[1] == 245 && p[2] == 245
            })
            .collect();
        let (code_top, code_bottom) = (code_rows[0], *code_rows.last().unwrap());
        let ink_in = |top: u32, bottom: u32| {
            (top..bottom).any(|y| {
                (0..rgba.width()).any(|x| {
                    let p = rgba.get_pixel(x, y);
                    p[0] < 160 && p[1] < 160 && p[2] < 160
                })
            })
        };
        assert!(
            ink_in(code_top, code_bottom),
            "the source should render as code text: {block}"
        );
        assert!(
            ink_in(code_bottom + 1, rgba.height()),
            "the paragraph after the failed block should render: {block}"
        );
    }

    #[test]
    fn glyph_blend_matches_tiny_skia_fill_rect() {
        // Glyph pixels bypass tiny-skia, so this also catches blend changes in
        // a tiny-skia upgrade. The page under glyphs is always opaque.
        let mut pixmap = Pixmap::new(1, 1).unwrap();
        for d in [0, 1, 127, 128, 254, 255] {
            for alpha in 0..=255_u8 {
                for value in 0..=255_u8 {
                    let rgba = [value, 255 - value, value / 2, alpha];
                    let mut blended = [d, d, d, 255];
                    pixmap.data_mut().copy_from_slice(&blended);
                    fill_rect(&mut pixmap, 0, 0, 1, 1, rgba);
                    blend_source_over(&mut blended, rgba);
                    assert_eq!(&blended[..], pixmap.data(), "{rgba:?} over {d}");
                }
            }
        }
    }

    #[test]
    fn render_markdown_code_block_has_gray_bg() {
        let md = b"```rust\nlet x = 1;\n```\n";
        let image = render_markdown(md, Path::new(""), 800).unwrap();
        // Sample pixels across the code block area for any gray background pixel
        let rgba = image.to_rgba8();
        let found_gray = (0..rgba.width()).step_by(4).any(|x| {
            (0..rgba.height()).step_by(4).any(|y| {
                let p = rgba.get_pixel(x, y);
                p[0] == 245 && p[1] == 245 && p[2] == 245
            })
        });
        assert!(
            found_gray,
            "code block should have gray (245,245,245) background"
        );
    }

    #[test]
    fn render_markdown_cjk_code_line_keeps_single_line_height() {
        let ascii = render_markdown(b"```bash\n--prompt \"abcdef\"\n```\n", Path::new(""), 800);
        let cjk = render_markdown(
            "```bash\n--prompt \"把衣服换成深绿色\"\n```\n".as_bytes(),
            Path::new(""),
            800,
        );
        assert_eq!(cjk.unwrap().height(), ascii.unwrap().height());
    }

    // Font faces used by CJK glyphs, grouped by prose, inline code, and code block.
    fn cjk_faces_by_context(markdown: &str) -> [BTreeSet<cosmic_text::fontdb::ID>; 3] {
        fn collect(layout: &TextLayout, faces: &mut BTreeSet<cosmic_text::fontdb::ID>) {
            for run in layout.buffer.layout_runs() {
                for glyph in run.glyphs {
                    if run.text[glyph.start..glyph.end]
                        .chars()
                        .any(fonts::is_cjk_char)
                    {
                        faces.insert(glyph.font_id);
                    }
                }
            }
        }

        let mut font_system = fonts::resolve_fonts().font_system;
        let blocks = parse_markdown_blocks(markdown.as_bytes());
        let rendered = layout_document(&blocks, Path::new(""), &mut font_system, 800, 24.0)
            .unwrap()
            .blocks;
        let [mut prose, mut inline_code, mut code_block] = Default::default();
        for block in &rendered {
            match block {
                RenderBlock::Inline { layout, .. } => {
                    for item in &layout.items {
                        if let InlineRenderItem::Text { layout, .. } = item {
                            let mono = layout.buffer.lines.iter().any(|line| {
                                line.attrs_list().defaults().family == Family::Monospace
                            });
                            collect(layout, if mono { &mut inline_code } else { &mut prose });
                        }
                    }
                }
                RenderBlock::Code { layout, .. } => collect(layout, &mut code_block),
                _ => {}
            }
        }
        [prose, inline_code, code_block]
    }

    #[test]
    fn monospace_cjk_uses_same_face_as_prose() {
        let [prose, inline_code, code_block] = cjk_faces_by_context(
            "把外套改成深绿色，保持背景不变。\n\n`把外套改成深绿色，保持背景不变。`\n\n```bash\n--prompt \"把外套改成深绿色，保持背景不变。\"\n```\n",
        );
        assert_eq!(prose.len(), 1, "prose CJK should use one face: {prose:?}");
        assert_eq!(inline_code, prose);
        assert_eq!(code_block, prose);
    }
}
