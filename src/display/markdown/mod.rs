pub mod fonts;
mod markie;
mod math;
mod mermaid;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, LayoutRun, Metrics, PhysicalGlyph, Renderer, Shaping,
    SwashCache, Weight, render_decoration,
};
use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba, imageops};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use tiny_skia::{BlendMode, Paint as SkPaint, Pixmap, Rect as SkRect, Transform};

use crate::display::MarkdownOptions;
use crate::imgutil;
use crate::term::{self, Size};

const DEFAULT_MARKDOWN_WIDTH: u32 = 1024;
const DEFAULT_MARKDOWN_MARGIN: u32 = 48;
pub const DEFAULT_MARKDOWN_FONT_PT: f64 = 18.0;
pub const MIN_MARKDOWN_WIDTH: u32 = 480;
const MARKDOWN_CHUNK_HEIGHT: u32 = 8192;

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

pub fn markdown(path: &str, size: Size, tmux: bool) -> Result<(), Box<dyn std::error::Error>> {
    markdown_with_options(
        path,
        size,
        tmux,
        MarkdownOptions {
            page: None,
            font_size_pt: 0.0,
        },
    )
}

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

pub fn markdown_from_bytes(
    data: &[u8],
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    markdown_from_bytes_with_options(
        data,
        size,
        tmux,
        MarkdownOptions {
            page: None,
            font_size_pt: 0.0,
        },
    )
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
    let font_size = markdown_font_size(opts.font_size_pt);
    let blocks = parse_markdown_blocks(data);

    let state = FONT_STATE.get_or_init(|| {
        let res = fonts::resolve_fonts();
        Mutex::new(FontState {
            font_system: res.font_system,
            swash: SwashCache::new(),
            warning: res.warning,
        })
    });

    // Layout pass: shape all text and place blocks (needs font_system).
    // This must process the full document to know total height, but allocates
    // no pixel buffer — only the logical block tree.
    let (mut rendered_blocks, total_height) = {
        let mut guard = state.lock().unwrap();
        if let Some(w) = guard.warning.take() {
            eprintln!("{w}");
        }
        let FontState { font_system, .. } = &mut *guard;
        layout_blocks_inner(&blocks, &base_dir, font_system, width, font_size)?
    };

    let total_pages = markdown_total_pages(total_height, page_height);
    let start_page = opts.page.unwrap_or(1).clamp(1, total_pages.max(1));
    let interactive =
        opts.page.is_none() && total_pages > 1 && term::is_terminal(&std::io::stdout());

    if !interactive {
        let y_start = (start_page - 1) as u32 * page_height;
        let draw_h = total_height.saturating_sub(y_start).min(page_height);
        let image = {
            let mut guard = state.lock().unwrap();
            let FontState {
                font_system, swash, ..
            } = &mut *guard;
            draw_blocks_page(
                &mut rendered_blocks,
                font_system,
                swash,
                width,
                y_start,
                draw_h,
            )?
        };
        return send_rendered_markdown(&image, size, tmux);
    }

    // Interactive paging: layout is done once; draw only the requested page
    // each time (small per-page Pixmap, released after send).
    let mut current = start_page;
    loop {
        let image = {
            let mut guard = state.lock().unwrap();
            let FontState {
                font_system, swash, ..
            } = &mut *guard;
            let y_start = (current - 1) as u32 * page_height;
            let draw_h = total_height.saturating_sub(y_start).min(page_height);
            draw_blocks_page(
                &mut rendered_blocks,
                font_system,
                swash,
                width,
                y_start,
                draw_h,
            )?
        };
        send_rendered_markdown(&image, size, tmux)?;

        let prompt = if current < total_pages {
            format!(
                "-- Markdown page {current}/{total_pages} -- Enter next, number+Enter jump, q+Enter quit: "
            )
        } else {
            format!("-- Markdown page {current}/{total_pages} -- Enter/q quit, number+Enter jump: ")
        };
        let input = match term::read_interactive_line(&prompt) {
            Ok(s) => s,
            Err(_) => break,
        };
        match markdown_pager_action(current, total_pages, &input) {
            PagerAction::ShowPage(next) => current = next,
            PagerAction::Quit => break,
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PagerAction {
    ShowPage(usize),
    Quit,
}

fn markdown_pager_action(current: usize, total_pages: usize, input: &str) -> PagerAction {
    let total_pages = total_pages.max(1);
    let current = current.clamp(1, total_pages);
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == " " {
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

pub fn render_markdown(
    data: &[u8],
    base_dir: &Path,
    max_pixel_width: u32,
) -> Result<DynamicImage, Box<dyn std::error::Error>> {
    render_markdown_with_font_size(data, base_dir, max_pixel_width, 0.0)
}

pub fn render_markdown_with_font_size(
    data: &[u8],
    base_dir: &Path,
    max_pixel_width: u32,
    font_size_pt: f64,
) -> Result<DynamicImage, Box<dyn std::error::Error>> {
    let blocks = parse_markdown_blocks(data);
    render_blocks(
        &blocks,
        base_dir,
        markdown_render_width(max_pixel_width),
        markdown_font_size(font_size_pt),
    )
}

pub fn render_markdown_page_detailed(
    data: &[u8],
    base_dir: &Path,
    max_pixel_width: u32,
    page_index: usize,
    page_height: u32,
    font_size_pt: f64,
) -> Result<DynamicImage, Box<dyn std::error::Error>> {
    let blocks = parse_markdown_blocks(data);
    let width = markdown_render_width(max_pixel_width);
    let font_size = markdown_font_size(font_size_pt);
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
    let (mut rendered_blocks, total_height) =
        layout_blocks_inner(&blocks, base_dir, font_system, width, font_size)?;
    let y_start = (page_index as u32)
        .saturating_mul(page_height)
        .min(total_height.saturating_sub(1));
    let draw_height = total_height.saturating_sub(y_start).min(page_height).max(1);
    draw_blocks_page(
        &mut rendered_blocks,
        font_system,
        swash,
        width,
        y_start,
        draw_height,
    )
}

pub fn measure_markdown_pages(
    data: &[u8],
    base_dir: &Path,
    max_pixel_width: u32,
    page_height: u32,
    font_size_pt: f64,
) -> Result<MarkdownPagePlan, Box<dyn std::error::Error>> {
    let blocks = parse_markdown_blocks(data);
    let width = markdown_render_width(max_pixel_width);
    let font_size = markdown_font_size(font_size_pt);
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
    let FontState { font_system, .. } = &mut *guard;
    let (_, total_height) = layout_blocks_inner(&blocks, base_dir, font_system, width, font_size)?;
    Ok(MarkdownPagePlan {
        total_height,
        page_height,
        total_pages: markdown_total_pages(total_height, page_height),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkdownPagePlan {
    pub total_height: u32,
    pub page_height: u32,
    pub total_pages: usize,
}

// ── AST types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum InlineToken {
    Text {
        text: String,
        bold: bool,
        italic: bool,
        mono: bool,
        color: Option<u32>, // packed 0xRRGGBB
        underline: bool,
    },
    Image {
        path: PathBuf,
        center: bool,
    },
    Math {
        text: String,
        display: bool,
    },
    SoftBreak,
    HardBreak,
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
    Math {
        text: String,
        display: bool,
    },
    Rule,
    Table {
        header: Vec<Vec<InlineToken>>,
        rows: Vec<Vec<Vec<InlineToken>>>,
    },
}

#[derive(Debug, Clone, Copy, Default)]
struct InlineState {
    bold: bool,
    italic: bool,
}

impl InlineState {
    fn regular() -> Self {
        Self {
            bold: false,
            italic: false,
        }
    }

    fn with_bold(self) -> Self {
        Self { bold: true, ..self }
    }

    fn with_italic(self) -> Self {
        Self {
            italic: true,
            ..self
        }
    }
}

// ── Parser ───────────────────────────────────────────────────────────────────

fn parse_markdown_blocks(data: &[u8]) -> Vec<Block> {
    let markdown = String::from_utf8_lossy(data);
    let parser = Parser::new_ext(&markdown, Options::all() | Options::ENABLE_MATH);
    let events: Vec<Event<'_>> = parser.collect();
    let mut idx = 0;
    parse_block_list(&events, &mut idx)
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
        Event::Start(Tag::Heading { level, .. }) => {
            let level = heading_level_from_tag(*level);
            *idx += 1;
            let tokens = collect_inline_tokens(events, idx, InlineState::regular());
            if matches!(events.get(*idx), Some(Event::End(TagEnd::Heading(_)))) {
                *idx += 1;
            }
            Some(Block::Heading { level, tokens })
        }
        Event::Start(Tag::Paragraph) => {
            *idx += 1;
            let tokens = collect_inline_tokens(events, idx, InlineState::regular());
            if matches!(events.get(*idx), Some(Event::End(TagEnd::Paragraph))) {
                *idx += 1;
            }
            if tokens.is_empty() {
                None
            } else {
                Some(Block::Paragraph(tokens))
            }
        }
        Event::Start(Tag::BlockQuote(_)) => {
            *idx += 1;
            let children = parse_block_list(events, idx);
            if matches!(events.get(*idx), Some(Event::End(TagEnd::BlockQuote(_)))) {
                *idx += 1;
            }
            Some(Block::BlockQuote(children))
        }
        Event::Start(Tag::List(start)) => {
            let ordered = start.is_some();
            *idx += 1;
            let (items, tight) = collect_list_items(events, idx);
            if matches!(events.get(*idx), Some(Event::End(TagEnd::List(_)))) {
                *idx += 1;
            }
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
            Some(Block::Math {
                text: text.to_string(),
                display: true,
            })
        }
        Event::Start(Tag::Table(_)) => {
            *idx += 1;
            let (header, rows) = collect_table(events, idx);
            if matches!(events.get(*idx), Some(Event::End(TagEnd::Table))) {
                *idx += 1;
            }
            Some(Block::Table { header, rows })
        }
        Event::Start(Tag::FootnoteDefinition(label)) => {
            let label = label.to_string();
            *idx += 1;
            let children = parse_block_list(events, idx);
            if matches!(
                events.get(*idx),
                Some(Event::End(TagEnd::FootnoteDefinition))
            ) {
                *idx += 1;
            }
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
            if matches!(events.get(*idx), Some(Event::End(TagEnd::DefinitionList))) {
                *idx += 1;
            }
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
    state: InlineState,
) -> Vec<InlineToken> {
    let mut tokens = Vec::new();
    while *idx < events.len() {
        match &events[*idx] {
            Event::End(_) => break,
            Event::Text(text) => {
                tokens.push(InlineToken::Text {
                    text: text.to_string(),
                    bold: state.bold,
                    italic: state.italic,
                    mono: false,
                    color: None,
                    underline: false,
                });
                *idx += 1;
            }
            Event::Code(text) => {
                // Inline code span: monospace
                tokens.push(InlineToken::Text {
                    text: text.to_string(),
                    bold: false,
                    italic: false,
                    mono: true,
                    color: None,
                    underline: false,
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
                let inner = collect_inline_tokens(events, idx, state.with_italic());
                if matches!(events.get(*idx), Some(Event::End(TagEnd::Emphasis))) {
                    *idx += 1;
                }
                tokens.extend(inner);
            }
            Event::Start(Tag::Strong) => {
                *idx += 1;
                // Emphasis level-2: entering bold escalates italic→bold+italic
                let inner = collect_inline_tokens(events, idx, state.with_bold());
                if matches!(events.get(*idx), Some(Event::End(TagEnd::Strong))) {
                    *idx += 1;
                }
                tokens.extend(inner);
            }
            Event::Start(Tag::Link { .. }) => {
                *idx += 1;
                let mut link_tokens = collect_inline_tokens(events, idx, state);
                if matches!(events.get(*idx), Some(Event::End(TagEnd::Link))) {
                    *idx += 1;
                }
                // Link color 0x064FBD + underline on all text spans
                for t in link_tokens.iter_mut() {
                    if let InlineToken::Text {
                        color, underline, ..
                    } = t
                    {
                        *color = Some(0x064FBD);
                        *underline = true;
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
                tokens.push(InlineToken::Image { path, center: true });
            }
            Event::Start(Tag::List(_)) => break,
            Event::Start(Tag::Strikethrough) => {
                *idx += 1;
                let inner = collect_inline_tokens(events, idx, state);
                if matches!(events.get(*idx), Some(Event::End(TagEnd::Strikethrough))) {
                    *idx += 1;
                }
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
        bold: false,
        italic: false,
        mono: false,
        color: None,
        underline: false,
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
                            let toks = collect_inline_tokens(events, idx, InlineState::regular());
                            item_tokens.extend(toks);
                            if matches!(events.get(*idx), Some(Event::End(TagEnd::Paragraph))) {
                                *idx += 1;
                            }
                        }
                        Event::Start(Tag::List(_)) => {
                            *idx += 1;
                            let (nested_items, _) = collect_list_items(events, idx);
                            if matches!(events.get(*idx), Some(Event::End(TagEnd::List(_)))) {
                                *idx += 1;
                            }
                            for nested in nested_items {
                                if !item_tokens.is_empty() {
                                    item_tokens.push(InlineToken::HardBreak);
                                }
                                item_tokens.push(plain_text_token("  • "));
                                item_tokens.extend(nested);
                            }
                        }
                        _ => {
                            let toks = collect_inline_tokens(events, idx, InlineState::regular());
                            item_tokens.extend(toks);
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
                    flatten_tokens(&collect_inline_tokens(events, idx, InlineState::regular()));
                if matches!(
                    events.get(*idx),
                    Some(Event::End(TagEnd::DefinitionListTitle))
                ) {
                    *idx += 1;
                }
            }
            Event::Start(Tag::DefinitionListDefinition) => {
                *idx += 1;
                let blocks = parse_block_list(events, idx);
                if matches!(
                    events.get(*idx),
                    Some(Event::End(TagEnd::DefinitionListDefinition))
                ) {
                    *idx += 1;
                }
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
                let cell = collect_inline_tokens(events, idx, InlineState::regular());
                if matches!(events.get(*idx), Some(Event::End(TagEnd::TableCell))) {
                    *idx += 1;
                }
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
                            let cell = collect_inline_tokens(events, idx, InlineState::regular());
                            if matches!(events.get(*idx), Some(Event::End(TagEnd::TableCell))) {
                                *idx += 1;
                            }
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

pub(crate) fn flatten_tokens(tokens: &[InlineToken]) -> String {
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
            Block::Math { text, .. } => {
                s.push_str(text);
                s.push('\n');
            }
            _ => {}
        }
    }
    s
}

fn heading_level_from_tag(level: pulldown_cmark::HeadingLevel) -> u32 {
    match level {
        pulldown_cmark::HeadingLevel::H1 => 1,
        pulldown_cmark::HeadingLevel::H2 => 2,
        pulldown_cmark::HeadingLevel::H3 => 3,
        pulldown_cmark::HeadingLevel::H4 => 4,
        pulldown_cmark::HeadingLevel::H5 => 5,
        pulldown_cmark::HeadingLevel::H6 => 6,
    }
}

// ── Renderer ──────────────────────────────────────────────────────────────────

fn render_blocks(
    blocks: &[Block],
    base_dir: &Path,
    width: u32,
    font_size: f64,
) -> Result<DynamicImage, Box<dyn std::error::Error>> {
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
    let (mut rendered_blocks, total_height) =
        layout_blocks_inner(blocks, base_dir, font_system, width, font_size)?;
    draw_blocks_page(
        &mut rendered_blocks,
        font_system,
        swash,
        width,
        0,
        total_height,
    )
}

fn layout_blocks_inner(
    blocks: &[Block],
    base_dir: &Path,
    font_system: &mut FontSystem,
    width: u32,
    font_size: f64,
) -> Result<(Vec<RenderBlock>, u32), Box<dyn std::error::Error>> {
    let mut image_cache = HashMap::new();
    let mut y = DEFAULT_MARKDOWN_MARGIN;
    let mut rendered_blocks: Vec<RenderBlock> = Vec::new();
    let content_width = width.saturating_sub(DEFAULT_MARKDOWN_MARGIN * 2).max(1);

    for block in blocks {
        match block {
            Block::Heading { level, tokens } => {
                let size = match level {
                    1 => font_size * 1.9,
                    2 => font_size * 1.6,
                    3 => font_size * 1.4,
                    4 => font_size * 1.25,
                    _ => font_size * 1.15,
                } as f32;
                let text = flatten_tokens(tokens);
                let layout = layout_text(font_system, &text, content_width, size, FontKind::Bold)?;
                y += (font_size * 0.75) as u32;
                rendered_blocks.push(RenderBlock::Text {
                    layout,
                    x: DEFAULT_MARKDOWN_MARGIN,
                    y,
                });
                y += rendered_blocks.last().unwrap().height() + (font_size * 0.5) as u32;
            }
            Block::Paragraph(tokens) => {
                // Solo image paragraph: render inline image centered
                let non_empty: Vec<_> = tokens
                    .iter()
                    .filter(|t| !matches!(t, InlineToken::SoftBreak | InlineToken::HardBreak))
                    .collect();
                let is_solo_image =
                    non_empty.len() == 1 && matches!(non_empty[0], InlineToken::Image { .. });
                let is_solo_math =
                    non_empty.len() == 1 && matches!(non_empty[0], InlineToken::Math { .. });

                if is_solo_image {
                    if let InlineToken::Image { path, center } = &non_empty[0] {
                        let resolved = resolve_image_path(base_dir, path);
                        if let Some(img) = load_inline_image(&resolved, &mut image_cache)? {
                            let img = scale_markdown_image_to_width(&img, content_width);
                            let x = if *center {
                                DEFAULT_MARKDOWN_MARGIN
                                    + content_width.saturating_sub(img.width()) / 2
                            } else {
                                DEFAULT_MARKDOWN_MARGIN
                            };
                            rendered_blocks.push(RenderBlock::Image { image: img, x, y });
                            y +=
                                rendered_blocks.last().unwrap().height() + (font_size * 0.6) as u32;
                        }
                    }
                } else if is_solo_math {
                    if let InlineToken::Math { text, display } = &non_empty[0] {
                        let rendered =
                            math::render_math(text, font_system, font_size as f32, *display)
                                .map_err(|err| format!("failed to render math: {err}"))?;
                        let image = scale_markdown_image_to_width(&rendered.image, content_width);
                        let x = DEFAULT_MARKDOWN_MARGIN
                            + content_width.saturating_sub(image.width()) / 2;
                        rendered_blocks.push(RenderBlock::Image { image, x, y });
                        y += rendered_blocks.last().unwrap().height() + (font_size * 0.6) as u32;
                    }
                } else if !flatten_tokens(tokens).trim().is_empty() {
                    let layout =
                        layout_inline_tokens(font_system, tokens, content_width, font_size as f32)?;
                    if layout.height > 0 {
                        rendered_blocks.push(RenderBlock::Inline {
                            layout,
                            x: DEFAULT_MARKDOWN_MARGIN,
                            y,
                        });
                        y += rendered_blocks.last().unwrap().height() + (font_size * 0.9) as u32;
                    }
                }
            }
            Block::List {
                items,
                ordered,
                tight,
            } => {
                for (item_idx, item) in items.iter().enumerate() {
                    let prefix = if *ordered {
                        format!("{}.", item_idx + 1)
                    } else {
                        "•".to_string()
                    };
                    let marker = layout_text(
                        font_system,
                        &prefix,
                        32,
                        font_size as f32,
                        FontKind::Regular,
                    )?;
                    let content = layout_inline_tokens(
                        font_system,
                        item,
                        content_width.saturating_sub(40),
                        font_size as f32,
                    )?;
                    rendered_blocks.push(RenderBlock::Text {
                        layout: marker,
                        x: DEFAULT_MARKDOWN_MARGIN,
                        y,
                    });
                    let content_height = content.height;
                    rendered_blocks.push(RenderBlock::Inline {
                        layout: content,
                        x: DEFAULT_MARKDOWN_MARGIN + 40,
                        y,
                    });
                    y += content_height.max(font_size as u32 + 8);
                    if item_idx + 1 < items.len() {
                        y += if *tight {
                            (font_size * 0.6) as u32
                        } else {
                            (font_size * 0.9) as u32
                        };
                    }
                }
                y += (font_size * 0.7) as u32;
            }
            Block::BlockQuote(children) => {
                let content_text = flatten_blocks_to_text(children);
                let trimmed = content_text.trim();
                if !trimmed.is_empty() {
                    let layout = layout_text(
                        font_system,
                        trimmed,
                        content_width.saturating_sub(48),
                        font_size as f32,
                        FontKind::Regular,
                    )?;
                    let h = layout.height;
                    // 4px vertical bar on the left
                    rendered_blocks.push(RenderBlock::Rect {
                        x: DEFAULT_MARKDOWN_MARGIN,
                        y,
                        width: 4,
                        height: h.max(font_size as u32),
                        color: Rgba([180, 180, 180, 255]),
                    });
                    rendered_blocks.push(RenderBlock::Text {
                        layout,
                        x: DEFAULT_MARKDOWN_MARGIN + 48,
                        y,
                    });
                    y += h + (font_size * 0.9) as u32;
                } else {
                    y += 4;
                }
            }
            Block::Code { lang, text } => {
                if lang.trim().eq_ignore_ascii_case("mermaid") {
                    let image =
                        mermaid::render_mermaid(text, font_system, content_width, font_size as f32)
                            .map_err(|err| format!("failed to render Mermaid: {err}"))?;
                    let x =
                        DEFAULT_MARKDOWN_MARGIN + content_width.saturating_sub(image.width()) / 2;
                    rendered_blocks.push(RenderBlock::Image { image, x, y });
                    y += rendered_blocks.last().unwrap().height() + (font_size * 0.6) as u32;
                    continue;
                }
                let code_size = (font_size * 0.95) as f32;
                let code_inner = content_width.saturating_sub(20);
                let spans_data = highlight_code_spans(lang, text);

                let mut buffer = Buffer::new(font_system, Metrics::new(code_size, code_size * 1.4));
                buffer.set_size(Some(code_inner as f32), None);
                {
                    let rich: Vec<(&str, Attrs)> = spans_data
                        .iter()
                        .map(|(s, r, g, b, bold)| {
                            let mut a = Attrs::new()
                                .family(Family::Monospace)
                                .color(Color::rgb(*r, *g, *b));
                            if *bold {
                                a = a.weight(Weight::BOLD);
                            }
                            (s.as_str(), a)
                        })
                        .collect();
                    let default_attrs = Attrs::new().family(Family::Monospace);
                    buffer.set_rich_text(rich, &default_attrs, Shaping::Advanced, None);
                }
                buffer.shape_until_scroll(font_system, false);
                let layout = text_layout_from_buffer(buffer, Color::rgb(50, 50, 50));
                rendered_blocks.push(RenderBlock::Code {
                    layout,
                    x: DEFAULT_MARKDOWN_MARGIN,
                    y,
                    width: content_width,
                    padding: 10,
                });
                y += rendered_blocks.last().unwrap().height() + 6;
            }
            Block::Math { text, display } => {
                let rendered = math::render_math(text, font_system, font_size as f32, *display)
                    .map_err(|err| format!("failed to render math: {err}"))?;
                let image = scale_markdown_image_to_width(&rendered.image, content_width);
                let x = DEFAULT_MARKDOWN_MARGIN + content_width.saturating_sub(image.width()) / 2;
                rendered_blocks.push(RenderBlock::Image { image, x, y });
                y += rendered_blocks.last().unwrap().height() + (font_size * 0.6) as u32;
            }
            Block::Rule => {
                rendered_blocks.push(RenderBlock::Rule {
                    x: DEFAULT_MARKDOWN_MARGIN,
                    y: y + 4,
                    width: content_width,
                });
                y += 12;
            }
            Block::Table { header, rows } => {
                let n_cols = header
                    .len()
                    .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
                if n_cols == 0 {
                    continue;
                }

                let border = 1_u32;
                let padding = (font_size * 0.6).max(8.0) as u32;
                let total_borders = border * (n_cols as u32 + 1);
                let col_width =
                    ((content_width.saturating_sub(total_borders)) / n_cols as u32).max(60);
                let cell_inner = col_width.saturating_sub(padding * 2);
                let table_w = col_width * n_cols as u32 + total_borders;

                // Layout each row, determine heights
                let mut all_row_data: Vec<(Vec<InlineLayout>, u32, bool)> = Vec::new();
                let row_pairs: Vec<(&Vec<Vec<InlineToken>>, bool)> =
                    std::iter::once((header as &Vec<Vec<InlineToken>>, true))
                        .chain(rows.iter().map(|r| (r as &Vec<Vec<InlineToken>>, false)))
                        .collect();

                for (row_tokens, is_header) in &row_pairs {
                    let mut cell_layouts = Vec::new();
                    let mut max_h = 0_u32;
                    for cell_toks in row_tokens.iter().take(n_cols) {
                        let layout = layout_inline_tokens_with_defaults(
                            font_system,
                            cell_toks,
                            cell_inner,
                            font_size as f32,
                            *is_header,
                        )?;
                        max_h = max_h.max(layout.height);
                        cell_layouts.push(layout);
                    }
                    // Pad missing cells
                    while cell_layouts.len() < n_cols {
                        cell_layouts.push(layout_inline_tokens(
                            font_system,
                            &[],
                            cell_inner,
                            font_size as f32,
                        )?);
                    }
                    all_row_data.push((cell_layouts, max_h + padding * 2, *is_header));
                }

                // Emit RenderBlocks for each row
                for (cell_layouts, row_h, is_header) in all_row_data {
                    // Top row border
                    rendered_blocks.push(RenderBlock::Rect {
                        x: DEFAULT_MARKDOWN_MARGIN,
                        y,
                        width: table_w,
                        height: border,
                        color: Rgba([200, 200, 200, 255]),
                    });
                    // Header shading
                    if is_header {
                        rendered_blocks.push(RenderBlock::Rect {
                            x: DEFAULT_MARKDOWN_MARGIN + border,
                            y: y + border,
                            width: table_w.saturating_sub(2 * border),
                            height: row_h,
                            color: Rgba([240, 240, 240, 255]),
                        });
                    }
                    // Cells
                    let mut cx = DEFAULT_MARKDOWN_MARGIN;
                    for layout in cell_layouts {
                        // Left cell border
                        rendered_blocks.push(RenderBlock::Rect {
                            x: cx,
                            y,
                            width: border,
                            height: row_h + border,
                            color: Rgba([200, 200, 200, 255]),
                        });
                        cx += border;
                        rendered_blocks.push(RenderBlock::Inline {
                            layout,
                            x: cx + padding,
                            y: y + border + padding,
                        });
                        cx += col_width;
                    }
                    // Right border
                    rendered_blocks.push(RenderBlock::Rect {
                        x: cx,
                        y,
                        width: border,
                        height: row_h + border,
                        color: Rgba([200, 200, 200, 255]),
                    });
                    y += border + row_h;
                }
                // Bottom border
                rendered_blocks.push(RenderBlock::Rect {
                    x: DEFAULT_MARKDOWN_MARGIN,
                    y,
                    width: table_w,
                    height: border,
                    color: Rgba([200, 200, 200, 255]),
                });
                y += border + (font_size * 0.5) as u32;
            }
        }
    }

    let total_height = (y + DEFAULT_MARKDOWN_MARGIN).max(DEFAULT_MARKDOWN_MARGIN + 50);
    Ok((rendered_blocks, total_height))
}

// Allocate a Pixmap of exactly `draw_height` rows starting at `y_start` in
// document coordinates, draw only the blocks that overlap that slice, and
// return the result as a DynamicImage.  For a full-document render pass
// `y_start=0` and `draw_height=total_height`.
fn draw_blocks_page(
    rendered_blocks: &mut [RenderBlock],
    font_system: &mut FontSystem,
    swash: &mut SwashCache,
    width: u32,
    y_start: u32,
    draw_height: u32,
) -> Result<DynamicImage, Box<dyn std::error::Error>> {
    let draw_height = draw_height.max(1);
    let mut pixmap = Pixmap::new(width, draw_height).ok_or("failed to allocate pixmap")?;
    sk_fill_rect(
        &mut pixmap,
        0,
        0,
        width,
        draw_height,
        Rgba([255, 255, 255, 255]),
    );
    let y_end = y_start.saturating_add(draw_height);
    for block in rendered_blocks.iter_mut() {
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

fn highlight_code_spans(lang: &str, text: &str) -> Vec<(String, u8, u8, u8, bool)> {
    use syntect::easy::HighlightLines;
    use syntect::highlighting::FontStyle;
    use syntect::util::LinesWithEndings;

    let ss = SYNTAX_SET.get_or_init(syntect::parsing::SyntaxSet::load_defaults_nonewlines);
    let ts = THEME_SET.get_or_init(syntect::highlighting::ThemeSet::load_defaults);
    let theme = ts
        .themes
        .get("InspiredGitHub")
        .or_else(|| ts.themes.values().next())
        .unwrap();
    let syntax = ss
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut hl = HighlightLines::new(syntax, theme);
    let mut result = Vec::new();
    for line in LinesWithEndings::from(text) {
        match hl.highlight_line(line, ss) {
            Ok(ranges) => {
                for (style, s) in &ranges {
                    result.push((
                        s.to_string(),
                        style.foreground.r,
                        style.foreground.g,
                        style.foreground.b,
                        style.font_style.contains(FontStyle::BOLD),
                    ));
                }
            }
            Err(_) => {
                result.push((line.to_string(), 50, 50, 50, false));
            }
        }
    }
    if result.is_empty() {
        result.push((String::new(), 0, 0, 0, false));
    }
    result
}

// ── Render primitives ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
enum FontKind {
    Regular,
    Bold,
    Mono,
}

#[derive(Debug)]
struct TextLayout {
    width: u32,
    height: u32,
    buffer: Buffer,
    color: Color,
    lines: Vec<TextLineRange>,
}

#[derive(Debug)]
struct TextLineRange {
    line_i: usize,
    layout_i: usize,
    top: f32,
    bottom: f32,
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
    kind: FontKind,
) -> Result<TextLayout, Box<dyn std::error::Error>> {
    layout_text_with_attrs(font_system, text, width, size, attrs_for_kind(kind, None))
}

fn layout_text_with_attrs(
    font_system: &mut FontSystem,
    text: &str,
    width: u32,
    size: f32,
    attrs: Attrs,
) -> Result<TextLayout, Box<dyn std::error::Error>> {
    let mut buffer = Buffer::new(font_system, Metrics::new(size, size * 1.4));
    buffer.set_size(Some(width as f32), None);
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    Ok(text_layout_from_buffer(buffer, Color::rgb(0, 0, 0)))
}

fn attrs_for_kind(kind: FontKind, color: Option<u32>) -> Attrs<'static> {
    let mut attrs = match kind {
        FontKind::Regular => Attrs::new().family(Family::SansSerif),
        FontKind::Bold => Attrs::new().family(Family::SansSerif).weight(Weight::BOLD),
        FontKind::Mono => Attrs::new().family(Family::Monospace),
    };
    if let Some(color) = color {
        attrs = attrs.color(Color::rgb(
            ((color >> 16) & 0xff) as u8,
            ((color >> 8) & 0xff) as u8,
            (color & 0xff) as u8,
        ));
    }
    attrs
}

fn text_layout_from_buffer(buffer: Buffer, color: Color) -> TextLayout {
    let mut text_width = 0_f32;
    let mut text_height = 0_f32;
    let mut lines = Vec::new();
    let mut current_line = None;
    let mut layout_i = 0;
    for run in buffer.layout_runs() {
        text_width = text_width.max(run.line_w);
        if current_line == Some(run.line_i) {
            layout_i += 1;
        } else {
            current_line = Some(run.line_i);
            layout_i = 0;
        }
        let bottom = run.line_top + run.line_height;
        text_height = text_height.max(bottom);
        lines.push(TextLineRange {
            line_i: run.line_i,
            layout_i,
            top: run.line_top,
            bottom,
        });
    }
    TextLayout {
        width: text_width.ceil() as u32,
        height: text_height.ceil() as u32,
        buffer,
        color,
        lines,
    }
}

fn layout_inline_tokens(
    font_system: &mut FontSystem,
    tokens: &[InlineToken],
    width: u32,
    font_size: f32,
) -> Result<InlineLayout, Box<dyn std::error::Error>> {
    layout_inline_tokens_with_defaults(font_system, tokens, width, font_size, false)
}

fn layout_inline_tokens_with_defaults(
    font_system: &mut FontSystem,
    tokens: &[InlineToken],
    width: u32,
    font_size: f32,
    default_bold: bool,
) -> Result<InlineLayout, Box<dyn std::error::Error>> {
    let space_width = measure_inline_text_width(font_system, " ", font_size, FontKind::Regular);
    let line_height = (font_size * 1.4).ceil() as u32;
    let mut lines: Vec<InlineLine> = vec![InlineLine::default()];

    for token in tokens {
        match token {
            InlineToken::Text {
                text,
                bold,
                italic: _,
                mono,
                color,
                underline: _,
            } => {
                let kind = if *mono {
                    FontKind::Mono
                } else if *bold || default_bold {
                    FontKind::Bold
                } else {
                    FontKind::Regular
                };
                for (idx, part) in text.split_inclusive(char::is_whitespace).enumerate() {
                    if part.is_empty() {
                        continue;
                    }
                    let part = if idx == 0 { part } else { part.trim_start() };
                    if part.is_empty() {
                        continue;
                    }
                    push_inline_text(
                        font_system,
                        &mut lines,
                        part,
                        width,
                        font_size,
                        kind,
                        *color,
                    )?;
                }
            }
            InlineToken::Math { text, display } => {
                let rendered = math::render_math(text, font_system, font_size, *display)
                    .map_err(|err| format!("failed to render math: {err}"))?;
                push_inline_image(&mut lines, rendered.image, rendered.baseline, width);
            }
            InlineToken::SoftBreak => {
                let current = lines.last_mut().unwrap();
                current.width += space_width;
            }
            InlineToken::HardBreak => lines.push(InlineLine::default()),
            InlineToken::Image { .. } => {}
        }
    }

    let mut items = Vec::new();
    let mut y = 0_u32;
    for line in lines {
        if line.items.is_empty() {
            y += line_height;
            continue;
        }
        let baseline = line.baseline.max((font_size * 0.9) as u32);
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

fn push_inline_text(
    font_system: &mut FontSystem,
    lines: &mut Vec<InlineLine>,
    text: &str,
    max_width: u32,
    font_size: f32,
    kind: FontKind,
    color: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let layout = layout_text_with_attrs(
        font_system,
        text,
        max_width,
        font_size,
        attrs_for_kind(kind, color),
    )?;
    if lines.last().unwrap().width > 0
        && lines.last().unwrap().width.saturating_add(layout.width) > max_width
    {
        lines.push(InlineLine::default());
    }
    let line = lines.last_mut().unwrap();
    let x = line.width;
    line.width = line.width.saturating_add(layout.width);
    let baseline = (font_size * 0.9) as u32;
    line.baseline = line.baseline.max(baseline);
    line.descent = line.descent.max(layout.height.saturating_sub(baseline));
    line.items.push(InlineLineItem::Text {
        layout,
        x,
        baseline,
    });
    Ok(())
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

fn measure_inline_text_width(
    font_system: &mut FontSystem,
    text: &str,
    size: f32,
    kind: FontKind,
) -> u32 {
    layout_text(font_system, text, u32::MAX / 2, size, kind)
        .map(|layout| layout.width)
        .unwrap_or(0)
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
        padding: u32,
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
            Self::Code {
                layout, padding, ..
            } => layout.height + padding * 2 + 6,
            Self::Image { image, .. } => image.height(),
            Self::Rule { .. } => 2,
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
        &mut self,
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
                padding,
            } => {
                let y_adj = *y as i32 - y_offset;
                sk_fill_rect_signed(
                    pixmap,
                    *x as i32,
                    y_adj,
                    *width,
                    layout.height + *padding * 2 + 6,
                    Rgba([245, 245, 245, 255]),
                );
                let text_top = *padding as i32;
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
                        (*x + *padding) as i32,
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
                sk_fill_rect_signed(
                    pixmap,
                    *x as i32,
                    *y as i32 - y_offset,
                    *width,
                    2,
                    Rgba([220, 220, 220, 255]),
                );
            }
            Self::Rect {
                x,
                y,
                width,
                height,
                color,
            } => {
                sk_fill_rect_signed(
                    pixmap,
                    *x as i32,
                    *y as i32 - y_offset,
                    *width,
                    *height,
                    *color,
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
    layout: &mut TextLayout,
    x: i32,
    y: i32,
    visible_top: u32,
    visible_bottom: u32,
) {
    if visible_top >= visible_bottom || layout.lines.is_empty() {
        return;
    }
    layout.buffer.shape_until_scroll(ctx.font_system, false);
    let Some((first, last)) = text_visible_range(layout, visible_top as f32, visible_bottom as f32)
    else {
        return;
    };
    let mut renderer = TextPixmapRenderer {
        pixmap: &mut *ctx.pixmap,
        font_system: &mut *ctx.font_system,
        swash: &mut *ctx.swash,
        x,
        y,
    };
    let line_height = layout.buffer.metrics().line_height;
    for range in &layout.lines[first..last] {
        let Some(line) = layout.buffer.lines.get(range.line_i) else {
            continue;
        };
        let Some(layout_lines) = line.layout_opt() else {
            continue;
        };
        let Some(layout_line) = layout_lines.get(range.layout_i) else {
            continue;
        };
        let run_line_height = layout_line.line_height_opt.unwrap_or(line_height);
        let glyph_height = layout_line.max_ascent + layout_line.max_descent;
        let line_y = range.top + (run_line_height - glyph_height) / 2.0 + layout_line.max_ascent;
        let Some(shape) = line.shape_opt() else {
            continue;
        };
        let run = LayoutRun {
            line_i: range.line_i,
            text: line.text(),
            rtl: shape.rtl,
            glyphs: &layout_line.glyphs,
            decorations: &layout_line.decorations,
            line_y,
            line_top: range.top,
            line_height: run_line_height,
            line_w: layout_line.w,
        };
        for glyph in run.glyphs {
            if glyph_is_missing(renderer.font_system, glyph) {
                continue;
            }
            let physical_glyph = glyph.physical((0.0, run.line_y), 1.0);
            let glyph_color = glyph.color_opt.map_or(layout.color, |some| some);
            renderer.glyph(physical_glyph, glyph_color);
        }
        render_decoration(&mut renderer, &run, layout.color);
    }
}

fn draw_inline_layout(
    ctx: &mut MarkdownDrawContext<'_>,
    layout: &mut InlineLayout,
    x: i32,
    y: i32,
    visible_top: u32,
    visible_bottom: u32,
) {
    if visible_top >= visible_bottom {
        return;
    }
    for item in &mut layout.items {
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

fn text_visible_range(
    layout: &TextLayout,
    visible_top: f32,
    visible_bottom: f32,
) -> Option<(usize, usize)> {
    let first = layout
        .lines
        .partition_point(|line| line.bottom <= visible_top);
    let line = layout.lines.get(first)?;
    if line.top >= visible_bottom {
        return None;
    }
    let last = layout.lines[first..].partition_point(|line| line.top < visible_bottom);
    Some((first, first + last))
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
        fill_cosmic_rect(self.pixmap, self.x + x, self.y + y, width, height, color);
    }

    fn glyph(&mut self, physical_glyph: PhysicalGlyph, color: Color) {
        let base_x = self.x + physical_glyph.x;
        let base_y = self.y + physical_glyph.y;
        let pixmap = &mut *self.pixmap;
        self.swash.with_pixels(
            self.font_system,
            physical_glyph.cache_key,
            color,
            |gx, gy, pixel_color| {
                fill_cosmic_rect(pixmap, base_x + gx, base_y + gy, 1, 1, pixel_color);
            },
        );
    }
}

fn fill_cosmic_rect(pixmap: &mut Pixmap, x: i32, y: i32, width: u32, height: u32, color: Color) {
    let a = color.a();
    if a == 0 || width == 0 || height == 0 {
        return;
    }
    let pw = pixmap.width() as i64;
    let ph = pixmap.height() as i64;
    let x0 = i64::from(x).max(0).min(pw);
    let y0 = i64::from(y).max(0).min(ph);
    let x1 = i64::from(x).saturating_add(i64::from(width)).max(0).min(pw);
    let y1 = i64::from(y)
        .saturating_add(i64::from(height))
        .max(0)
        .min(ph);
    let draw_w = x1 - x0;
    let draw_h = y1 - y0;
    if draw_w <= 0 || draw_h <= 0 {
        return;
    }
    if let Some(rect) = SkRect::from_xywh(x0 as f32, y0 as f32, draw_w as f32, draw_h as f32) {
        let mut paint = SkPaint {
            blend_mode: BlendMode::SourceOver,
            ..SkPaint::default()
        };
        paint.set_color_rgba8(color.r(), color.g(), color.b(), a);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

fn sk_fill_rect(pixmap: &mut Pixmap, x: u32, y: u32, width: u32, height: u32, color: Rgba<u8>) {
    if width == 0 || height == 0 {
        return;
    }
    let x = x.min(pixmap.width()) as f32;
    let y = y.min(pixmap.height()) as f32;
    let w = (width as f32).min(pixmap.width() as f32 - x);
    let h = (height as f32).min(pixmap.height() as f32 - y);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    if let Some(rect) = SkRect::from_xywh(x, y, w, h) {
        let mut paint = SkPaint {
            blend_mode: BlendMode::SourceOver,
            ..SkPaint::default()
        };
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

// Like sk_fill_rect but accepts signed coordinates, handling negative y when a
// block is partially above the current page slice.
fn sk_fill_rect_signed(
    pixmap: &mut Pixmap,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: Rgba<u8>,
) {
    if width == 0 || height == 0 {
        return;
    }
    let pw = pixmap.width() as i32;
    let ph = pixmap.height() as i32;
    let x0 = x.max(0).min(pw) as f32;
    let y0 = y.max(0).min(ph) as f32;
    let x1 = (x + width as i32).min(pw).max(0) as f32;
    let y1 = (y + height as i32).min(ph).max(0) as f32;
    let w = x1 - x0;
    let h = y1 - y0;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    if let Some(rect) = SkRect::from_xywh(x0, y0, w, h) {
        let mut paint = SkPaint {
            blend_mode: BlendMode::SourceOver,
            ..SkPaint::default()
        };
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
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
            let src = image.get_pixel(src_x, src_y);
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
    let data = imgutil::read_source(path.to_str().unwrap_or_default())?;
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
        let chunk = image.crop_imm(rect.0, rect.1, rect.2, rect.3);
        let png = imgutil::encode_png(&chunk)?;
        let prepared = crate::display::image::prepare_image(&png, size.pixel_width)?;
        crate::kitty::send_static_image(
            &prepared.png_data,
            prepared.width,
            prepared.height,
            size,
            tmux,
        )?;
    }
    Ok(())
}

fn scale_markdown_image_to_width(image: &DynamicImage, max_width: u32) -> DynamicImage {
    if image.width() <= max_width || max_width == 0 {
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

pub fn markdown_font_size(value: f64) -> f64 {
    if value <= 0.0 {
        DEFAULT_MARKDOWN_FONT_PT
    } else {
        value
    }
}

pub fn markdown_page_height(size: Size) -> u32 {
    let mut height = if size.pixel_height > 0 {
        size.pixel_height
    } else if size.rows > 0 {
        size.rows * term::DEFAULT_CELL_HEIGHT
    } else {
        384
    };
    let reserved = if size.rows > 0 && size.pixel_height > 0 {
        2 * (size.pixel_height / size.rows.max(1)).max(1)
    } else {
        2 * term::DEFAULT_CELL_HEIGHT
    };
    height = height.saturating_sub(reserved);
    height.max(120)
}

pub fn markdown_total_pages(total_height: u32, page_height: u32) -> usize {
    if total_height == 0 || page_height == 0 {
        1
    } else {
        total_height.div_ceil(page_height) as usize
    }
}

pub fn markdown_should_paginate(bounds: (u32, u32), size: Size) -> bool {
    bounds.1 > markdown_page_height(size)
}

pub fn markdown_chunk_max_height(width: u32) -> u32 {
    if width == 0 {
        return 1;
    }
    let by_pixels = (imgutil::MAX_PIXELS / width as u64) as u32;
    let by_bytes = (imgutil::MAX_RGBA_BYTES / 4 / width as u64) as u32;
    by_pixels.min(by_bytes).clamp(1, MARKDOWN_CHUNK_HEIGHT)
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
    use image::GenericImage;

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

    #[test]
    fn markdown_render_width_cases() {
        assert_eq!(markdown_render_width(0), DEFAULT_MARKDOWN_WIDTH);
        assert_eq!(markdown_render_width(320), MIN_MARKDOWN_WIDTH);
        assert_eq!(markdown_render_width(800), 800);
        assert_eq!(markdown_render_width(1600), DEFAULT_MARKDOWN_WIDTH);
    }

    #[test]
    fn render_markdown_produces_image() {
        let image = render_markdown(
            b"# Hello\n\n- one\n- two\n\n```go\nfmt.Println(\"hi\")\n```\n",
            Path::new(""),
            800,
        )
        .unwrap();
        assert_eq!(image.width(), 800);
        assert!(image.height() > 0);
        let encoded = imgutil::encode_png(&image).unwrap();
        assert!(imgutil::is_png(&encoded));
    }

    #[test]
    fn render_markdown_produces_image_with_chinese_text() {
        let image = render_markdown(
            "# 中文\n\n| 银行 | 金额 |\n| --- | --- |\n| 建行 | 3381.96 |\n".as_bytes(),
            Path::new(""),
            800,
        )
        .unwrap();
        assert_eq!(image.width(), 800);
        assert!(image.height() > 0);
    }

    #[test]
    fn render_markdown_with_font_size_affects_layout() {
        let data = b"# Title\n\nParagraph text that wraps enough to make font size visible in layout.\n\n- one\n- two\n";
        let default = render_markdown(data, Path::new(""), 800).unwrap();
        let large = render_markdown_with_font_size(data, Path::new(""), 800, 24.0).unwrap();
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
        assert_eq!(image.width(), 800);
        assert!(image.height() > 120);
    }

    #[test]
    fn render_markdown_with_mermaid() {
        let md = b"```mermaid\nflowchart TD\n    A[Start] --> B{Decision}\n    B -->|Yes| C[Continue]\n    B -->|No| D[Retry]\n```\n";
        let image = render_markdown(md, Path::new(""), 800).unwrap();
        assert_eq!(image.width(), 800);
        assert!(image.height() > 180);
    }

    #[test]
    fn render_markdown_with_sequence_mermaid_uses_diagram_layout() {
        let md = b"```mermaid\nsequenceDiagram\n    participant User\n    participant System\n    participant Database\n\n    User->>System: Login Request\n    System->>Database: Query User\n    Database-->>System: User Data\n    System-->>User: Login Success\n```\n";
        let image = render_markdown(md, Path::new(""), 800).unwrap();

        assert_eq!(image.width(), 800);
        assert!(
            image.height() > 280,
            "sequence Mermaid should render as a diagram, not compact text fallback"
        );
    }

    #[test]
    fn render_markdown_with_markie_mermaid_demo_types() {
        let diagrams: &[(&str, &[u8], u32)] = &[
            (
                "class",
                b"```mermaid\nclassDiagram\n    class Animal {\n        +String name\n        +makeSound()\n    }\n    class Dog {\n        +bark()\n    }\n    Animal <|-- Dog\n```\n",
                240,
            ),
            (
                "state",
                b"```mermaid\nstateDiagram\n    [*] --> Idle\n    Idle --> Loading: Load Data\n    Loading --> Success: Complete\n    Success --> [*]\n```\n",
                220,
            ),
            (
                "er",
                b"```mermaid\nerDiagram\n    CUSTOMER ||--o{ ORDER : places\n    ORDER ||--|{ LINE_ITEM : contains\n    CUSTOMER {\n        int id\n        string name\n    }\n```\n",
                220,
            ),
        ];

        for (name, md, min_height) in diagrams {
            let image = render_markdown(md, Path::new(""), 800).unwrap();
            assert_eq!(image.width(), 800, "{name} diagram width");
            assert!(
                image.height() > *min_height,
                "{name} Mermaid should render as a diagram, not compact text fallback"
            );
        }
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
    fn render_markdown_page_detailed_matches_non_first_code_slice() {
        let lines = (0..80)
            .map(|i| format!("let value_{i} = {i};\n"))
            .collect::<String>();
        let markdown = format!("```rust\n{lines}```\n");
        let page_height = 160;
        let full = render_markdown(markdown.as_bytes(), Path::new(""), 800).unwrap();
        assert!(full.height() > page_height * 2);

        let page = render_markdown_page_detailed(
            markdown.as_bytes(),
            Path::new(""),
            800,
            1,
            page_height,
            0.0,
        )
        .unwrap();
        let expected = full.crop_imm(0, page_height, full.width(), page_height);

        assert_eq!(page.dimensions(), expected.dimensions());
        assert_eq!(page.to_rgba8().as_raw(), expected.to_rgba8().as_raw());
    }

    #[test]
    fn render_markdown_page_detailed_matches_non_first_image_slice() {
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

        let page =
            render_markdown_page_detailed(markdown, dir.path(), 800, 1, page_height, 0.0).unwrap();
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
    fn markdown_pagination_helpers() {
        let size = Size {
            pixel_width: 800,
            pixel_height: 600,
            cols: 100,
            rows: 30,
        };
        let height = markdown_page_height(size);
        assert!(height < 600 && height > 0);
        assert_eq!(markdown_total_pages(height * 2 + 1, height), 3);
        assert!(markdown_should_paginate((800, height + 1), size));
        assert!(!markdown_should_paginate((800, height), size));
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
    fn markdown_base_dir_cases() {
        assert_eq!(markdown_base_dir(""), PathBuf::new());
        assert_eq!(markdown_base_dir("docs/README.md"), PathBuf::from("docs"));
    }

    #[test]
    fn markdown_font_size_default_and_override() {
        assert_eq!(markdown_font_size(0.0), DEFAULT_MARKDOWN_FONT_PT);
        assert_eq!(markdown_font_size(20.5), 20.5);
    }

    #[test]
    fn render_markdown_uses_base_dir_for_images() {
        let dir = tempfile::tempdir().unwrap();
        let image_path = dir.path().join("inline.png");
        let mut image = DynamicImage::new_rgba8(32, 24);
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        std::fs::write(&image_path, imgutil::encode_png(&image).unwrap()).unwrap();
        let rendered = render_markdown(b"![alt](inline.png)\n", dir.path(), 800).unwrap();
        assert_eq!(rendered.width(), 800);
        assert!(rendered.height() > 0);
    }

    #[test]
    fn parse_markdown_document_produces_ast() {
        let md = b"# Hello\n\nA paragraph with **bold** and *italic* text.\n\n> Quoted block\n\n| Col1 | Col2 |\n|------|------|\n| a    | b    |\n";
        let blocks = parse_markdown_blocks(md);

        // Block 0: Heading level 1
        assert!(
            matches!(&blocks[0], Block::Heading { level: 1, .. }),
            "expected Heading(1), got {:?}",
            blocks[0]
        );
        if let Block::Heading { tokens, .. } = &blocks[0] {
            assert_eq!(flatten_tokens(tokens).trim(), "Hello");
        }

        // Block 1: Paragraph with bold and italic spans
        assert!(
            matches!(&blocks[1], Block::Paragraph(_)),
            "expected Paragraph"
        );
        if let Block::Paragraph(tokens) = &blocks[1] {
            let has_bold = tokens
                .iter()
                .any(|t| matches!(t, InlineToken::Text { bold: true, .. }));
            let has_italic = tokens
                .iter()
                .any(|t| matches!(t, InlineToken::Text { italic: true, .. }));
            assert!(has_bold, "paragraph should contain a bold span");
            assert!(has_italic, "paragraph should contain an italic span");
        }

        // Block 2: BlockQuote
        assert!(
            matches!(&blocks[2], Block::BlockQuote(_)),
            "expected BlockQuote, got {:?}",
            blocks[2]
        );

        // Block 3: Table with 2-column header and 1 data row
        assert!(
            matches!(&blocks[3], Block::Table { .. }),
            "expected Table, got {:?}",
            blocks[3]
        );
        if let Block::Table { header, rows } = &blocks[3] {
            assert_eq!(header.len(), 2, "table should have 2 header columns");
            assert_eq!(rows.len(), 1, "table should have 1 data row");
            assert_eq!(rows[0].len(), 2, "data row should have 2 cells");
        }
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
    fn inline_tokens_emphasis_escalation() {
        // Bold inside italic → bold+italic for nested text
        let md = b"*outer **inner** end*\n";
        let blocks = parse_markdown_blocks(md);
        if let Block::Paragraph(tokens) = &blocks[0] {
            let inner = tokens.iter().find(|t| {
                matches!(t, InlineToken::Text { text, bold: true, italic: true, .. } if text == "inner")
            });
            assert!(inner.is_some(), "nested bold-italic should have both flags");
        }
    }

    #[test]
    fn inline_tokens_link_color_and_underline() {
        let md = b"[click here](https://example.com)\n";
        let blocks = parse_markdown_blocks(md);
        if let Block::Paragraph(tokens) = &blocks[0] {
            let link_tok = tokens.iter().find(|t| {
                matches!(
                    t,
                    InlineToken::Text {
                        underline: true,
                        color: Some(0x064FBD),
                        ..
                    }
                )
            });
            assert!(
                link_tok.is_some(),
                "link text should have 0x064FBD color and underline"
            );
        }
    }

    #[test]
    fn render_markdown_tight_list_smaller_than_loose() {
        let tight_md = b"- alpha\n- beta\n- gamma\n";
        let loose_md = b"- alpha\n\n- beta\n\n- gamma\n";
        let tight = render_markdown(tight_md, Path::new(""), 800).unwrap();
        let loose = render_markdown(loose_md, Path::new(""), 800).unwrap();
        assert!(
            loose.height() >= tight.height(),
            "loose list should be at least as tall as tight list"
        );
    }

    #[test]
    fn render_markdown_with_blockquote() {
        let md = b"# Title\n\n> This is a blockquote with some text.\n\nNormal paragraph.\n";
        let image = render_markdown(md, Path::new(""), 800).unwrap();
        assert_eq!(image.width(), 800);
        assert!(image.height() > 0);
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
            two_rows.height() >= one_row.height(),
            "more rows should produce taller image"
        );
    }

    #[test]
    fn cosmic_text_rich_text_compiles() {
        let mut fs = FontSystem::new();
        let mut buf = Buffer::new(&mut fs, Metrics::new(12.0, 16.0));
        buf.set_size(Some(400.0), None);
        let spans: Vec<(&str, Attrs)> = vec![
            ("hello ", Attrs::new().family(Family::Monospace)),
            (
                "world",
                Attrs::new()
                    .family(Family::Monospace)
                    .color(Color::rgb(255, 0, 0)),
            ),
        ];
        let default_attrs = Attrs::new().family(Family::Monospace);
        buf.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut fs, false);
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
}
