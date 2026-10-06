use cosmic_text::FontSystem;

#[cfg(target_os = "macos")]
const MISSING_CJK_FONT_WARNING: &str = "warning: could not find a suitable Chinese font for Markdown rendering; using the renderer default font";
#[cfg(not(target_os = "macos"))]
const MISSING_CJK_FONT_WARNING: &str = "warning: could not find a suitable Chinese font for Markdown rendering; using the renderer default font. Install NotoSansCJK in ~/.local/share/fonts or a system font directory";

// Preferred Simplified Chinese families. Put the region-specific names before
// their broader family prefixes so PingFang SC / Noto Sans CJK SC win over
// Hong Kong, Traditional Chinese, Japanese, or Korean variants.
const PREFERRED_FAMILIES: &[&str] = &["pingfangsc", "pingfang", "notosanscjksc", "notosanscjk"];

// Test string for Chinese glyph coverage check
const CJK_TEST_CHARS: &str = "中国银行卡号金额";

/// Whether `ch` belongs to a CJK block (ideographs, kana, Hangul, CJK and
/// full-width punctuation) that should render with the preferred CJK family.
pub fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{2E80}'..='\u{2FDF}'
            | '\u{3000}'..='\u{33FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{AC00}'..='\u{D7AF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FE30}'..='\u{FE4F}'
            | '\u{FF00}'..='\u{FFEF}'
            | '\u{20000}'..='\u{3FFFF}'
    )
}

/// Splits `text` into maximal runs of CJK and non-CJK characters.
pub fn cjk_runs(text: &str) -> impl Iterator<Item = (&str, bool)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let first = rest.chars().next()?;
        let cjk = is_cjk_char(first);
        let end = rest
            .char_indices()
            .find(|&(_, ch)| is_cjk_char(ch) != cjk)
            .map_or(rest.len(), |(idx, _)| idx);
        let (run, tail) = rest.split_at(end);
        rest = tail;
        Some((run, cjk))
    })
}

pub struct FontResolution {
    pub font_system: FontSystem,
    pub warning: Option<String>,
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

fn preferred_cjk_sans_family(fs: &FontSystem) -> Option<String> {
    fs.db()
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name))
        .filter_map(|name| {
            let rank = family_rank(name);
            (rank != usize::MAX).then_some((rank, name))
        })
        .min_by(|(rank_a, name_a), (rank_b, name_b)| {
            rank_a.cmp(rank_b).then_with(|| name_a.cmp(name_b))
        })
        .map(|(_, name)| name.clone())
}

fn configure_preferred_cjk_sans_family(fs: &mut FontSystem) -> Option<String> {
    let family = preferred_cjk_sans_family(fs)?;
    fs.db_mut().set_sans_serif_family(family.clone());
    Some(family)
}

/// Resolve fonts for Markdown rendering, returning a ready FontSystem and optional warning.
pub fn resolve_fonts() -> FontResolution {
    let mut fs = without_zero_width_monospace_faces(FontSystem::new());
    configure_preferred_cjk_sans_family(&mut fs);
    let warning =
        (!font_system_can_render_cjk(&mut fs)).then(|| MISSING_CJK_FONT_WARNING.to_string());
    FontResolution {
        font_system: fs,
        warning,
    }
}

// cosmic-text sizes monospace fallback glyphs by the face's space advance per
// em. It skips faces without a space glyph but not faces whose space advance is
// zero (macOS GB18030Bitmap), which yields an infinite glyph size: every CJK
// character in a code block becomes its own NaN-height line.
fn without_zero_width_monospace_faces(fs: FontSystem) -> FontSystem {
    use cosmic_text::skrifa::instance::{LocationRef, Size};
    use cosmic_text::skrifa::{FontRef, MetadataProvider};

    let broken: Vec<_> = fs
        .db()
        .faces()
        .filter(|face| face.monospaced)
        .filter(|face| {
            fs.db()
                .with_face_data(face.id, |data, index| {
                    let font = FontRef::from_index(data, index).ok()?;
                    let space = font.charmap().map(' ')?;
                    font.glyph_metrics(Size::unscaled(), LocationRef::default())
                        .advance_width(space)
                })
                .flatten()
                == Some(0.0)
        })
        .map(|face| face.id)
        .collect();
    if broken.is_empty() {
        return fs;
    }
    // FontSystem caches its monospace face list at construction, so rebuild it.
    let (locale, mut db) = fs.into_locale_and_db();
    for id in broken {
        db.remove_face(id);
    }
    FontSystem::new_with_locale_and_db(locale, db)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_preferred_cjk_family_keeps_bold_heading_in_one_family() {
        use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Weight};

        let mut fs = FontSystem::new();
        let Some(family) = configure_preferred_cjk_sans_family(&mut fs) else {
            return;
        };
        assert_eq!(fs.db().family_name(&Family::SansSerif), family);

        let mut buffer = Buffer::new(&mut fs, Metrics::new(32.0, 44.0));
        buffer.set_text(
            "环境要求构建",
            &Attrs::new().family(Family::SansSerif).weight(Weight::BOLD),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut fs, false);

        let mut glyph_count = 0;
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                glyph_count += 1;
                let face = fs.db().face(glyph.font_id).unwrap();
                assert!(
                    face.families.iter().any(|(name, _)| name == &family),
                    "glyph {}..{} used {:?}, expected {family}",
                    glyph.start,
                    glyph.end,
                    face.families
                );
            }
        }
        assert!(glyph_count > 0);
    }
}
