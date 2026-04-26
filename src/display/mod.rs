pub mod archive;
pub mod image;
pub mod markdown;
pub mod pdf;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarkdownOptions {
    pub page: Option<usize>,
    pub font_size_pt: f64,
}
