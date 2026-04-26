use std::error::Error;
use std::fmt;
use std::path::Path;

use glob::glob;

pub const DEFAULT_MARKDOWN_FONT_PT: f64 = 18.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Auto,
    Markdown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cli {
    pub page: Option<usize>,
    pub font_size_pt: f64,
    pub kind: InputKind,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub path: String,
    pub page: Option<usize>,
    pub font_size_pt: f64,
    pub kind: InputKind,
}

#[derive(Debug)]
pub struct HelpRequested;

impl fmt::Display for HelpRequested {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("help requested")
    }
}

impl Error for HelpRequested {}

pub fn is_help_error(err: &(dyn Error + 'static)) -> bool {
    err.is::<HelpRequested>()
}

pub fn parse_cli(args: &[String]) -> Result<Cli, Box<dyn Error>> {
    let args = normalize_page_args(args);
    let mut page = None;
    let mut font_size_pt = DEFAULT_MARKDOWN_FONT_PT;
    let mut kind = InputKind::Auto;
    let mut files = Vec::with_capacity(args.len());

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(value) = arg.strip_prefix("--md-font-size=") {
            font_size_pt = parse_markdown_font_size(value)?;
            i += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--markdown-font-size=") {
            font_size_pt = parse_markdown_font_size(value)?;
            i += 1;
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                return Err(Box::new(HelpRequested));
            }
            "--markdown" => kind = InputKind::Markdown,
            "--md-font-size" | "--markdown-font-size" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| format!("missing value for {arg}"))?;
                font_size_pt = parse_markdown_font_size(value)?;
            }
            "-p" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| String::from("missing value for -p"))?;
                page = Some(parse_page(value)?);
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}").into()),
            _ => files.push(arg.clone()),
        }
        i += 1;
    }

    Ok(Cli {
        page,
        font_size_pt,
        kind,
        files,
    })
}

fn parse_page(value: &str) -> Result<usize, Box<dyn Error>> {
    let page = value
        .parse::<usize>()
        .map_err(|_| format!("invalid -p value \"{value}\""))?;
    if page < 1 {
        return Err(String::from("-p must be >= 1").into());
    }
    Ok(page)
}

fn parse_markdown_font_size(value: &str) -> Result<f64, Box<dyn Error>> {
    let size = value
        .parse::<f64>()
        .map_err(|_| format!("invalid --md-font-size value \"{value}\""))?;
    if size <= 0.0 {
        return Err(String::from("--md-font-size must be > 0").into());
    }
    Ok(size)
}

pub fn normalize_page_args(args: &[String]) -> Vec<String> {
    let mut normalized = Vec::with_capacity(args.len());
    for arg in args {
        if let Some(value) = arg.strip_prefix("-p")
            && !value.is_empty()
            && all_digits(value)
        {
            normalized.push(String::from("-p"));
            normalized.push(value.to_string());
        } else {
            normalized.push(arg.clone());
        }
    }
    normalized
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

pub fn build_sources(cli: &Cli) -> Vec<Source> {
    let mut sources = Vec::new();
    for arg in &cli.files {
        if arg == "-" {
            sources.push(Source {
                path: String::new(),
                page: cli.page,
                font_size_pt: cli.font_size_pt,
                kind: cli.kind,
            });
            continue;
        }
        for path in expand_glob(arg) {
            sources.push(Source {
                path,
                page: cli.page,
                font_size_pt: cli.font_size_pt,
                kind: cli.kind,
            });
        }
    }
    sources
}

pub fn expand_glob(arg: &str) -> Vec<String> {
    if !arg.contains(['*', '?', '[']) {
        return vec![arg.to_string()];
    }
    match glob(arg) {
        Ok(paths) => {
            let matches: Vec<String> = paths
                .filter_map(Result::ok)
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            if matches.is_empty() {
                vec![arg.to_string()]
            } else {
                matches
            }
        }
        Err(_) => vec![arg.to_string()],
    }
}

pub fn is_markdown_path(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase()),
        Some(ext) if ext == "md" || ext == "markdown"
    )
}

pub fn has_image_path_extension(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase()),
        Some(ext)
            if matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tiff" | "tif"
            )
    )
}

pub fn bytes_has_prefix(data: &[u8], prefix: &[u8]) -> bool {
    data.starts_with(prefix)
}

pub fn sanitize_control_chars(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if (ch < '\u{20}' || ch == '\u{7f}') && ch != '\n' && ch != '\t' {
                '?'
            } else {
                ch
            }
        })
        .collect()
}

pub fn safe_err(err: &dyn Error) -> String {
    sanitize_control_chars(&err.to_string())
}

pub fn print_usage() {
    eprintln!(
        "Usage: icat [--markdown] [--md-font-size N] [-pN | -p N] [files/patterns...]\n\nDisplay images in the terminal using Kitty graphics protocol.\n\nOptions:\n  --markdown         Treat input as Markdown and render it to an image\n  --md-font-size N   Markdown base font size in points (default 18)\n  -p N               PDF page, archive index, or Markdown page (1-based)\n  -                  Read from stdin\n  -h                 Show this help\n\nExamples:\n  icat image.png\n  icat *.jpg\n  icat document.pdf\n  icat README.md\n  cat README.md | icat --markdown\n  icat -p3 document.pdf\n  icat photos.zip\n  icat -p 2 photos.zip\n  cat image.png | icat"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn expand_glob_plain() {
        assert_eq!(expand_glob("image.png"), vec!["image.png"]);
    }

    #[test]
    fn expand_glob_no_matches() {
        assert_eq!(
            expand_glob("*.nonexistent_xyz_suffix"),
            vec!["*.nonexistent_xyz_suffix"]
        );
    }

    #[test]
    fn expand_glob_expansion() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("icat_test_glob_a.txt");
        let b = dir.path().join("icat_test_glob_b.txt");
        fs::write(&a, []).unwrap();
        fs::write(&b, []).unwrap();
        let got = expand_glob(&dir.path().join("icat_test_glob_*.txt").to_string_lossy());
        assert!(got.len() >= 2, "got {got:?}");
    }

    #[test]
    fn build_sources_stdin_dash() {
        let sources = build_sources(&Cli {
            page: None,
            font_size_pt: DEFAULT_MARKDOWN_FONT_PT,
            kind: InputKind::Auto,
            files: vec![String::from("-")],
        });
        assert_eq!(sources.len(), 1);
        assert!(sources[0].path.is_empty());
    }

    #[test]
    fn build_sources_multiple_files() {
        let sources = build_sources(&Cli {
            page: None,
            font_size_pt: DEFAULT_MARKDOWN_FONT_PT,
            kind: InputKind::Auto,
            files: vec![String::from("a.png"), String::from("b.jpg")],
        });
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].path, "a.png");
        assert_eq!(sources[1].path, "b.jpg");
    }

    #[test]
    fn build_sources_with_page() {
        let sources = build_sources(&Cli {
            page: Some(3),
            font_size_pt: DEFAULT_MARKDOWN_FONT_PT,
            kind: InputKind::Auto,
            files: vec![String::from("doc.pdf")],
        });
        assert_eq!(sources[0].page, Some(3));
    }

    #[test]
    fn build_sources_with_markdown_kind() {
        let sources = build_sources(&Cli {
            page: None,
            font_size_pt: DEFAULT_MARKDOWN_FONT_PT,
            kind: InputKind::Markdown,
            files: vec![String::from("README.md")],
        });
        assert_eq!(sources[0].kind, InputKind::Markdown);
    }

    #[test]
    fn safe_err_replaces_control_chars() {
        let err = std::io::Error::other("bad\0err\n");
        assert_eq!(safe_err(&err), "bad?err\n");
    }

    #[test]
    fn parse_cli_with_attached_page_value() {
        let cli = parse_cli(&[String::from("-p3"), String::from("doc.pdf")]).unwrap();
        assert_eq!(cli.page, Some(3));
    }

    #[test]
    fn parse_cli_with_separated_page_value() {
        let cli = parse_cli(&[
            String::from("-p"),
            String::from("4"),
            String::from("doc.pdf"),
        ])
        .unwrap();
        assert_eq!(cli.page, Some(4));
    }

    #[test]
    fn parse_cli_with_attached_page_value_at_end() {
        let cli = parse_cli(&[String::from("doc.pdf"), String::from("-p3")]).unwrap();
        assert_eq!(cli.page, Some(3));
        assert_eq!(cli.files, vec![String::from("doc.pdf")]);
    }

    #[test]
    fn parse_cli_with_separated_page_value_at_end() {
        let cli = parse_cli(&[
            String::from("doc.pdf"),
            String::from("-p"),
            String::from("4"),
        ])
        .unwrap();
        assert_eq!(cli.page, Some(4));
        assert_eq!(cli.files, vec![String::from("doc.pdf")]);
    }

    #[test]
    fn parse_cli_with_markdown_flag() {
        let cli = parse_cli(&[String::from("--markdown"), String::from("README.md")]).unwrap();
        assert_eq!(cli.kind, InputKind::Markdown);
        assert_eq!(cli.files, vec![String::from("README.md")]);
    }

    #[test]
    fn parse_cli_with_markdown_font_size() {
        let cli = parse_cli(&[
            String::from("--md-font-size"),
            String::from("20"),
            String::from("README.md"),
        ])
        .unwrap();
        assert_eq!(cli.font_size_pt, 20.0);
    }

    #[test]
    fn parse_cli_with_markdown_font_size_equals() {
        let cli = parse_cli(&[
            String::from("--markdown-font-size=21.5"),
            String::from("README.md"),
        ])
        .unwrap();
        assert_eq!(cli.font_size_pt, 21.5);
    }

    #[test]
    fn parse_cli_reports_help_request() {
        let err = parse_cli(&[String::from("--help")]).unwrap_err();
        assert!(is_help_error(err.as_ref()));
    }

    #[test]
    fn parse_cli_rejects_invalid_markdown_font_size() {
        assert!(
            parse_cli(&[
                String::from("--md-font-size"),
                String::from("0"),
                String::from("README.md")
            ])
            .is_err()
        );
    }

    #[test]
    fn parse_cli_rejects_missing_option_values() {
        assert_eq!(
            parse_cli(&[String::from("-p")]).unwrap_err().to_string(),
            "missing value for -p"
        );
        assert_eq!(
            parse_cli(&[String::from("--md-font-size")])
                .unwrap_err()
                .to_string(),
            "missing value for --md-font-size"
        );
    }

    #[test]
    fn parse_cli_rejects_zero_page() {
        assert!(parse_cli(&[String::from("-p0"), String::from("doc.pdf")]).is_err());
    }

    #[test]
    fn markdown_path_detection() {
        assert!(is_markdown_path("README.md"));
        assert!(is_markdown_path("guide.MARKDOWN"));
        assert!(is_markdown_path("notes.markdown"));
        assert!(!is_markdown_path("image.png"));
    }

    #[test]
    fn image_extension_detection() {
        assert!(has_image_path_extension("image.png"));
        assert!(has_image_path_extension("photo.JPEG"));
        assert!(has_image_path_extension("scan.tiff"));
        assert!(!has_image_path_extension("archive.tar"));
    }

    #[test]
    fn build_sources_with_markdown_page() {
        let sources = build_sources(&Cli {
            page: Some(7),
            font_size_pt: DEFAULT_MARKDOWN_FONT_PT,
            kind: InputKind::Markdown,
            files: vec![String::from("README.md")],
        });
        assert_eq!(sources[0].page, Some(7));
    }

    #[test]
    fn build_sources_with_markdown_font_size() {
        let sources = build_sources(&Cli {
            page: None,
            font_size_pt: 20.0,
            kind: InputKind::Markdown,
            files: vec![String::from("README.md")],
        });
        assert_eq!(sources[0].font_size_pt, 20.0);
    }
}
