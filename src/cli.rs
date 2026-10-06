use std::error::Error;
use std::fmt;

use glob::glob;

// Single source of truth lives in the markdown renderer; the CLI default must
// track it, otherwise editing the renderer constant silently has no effect.
use crate::display::markdown::DEFAULT_MARKDOWN_FONT_PT;
use crate::imgutil;

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
    let mut page = None;
    let mut font_size_pt = DEFAULT_MARKDOWN_FONT_PT;
    let mut kind = InputKind::Auto;
    let mut files = Vec::with_capacity(args.len());

    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if let Some((name, value)) = arg.split_once('=')
            && matches!(name, "--md-font-size" | "--markdown-font-size")
        {
            font_size_pt = parse_markdown_font_size(value)?;
            continue;
        }
        // `-p3` is shorthand for `-p 3`.
        if let Some(value) = arg.strip_prefix("-p")
            && !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit())
        {
            page = Some(parse_page(value)?);
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                return Err(Box::new(HelpRequested));
            }
            "--markdown" => kind = InputKind::Markdown,
            "--md-font-size" | "--markdown-font-size" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for {arg}"))?;
                font_size_pt = parse_markdown_font_size(value)?;
            }
            "-p" => {
                let value = args
                    .next()
                    .ok_or_else(|| String::from("missing value for -p"))?;
                page = Some(parse_page(value)?);
            }
            "-" => files.push(arg.clone()),
            _ if arg.starts_with('-') => return Err(format!("unknown option {arg}").into()),
            _ => files.push(arg.clone()),
        }
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

/// Expands each file argument into sources; `-` means stdin (an empty path).
pub fn build_sources(cli: &Cli) -> Vec<Source> {
    cli.files
        .iter()
        .flat_map(|arg| {
            if arg == "-" {
                vec![String::new()]
            } else {
                expand_glob(arg)
            }
        })
        .map(|path| Source {
            path,
            page: cli.page,
            font_size_pt: cli.font_size_pt,
            kind: cli.kind,
        })
        .collect()
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
        imgutil::lowercase_extension(path).as_deref(),
        Some("md" | "markdown")
    )
}

pub fn is_pdf_path(path: &str) -> bool {
    imgutil::lowercase_extension(path).as_deref() == Some("pdf")
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
        "Usage: icat [--markdown] [--md-font-size N] [-pN | -p N] [files/patterns...]\n\nDisplay images in the terminal using Kitty graphics protocol.\n\nOptions:\n  --markdown         Treat input as Markdown and render it to an image\n  --md-font-size N   Markdown base font size in points (default {DEFAULT_MARKDOWN_FONT_PT})\n  -p N               PDF page, archive index, or Markdown page (1-based)\n  -                  Read from stdin\n  -h                 Show this help\n\nExamples:\n  icat image.png\n  icat *.jpg\n  icat document.pdf\n  icat README.md\n  cat README.md | icat --markdown\n  icat -p3 document.pdf\n  icat photos.zip\n  icat -p 2 photos.zip\n  cat image.png | icat"
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
    fn build_sources_maps_dash_to_stdin_and_copies_options() {
        let sources = build_sources(&Cli {
            page: Some(7),
            font_size_pt: 20.0,
            kind: InputKind::Markdown,
            files: vec![String::from("a.md"), String::from("-")],
        });
        let paths: Vec<_> = sources.iter().map(|src| src.path.as_str()).collect();
        assert_eq!(paths, ["a.md", ""]);
        assert!(sources.iter().all(|src| src.page == Some(7)
            && src.font_size_pt == 20.0
            && src.kind == InputKind::Markdown));
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
    fn parse_cli_accepts_dash_as_stdin_file() {
        let cli = parse_cli(&[String::from("-")]).unwrap();
        assert_eq!(cli.files, vec![String::from("-")]);
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
    fn pdf_path_detection_requires_an_extension() {
        assert!(is_pdf_path("paper.PDF"));
        assert!(!is_pdf_path("pdf"));
        assert!(!is_pdf_path("dir/pdf"));
    }
}
