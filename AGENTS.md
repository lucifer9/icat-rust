# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2024 binary crate named `icat`. The CLI entry point is
`src/main.rs`, with reusable modules exported from `src/lib.rs`.

- `src/cli.rs`: argument parsing, input-kind detection, and terminal-safe errors.
- `src/display/`: format dispatch and rendering for images, archives, Markdown,
  PDFs, and related helpers.
- `src/display/archive/`: archive-specific readers for ZIP, TAR, 7z, and RAR.
- `src/display/markdown/`: Markdown rendering, font handling, math, and Mermaid
  support.
- `src/display/markdown/markie/`: SVG-backed math layout and native Mermaid
  parsing, layout, and rendering.
- `src/display/pdf/`: PDF loading, text extraction and CMap decoding, plus
  embedded-image fallback.
- `src/kitty/`: Kitty graphics protocol output.
- `src/imgutil.rs` and `src/term.rs`: image I/O and terminal integration.
- `tests/cli.rs`: end-to-end tests for the compiled `icat` binary.

Keep local sample files, PDFs, archives, and generated output out of commits
unless they are deliberate fixtures.

## Build, Test, and Development Commands

- `cargo build`: compile the crate.
- `cargo run -- --help`: run the local binary and print usage.
- `cargo test`: run unit tests and integration tests.
- `cargo test display::markdown`: run Markdown parser and renderer tests.
- `cargo test display::pdf`: run PDF extraction and image fallback tests.
- `cargo test --locked`: verify the checked-in `Cargo.lock` is sufficient for a
  reproducible test build.
- `cargo fmt --check`: verify Rust formatting before review.
- `cargo clippy --all-targets --all-features -- -D warnings`: run stricter lint
  checks across tests and feature combinations.

Use `cargo run -- <path>` for manual checks against images, archives, Markdown,
or PDF input.

## Coding Style & Naming Conventions

Use standard `rustfmt` formatting and Rust naming conventions: `snake_case` for
functions, modules, and variables; `PascalCase` for types and traits;
`SCREAMING_SNAKE_CASE` for constants. Prefer small, direct helpers in the module
that owns the behavior. Put high-level public behavior before private details.
Comments should explain non-obvious reasons, not restate code.

## Rendering Invariants

Markdown block and inline parser loops must always consume input or advance the
event index; malformed or nested block content must not hang rendering. Preserve
the distinction between normal prose whitespace and whitespace inside inline
code, and keep long unbreakable text within the output canvas. Relative image
paths are resolved against the Markdown source directory, and images must work
both as standalone blocks and inline with surrounding text.

Font discovery and glyph/SVG font databases are intentionally cached because
loading system fonts is expensive. Reuse the existing shared caches instead of
creating a new font database for each paragraph, formula, or Mermaid diagram.

For PDFs, a font-specific `ToUnicode` CMap takes precedence over the raw scanned
CMap fallback. Keep the fallback separate rather than inserting it as a fake
font entry. Embedded images are tried largest-first, but extraction must continue
to smaller images when a larger candidate cannot be decoded.

## Testing Guidelines

Unit tests live beside the module under `#[cfg(test)] mod tests`; binary-level
behavior belongs in `tests/cli.rs`. Test outcomes visible to users: exit status,
rendered protocol prefixes, extracted PDF text, archive selection behavior, and
error handling. Markdown regressions should assert rendered pixels or layout
outcomes for inline images, styling, whitespace, wrapping, math, Mermaid, and
nested blocks rather than only inspecting parser tokens. PDF tests should cover
font-specific and raw CMap decoding plus corrupt-image fallback. Prefer fixtures
built in memory or temporary directories via `tempfile` over committed binary
blobs. Name tests by behavior, for example
`stdin_markdown_produces_kitty_output`.

## Commit & Pull Request Guidelines

The current history is minimal, so use Conventional Commits for new work:
`type(scope): description`, imperative mood, under 72 characters, for example
`fix(markdown): clip rendering to visible page`. Keep commits focused on one
logical change.

Pull requests should describe the user-visible change, list verification
commands run, call out protocol or file-format risks, and link related issues
when available. Include screenshots or terminal captures only when visual output
changed.
