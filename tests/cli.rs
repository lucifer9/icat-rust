//! End-to-end integration tests that exercise the compiled `icat` binary.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use tempfile::tempdir;

fn icat_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_icat"))
}

/// Encode a 4x4 white RGBA PNG into memory using the `image` crate.
fn small_png() -> Vec<u8> {
    use image::{DynamicImage, ImageFormat};
    let img = DynamicImage::new_rgba8(4, 4);
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png).unwrap();
    buf.into_inner()
}

/// Create a zip archive (in memory) containing one PNG entry.
fn zip_with_png(png: &[u8]) -> Vec<u8> {
    use zip::ZipWriter;
    use zip::write::{ExtendedFileOptions, FileOptions};
    let buf = std::io::Cursor::new(Vec::new());
    let mut zw = ZipWriter::new(buf);
    let opts: FileOptions<ExtendedFileOptions> = FileOptions::default();
    zw.start_file("image.png", opts).unwrap();
    zw.write_all(png).unwrap();
    zw.finish().unwrap().into_inner()
}

fn minimal_text_pdf() -> Vec<u8> {
    let content = "BT\n/F1 12 Tf\n1 0 0 1 72 720 Tm\n(Hello from PDF) Tj\nET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), content),
    ];
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = vec![0_usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref_offset = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
            objects.len() + 1,
            xref_offset
        )
        .as_bytes(),
    );
    out
}

/// Run `icat` with the given args and env, piping `stdin_data` on stdin.
/// Returns (stdout bytes, exit status success).
fn run_icat(
    args: &[&str],
    stdin_data: Option<&[u8]>,
    extra_env: &[(&str, &str)],
) -> (Vec<u8>, bool) {
    let mut cmd = Command::new(icat_bin());
    cmd.args(args);
    // Provide terminal size and strip TMUX so tests run in non-tmux mode by default
    cmd.env("COLUMNS", "80")
        .env("LINES", "24")
        .env_remove("TMUX");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    if stdin_data.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    let mut child = cmd.spawn().expect("failed to spawn icat");
    if let Some(data) = stdin_data {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(data).unwrap();
        drop(stdin);
    }
    let output = child.wait_with_output().unwrap();
    (output.stdout, output.status.success())
}

// ── Helper assertions ─────────────────────────────────────────────────────────

fn assert_kitty_output(stdout: &[u8], tmux: bool) {
    assert!(!stdout.is_empty(), "stdout should not be empty");
    if tmux {
        // tmux passthrough prefix: \r\x1bPtmux;\x1b\x1b_G...
        let tmux_prefix: &[u8] = b"\r\x1bPtmux;\x1b\x1b_G";
        let alt_prefix: &[u8] = b"\x1bPtmux;\x1b\x1b_G";
        assert!(
            stdout.starts_with(tmux_prefix) || stdout.starts_with(alt_prefix),
            "expected tmux header prefix, got: {:?}",
            &stdout[..stdout.len().min(40)]
        );
    } else {
        // raw Kitty prefix: \x1b_G
        assert!(
            stdout.starts_with(b"\x1b_G"),
            "expected \\x1b_G prefix, got: {:?}",
            &stdout[..stdout.len().min(40)]
        );
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn stdin_image_produces_kitty_output() {
    let png = small_png();
    let (stdout, ok) = run_icat(&[], Some(&png), &[]);
    assert!(ok, "icat should exit 0 for valid stdin image");
    assert_kitty_output(&stdout, false);
}

#[test]
fn stdin_markdown_produces_kitty_output() {
    let md = b"# Hello\n\nThis is a test paragraph.\n";
    let (stdout, ok) = run_icat(&["--markdown"], Some(md), &[]);
    assert!(ok, "icat should exit 0 for stdin markdown");
    assert_kitty_output(&stdout, false);
}

#[test]
fn file_archive_zip_produces_kitty_output() {
    let dir = tempdir().unwrap();
    let png = small_png();
    let zip_data = zip_with_png(&png);
    let zip_path = dir.path().join("archive.zip");
    std::fs::write(&zip_path, &zip_data).unwrap();

    let path_str = zip_path.to_str().unwrap();
    let (stdout, ok) = run_icat(&[path_str], None, &[]);
    assert!(ok, "icat should exit 0 for zip archive");
    assert_kitty_output(&stdout, false);
}

#[test]
fn file_markdown_page_produces_kitty_output() {
    let dir = tempdir().unwrap();
    let md_path = dir.path().join("README.md");
    std::fs::write(&md_path, b"# Title\n\nContent paragraph.\n").unwrap();

    let path_str = md_path.to_str().unwrap();
    let (stdout, ok) = run_icat(&["-p", "1", path_str], None, &[]);
    assert!(ok, "icat should exit 0 for -p 1 markdown file");
    assert_kitty_output(&stdout, false);
}

#[test]
fn stdin_pdf_prints_extracted_text() {
    let pdf = minimal_text_pdf();
    let (stdout, ok) = run_icat(&[], Some(&pdf), &[]);
    assert!(ok, "icat should exit 0 for valid stdin PDF text");
    assert!(
        String::from_utf8_lossy(&stdout).contains("Hello from PDF"),
        "expected extracted PDF text, got: {:?}",
        String::from_utf8_lossy(&stdout)
    );
}

#[test]
#[cfg(unix)]
fn tmux_mode_produces_tmux_header() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    let dir = tempdir().unwrap();

    // Create a fake `tmux` binary that always exits 0
    let fake_tmux = dir.path().join("tmux");
    std::fs::write(&fake_tmux, b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&fake_tmux, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Create a Unix socket file — in_tmux() checks for read+write access on it
    let socket_path = dir.path().join("tmux.sock");
    let _listener = UnixListener::bind(&socket_path).expect("bind unix socket");

    // Use the current process's PID — in_tmux() checks the process is alive via kill(pid, 0)
    let pid = std::process::id();
    let tmux_env = format!("{},{pid},0", socket_path.display());

    let original_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{original_path}", dir.path().display());

    let png = small_png();
    let (stdout, ok) = run_icat(&[], Some(&png), &[("TMUX", &tmux_env), ("PATH", &new_path)]);
    assert!(
        ok,
        "icat should exit 0 in tmux mode with working tmux binary"
    );
    assert_kitty_output(&stdout, true);
}
