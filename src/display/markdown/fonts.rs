use std::path::{Path, PathBuf};

use cosmic_text::FontSystem;

const DARWIN_USER_FONT_ROOTS: &[&str] = &["~/Library/Fonts"];
const DARWIN_SYS_FONT_ROOTS: &[&str] = &["/Library/Fonts", "/System/Library/Fonts"];
#[cfg(not(target_os = "macos"))]
const LINUX_USER_FONT_ROOTS: &[&str] = &["~/.local/share/fonts"];
#[cfg(not(target_os = "macos"))]
const LINUX_SYS_FONT_ROOTS: &[&str] = &["/usr/share/fonts"];
const DARWIN_FALLBACK_FONT: &str = "/Library/Fonts/Arial Unicode.ttf";

// The two preferred CJK families (both platforms, same order as Go)
const PREFERRED_FAMILIES: &[&str] = &["pingfang", "notosanscjk"];

// Test string for Chinese glyph coverage check
const CJK_TEST_CHARS: &str = "中国银行卡号金额";

pub struct FontResolution {
    pub font_system: FontSystem,
    pub warning: Option<String>,
}

/// Returns font search root directories in priority order (user before system).
pub fn font_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let user = &DARWIN_USER_FONT_ROOTS;
        let sys = &DARWIN_SYS_FONT_ROOTS;
        for &r in user.iter().chain(sys.iter()) {
            roots.push(expand_tilde(r));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let user = &LINUX_USER_FONT_ROOTS;
        let sys = &LINUX_SYS_FONT_ROOTS;
        for &r in user.iter().chain(sys.iter()) {
            roots.push(expand_tilde(r));
        }
    }
    dedup_paths(roots)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Returns 0-based rank for preferred families (lower = higher priority), usize::MAX if not preferred.
pub fn family_rank(filename: &str) -> usize {
    let norm = normalize_font_name(filename);
    for (i, family) in PREFERRED_FAMILIES.iter().enumerate() {
        if norm.contains(family) {
            return i;
        }
    }
    usize::MAX
}

/// Normalize: keep only ASCII alphanumeric, lowercase.
pub fn normalize_font_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn is_font_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("ttf" | "ttc" | "otf")
    )
}

/// Discover font candidates sorted by preference.
/// Returns (candidates, permission_denied).
pub fn discover_candidates() -> (Vec<PathBuf>, bool) {
    let roots = font_search_roots();
    let mut permission_denied = false;

    struct Candidate {
        path: PathBuf,
        root_index: usize,
        family_rank: usize,
        is_ttc: bool,
        has_regular: bool,
        filename_lower: String,
    }

    let mut candidates: Vec<Candidate> = Vec::new();

    for (root_index, root) in roots.iter().enumerate() {
        let walker = match std::fs::read_dir(root) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                permission_denied = true;
                continue;
            }
            Err(_) => continue,
        };
        // Walk recursively
        let mut stack = vec![walker];
        while let Some(dir) = stack.last_mut() {
            let entry = match dir.next() {
                Some(Ok(e)) => e,
                Some(Err(e)) => {
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        permission_denied = true;
                    }
                    continue;
                }
                None => {
                    stack.pop();
                    continue;
                }
            };
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                match std::fs::read_dir(&path) {
                    Ok(d) => stack.push(d),
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        permission_denied = true;
                    }
                    Err(_) => {}
                }
                continue;
            }
            if !is_font_file(&path) {
                continue;
            }
            let fname = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let fname_lower = fname.to_ascii_lowercase();
            let rank = family_rank(&fname_lower);
            if rank == usize::MAX {
                continue;
            } // not a preferred family
            candidates.push(Candidate {
                root_index,
                family_rank: rank,
                is_ttc: fname_lower.ends_with(".ttc"),
                has_regular: fname_lower.contains("regular"),
                filename_lower: fname_lower,
                path,
            });
        }
    }

    // Sort: root_index ASC, family_rank ASC, non-ttc before ttc, "regular" first, filename ASC, path ASC
    candidates.sort_by(|a, b| {
        a.root_index
            .cmp(&b.root_index)
            .then(a.family_rank.cmp(&b.family_rank))
            .then(a.is_ttc.cmp(&b.is_ttc)) // false < true, so non-ttc first
            .then(b.has_regular.cmp(&a.has_regular)) // true > false, so regular first
            .then(a.filename_lower.cmp(&b.filename_lower))
            .then(a.path.cmp(&b.path))
    });

    (
        candidates.into_iter().map(|c| c.path).collect(),
        permission_denied,
    )
}

/// Test whether a FontSystem can render the CJK test string.
fn font_system_can_render_cjk(fs: &mut FontSystem) -> bool {
    use cosmic_text::{Attrs, Buffer, Metrics, Shaping};
    let mut buf = Buffer::new(fs, Metrics::new(16.0, 20.0));
    buf.set_text(CJK_TEST_CHARS, &Attrs::new(), Shaping::Advanced, None);
    buf.shape_until_scroll(fs, false);
    let mut saw_glyph = false;
    for run in buf.layout_runs() {
        for glyph in run.glyphs {
            saw_glyph = true;
            if glyph.glyph_id == 0 {
                return false;
            }
        }
    }
    saw_glyph
}

/// Resolve fonts for Markdown rendering, returning a ready FontSystem and optional warning.
pub fn resolve_fonts() -> FontResolution {
    let mut fs = FontSystem::new();
    let warning = if font_system_can_render_cjk(&mut fs) {
        None
    } else {
        let (candidates, permission_denied) = discover_candidates();
        let mut last_attempt = None;
        for path in &candidates {
            last_attempt = Some((
                path.clone(),
                if fs.db_mut().load_font_file(path).is_ok() {
                    "font does not cover CJK characters".to_string()
                } else {
                    "no faces loaded".to_string()
                },
            ));
            if font_system_can_render_cjk(&mut fs) {
                return FontResolution {
                    font_system: fs,
                    warning: None,
                };
            }
        }
        #[cfg(target_os = "macos")]
        let fallback_arg = {
            let fallback = PathBuf::from(DARWIN_FALLBACK_FONT);
            if fallback.exists() {
                fs.db_mut().load_font_file(&fallback).ok();
                Some(fallback)
            } else {
                None
            }
        };
        #[cfg(not(target_os = "macos"))]
        let fallback_arg: Option<PathBuf> = None;
        Some(missing_font_warning(
            permission_denied,
            last_attempt.as_ref(),
            fallback_arg.as_ref(),
            cfg!(target_os = "macos"),
        ))
    };
    FontResolution {
        font_system: fs,
        warning,
    }
}

/// Build the warning message matching Go's text exactly.
pub fn missing_font_warning(
    permission_denied: bool,
    attempt: Option<&(PathBuf, String)>,
    fallback: Option<&PathBuf>,
    is_darwin: bool,
) -> String {
    let mut msg = if permission_denied {
        "warning: font discovery hit permission errors, so icat could not fully search for a suitable Chinese font".to_string()
    } else {
        "warning: could not find a suitable Chinese font for Markdown rendering".to_string()
    };

    if let Some((path, err)) = attempt {
        let path_str = path.display().to_string();
        let err_display = if path_str.to_ascii_lowercase().ends_with(".ttc")
            && err.contains("bad ttf version")
        {
            "found a matching TTC font but the current pure-Go renderer cannot load this collection format".to_string()
        } else {
            err.clone()
        };
        msg.push_str(&format!(
            "; found matching font {}, but it is incompatible: {}",
            path_str, err_display
        ));
    }

    match fallback {
        Some(p) => msg.push_str(&format!("; using {}", p.display())),
        None if is_darwin => msg.push_str("; using the renderer default font"),
        None => msg.push_str("; using the renderer default font. Install NotoSansCJK in ~/.local/share/fonts or a system font directory"),
    }

    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_search_roots_prioritize_user_fonts() {
        let roots = font_search_roots();
        assert!(
            !roots.is_empty(),
            "font_search_roots should return at least one root"
        );
        let first = roots[0].to_string_lossy();
        #[cfg(target_os = "macos")]
        assert!(
            first.contains("Library/Fonts"),
            "first root should contain Library/Fonts on macOS, got: {first}"
        );
        #[cfg(not(target_os = "macos"))]
        assert!(
            first.contains(".local/share/fonts"),
            "first root should contain .local/share/fonts on Linux, got: {first}"
        );
    }

    #[test]
    fn test_family_rank_normalizes_font_names() {
        assert!(
            family_rank("PingFang SC") < usize::MAX,
            "PingFang SC should be a preferred family"
        );
        assert!(
            family_rank("NotoSansCJK-Regular.ttf") < usize::MAX,
            "NotoSansCJK-Regular.ttf should be a preferred family"
        );
        assert_eq!(
            family_rank("Arial.ttf"),
            usize::MAX,
            "Arial.ttf should not be a preferred family"
        );
    }

    #[test]
    fn test_family_rank_prefers_pingfang_over_noto() {
        assert!(
            family_rank("PingFang.ttf") < family_rank("NotoSansCJK.ttf"),
            "PingFang should have lower (higher-priority) rank than NotoSansCJK"
        );
    }

    #[test]
    fn test_discover_candidates_ordering() {
        // Test the sort ordering indirectly by verifying rank values
        assert!(
            family_rank("pingfangsc.ttf") < family_rank("notosanscjksc-regular.otf"),
            "pingfang should rank before notosanscjk"
        );
    }

    #[test]
    fn test_missing_font_warning_permission() {
        let msg = missing_font_warning(true, None, None, false);
        assert!(
            msg.contains("permission errors"),
            "warning should mention permission errors, got: {msg}"
        );
    }

    #[test]
    fn test_missing_font_warning_incompatible() {
        let attempt = (
            PathBuf::from("/fonts/NotoSansCJK.ttf"),
            "some error".to_string(),
        );
        let msg = missing_font_warning(false, Some(&attempt), None, false);
        assert!(
            msg.contains("found matching font"),
            "warning should mention found matching font, got: {msg}"
        );
        assert!(
            msg.contains("incompatible"),
            "warning should mention incompatible, got: {msg}"
        );
    }
}
