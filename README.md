[中文](README_cn.md)

# icat

`icat` displays images and document content directly in terminals that support
the Kitty graphics protocol. It accepts image files, Markdown, PDFs, archives
that contain images, shell globs, and stdin.

## Features

- Display common image formats: PNG, JPEG, GIF, BMP, WebP, and TIFF.
- Render Markdown headings, lists, tables, blockquotes, styled inline text,
  syntax-highlighted code, local images, LaTeX math, and Mermaid diagrams.
- Wrap long Markdown content safely while preserving inline-code whitespace,
  Unicode spacing, link underlines, and italic styling.
- Extract readable PDF text, including CJK text recovered from embedded CMaps,
  or display the largest decodable image from image-based PDFs.
- Pick images from ZIP, TAR, TAR.GZ, 7z, and RAR archives.
- Work inside tmux by using Kitty passthrough when available.

## Requirements

- Rust toolchain with Cargo.
- A terminal with Kitty graphics protocol support for image output.

## Build

```sh
cargo build
```

For an optimized local binary:

```sh
cargo build --release
```

The release binary is written to `target/release/icat`.

## Usage

```sh
cargo run -- image.png
cargo run -- '*.jpg'
cargo run -- document.pdf
cargo run -- README.md
cat README.md | cargo run -- --markdown
cargo run -- -p3 document.pdf
cargo run -- photos.zip
cargo run -- -p 2 photos.zip
cat image.png | cargo run --
```

Installed or release binaries use the same arguments without `cargo run --`:

```sh
icat image.png
icat --markdown notes.md
icat --md-font-size 20 README.md
icat -p 2 archive.zip
```

Run `icat --help` for the full option list.

## Input Selection

For PDFs, `-p N` selects a 1-based page. For archives, `-p N` selects the
1-based image index; without `-p`, `icat` chooses a random image from the
archive. For Markdown, `-p N` selects a rendered page and `--md-font-size N`
sets the base font size in points.

When Markdown spans multiple terminal pages and stdout is interactive, omitting
`-p` opens a simple pager. Press Enter for the next page, enter a page number to
jump, or enter `q` to quit.

Use `-` to read explicit stdin input:

```sh
cat image.png | icat -
cat notes.md | icat --markdown -
```

## Markdown Rendering

Markdown images can appear on their own or inline with text. Relative image
paths are resolved from the Markdown file's directory; for stdin Markdown they
are resolved from the current working directory. Missing inline images do not
remove the surrounding text.

Math uses standard inline and display delimiters supported by Markdown, and a
fenced `mermaid` code block renders as a diagram. The built-in Mermaid renderer
supports flowcharts, sequence diagrams, class diagrams, state diagrams, and ER
diagrams.

## PDF Behavior

Without `-p`, `icat` prefers readable text extracted across the document and
falls back to an embedded image from the first page. With `-p N`, it first tries
the largest embedded image on that page and then falls back to that page's text.
If the largest image is corrupt or unsupported, the next-largest decodable image
is used.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo test --locked
```

Unit tests live beside the modules they cover, and end-to-end CLI tests live in
`tests/cli.rs`.

## License

This project is licensed under the MIT License. See `LICENSE` for details.
