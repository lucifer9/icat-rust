use std::process;

use icat::cli::{InputKind, Source};
use icat::{cli, display, imgutil, term};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match cli::parse_cli(&args) {
        Ok(parsed) => parsed,
        Err(err) => {
            if cli::is_help_error(err.as_ref()) {
                process::exit(0);
            }
            eprintln!("error: {}", cli::safe_err(err.as_ref()));
            process::exit(1);
        }
    };

    let size = term::get_size();
    let tmux = term::in_tmux();
    if tmux && let Err(err) = term::enable_tmux_passthrough() {
        eprintln!("error: {}", cli::sanitize_control_chars(&err.to_string()));
        process::exit(1);
    }

    let sources = cli::build_sources(&parsed);
    if sources.is_empty() {
        if !std::io::stdin().is_terminal() {
            let src = Source {
                path: String::new(),
                page: parsed.page,
                font_size_pt: parsed.font_size_pt,
                kind: parsed.kind,
            };
            if let Err(err) = dispatch(&src, size, tmux) {
                eprintln!("error: {}", cli::safe_err(err.as_ref()));
                process::exit(1);
            }
            return;
        }
        cli::print_usage();
        process::exit(1);
    }

    let mut any_failed = false;
    for src in &sources {
        if let Err(err) = dispatch(src, size, tmux) {
            eprintln!("error: {}", cli::safe_err(err.as_ref()));
            any_failed = true;
        }
    }
    if any_failed {
        process::exit(1);
    }
}

fn dispatch(src: &Source, size: term::Size, tmux: bool) -> Result<(), Box<dyn std::error::Error>> {
    let markdown_opts = display::MarkdownOptions {
        page: src.page,
        font_size_pt: src.font_size_pt,
    };

    if !src.path.is_empty() {
        if src.kind == InputKind::Markdown || cli::is_markdown_path(&src.path) {
            return display::markdown::markdown_with_options(&src.path, size, tmux, markdown_opts);
        }
        if src
            .path
            .rsplit('.')
            .next()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
        {
            return display::pdf::pdf(&src.path, src.page, size, tmux);
        }
        if cli::has_image_path_extension(&src.path) {
            return display::image::image(&src.path, size, tmux);
        }
        match display::archive::archive(&src.path, src.page, size, tmux) {
            Ok(()) => return Ok(()),
            Err(err)
                if err
                    .downcast_ref::<display::archive::NotArchiveError>()
                    .is_some() => {}
            Err(err) => return Err(err),
        }
        return display::image::image(&src.path, size, tmux);
    }

    let data = imgutil::read_source("").map_err(|err| format!("failed to read stdin: {err}"))?;
    if src.kind == InputKind::Markdown {
        return display::markdown::markdown_from_bytes_with_options(
            &data,
            size,
            tmux,
            markdown_opts,
        );
    }
    if cli::bytes_has_prefix(&data, b"%PDF-") {
        return display::pdf::pdf_from_bytes(&data, src.page, size, tmux);
    }
    display::image::image_from_bytes(&data, size, tmux)
}

use std::io::IsTerminal;
