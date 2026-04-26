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
- `src/kitty/`: Kitty graphics protocol output.
- `src/imgutil.rs` and `src/term.rs`: image I/O and terminal integration.
- `tests/cli.rs`: end-to-end tests for the compiled `icat` binary.

Keep local sample files, PDFs, archives, and generated output out of commits
unless they are deliberate fixtures.

## Build, Test, and Development Commands

- `cargo build`: compile the crate.
- `cargo run -- --help`: run the local binary and print usage.
- `cargo test`: run unit tests and integration tests.
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

## Testing Guidelines

Unit tests live beside the module under `#[cfg(test)] mod tests`; binary-level
behavior belongs in `tests/cli.rs`. Test outcomes visible to users: exit status,
rendered protocol prefixes, extracted PDF text, archive selection behavior, and
error handling. Prefer fixtures built in memory or temporary directories via
`tempfile` over committed binary blobs. Name tests by behavior, for example
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
