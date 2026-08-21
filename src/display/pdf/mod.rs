use std::collections::HashMap;
use std::sync::OnceLock;

use ::image::{DynamicImage, GrayImage, RgbImage};
use lopdf::content::{Content, Operation};
use lopdf::{Document, LoadOptions, Object};
use regex::Regex;

use crate::display::image;
use crate::imgutil;
use crate::term::Size;

const MIN_PDF_TEXT_CHARS: usize = 50;
// PDF viewers conventionally assume 12pt when content omits Tf.
const DEFAULT_PDF_FONT_SIZE: f64 = 12.0;
const MAX_PDF_TEXT_BYTES: usize = 8 * 1024 * 1024;

#[cfg(not(test))]
const MAX_PDF_DECOMPRESSED_STREAM_BYTES: usize = 64 * 1024 * 1024;
#[cfg(test)]
const MAX_PDF_DECOMPRESSED_STREAM_BYTES: usize = 128; // small limit for testing

#[cfg(not(test))]
const MAX_PDF_CMAP_BYTES: usize = 8 * 1024 * 1024;
#[cfg(test)]
const MAX_PDF_CMAP_BYTES: usize = 512;

static RE_RANGE_SECTION: OnceLock<Regex> = OnceLock::new();
static RE_RANGE_ENTRY: OnceLock<Regex> = OnceLock::new();
static RE_CHAR_SECTION: OnceLock<Regex> = OnceLock::new();
static RE_CHAR_ENTRY: OnceLock<Regex> = OnceLock::new();
static RE_WHITESPACE: OnceLock<Regex> = OnceLock::new();

type CidToUnicode = HashMap<u16, char>;

#[derive(Debug, Default)]
struct PdfResult {
    warning: Option<String>,
    text: String,
    image_data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct PdfTextRun {
    text: String,
    x: f64,
    y: f64,
    font_size: f64,
    width: f64,
    order: usize,
}

#[derive(Debug, Clone)]
struct PdfTextLine {
    runs: Vec<PdfTextRun>,
    y: f64,
    font_size: f64,
}

pub fn pdf(
    path: &str,
    page: Option<usize>,
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let label = if path.is_empty() { "<stdin>" } else { path };
    let data =
        imgutil::read_source(path).map_err(|err| format!("failed to read PDF {label}: {err}"))?;
    run_pdf_strategies(&data, label, page.unwrap_or(0), size, tmux)
}

pub fn pdf_from_bytes(
    data: &[u8],
    page: Option<usize>,
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    run_pdf_strategies(data, "<stdin>", page.unwrap_or(0), size, tmux)
}

fn run_pdf_strategies(
    data: &[u8],
    label: &str,
    page: usize,
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = prepare_pdf(data, label, page)?;
    if !result.text.is_empty() {
        if let Some(warning) = result.warning {
            eprintln!("{warning}");
        }
        print!("{}", sanitize_text(&result.text));
        return Ok(());
    }
    if let Some(warning) = result.warning {
        eprintln!("{warning}");
    }
    image::image_from_bytes(&result.image_data, size, tmux)
}

/// Try to load a PDF, falling back to scanning for valid `%%EOF` boundaries
/// when the standard loader fails (e.g. a malformed incremental-update /Prev pointer).
fn load_pdf_lenient(data: &[u8]) -> Result<Document, lopdf::Error> {
    if let Ok(doc) = load_pdf_mem_bounded(data) {
        return Ok(doc);
    }
    // Collect all %%EOF positions (last-to-first so we try the most-complete version first)
    let marker = b"%%EOF";
    let mut positions: Vec<usize> = data
        .windows(marker.len())
        .enumerate()
        .filter(|(_, w)| *w == marker)
        .map(|(i, _)| i + marker.len())
        .collect();
    positions.reverse();
    for end in positions {
        if let Ok(doc) = load_pdf_mem_bounded(&data[..end]) {
            return Ok(doc);
        }
    }
    // Last resort: full data again (will fail with the original error)
    load_pdf_mem_bounded(data)
}

fn load_pdf_mem_bounded(data: &[u8]) -> Result<Document, lopdf::Error> {
    Document::load_mem_with_options(
        data,
        LoadOptions::with_max_decompressed_size(MAX_PDF_DECOMPRESSED_STREAM_BYTES),
    )
}

fn prepare_pdf(
    data: &[u8],
    label: &str,
    page: usize,
) -> Result<PdfResult, Box<dyn std::error::Error>> {
    let document = load_pdf_lenient(data)?;
    let pages = document.get_pages();
    let total_pages = pages.len();
    let (effective_page, warning) = clamp_pdf_page(page, total_pages, label);

    if effective_page > 0 {
        if let Ok(image_data) = extract_largest_image(&document, effective_page as u32) {
            return Ok(PdfResult {
                warning,
                text: String::new(),
                image_data,
            });
        }
        let text = extract_page_text(&document, effective_page as u32, &scan_cmaps_from_raw(data))?;
        let trimmed = text.trim().to_string();
        if trimmed.len() >= MIN_PDF_TEXT_CHARS || !trimmed.is_empty() {
            return Ok(PdfResult {
                warning,
                text: trimmed,
                image_data: Vec::new(),
            });
        }
        return Err(format!("failed to extract content from PDF {label}").into());
    }

    let text = extract_text_all_pages(&document, &scan_cmaps_from_raw(data))?;
    if text.trim().len() >= MIN_PDF_TEXT_CHARS {
        return Ok(PdfResult {
            warning,
            text,
            image_data: Vec::new(),
        });
    }

    if let Ok(image_data) = extract_largest_image(&document, 1) {
        return Ok(PdfResult {
            warning,
            text: String::new(),
            image_data,
        });
    }

    let trimmed = text.trim().to_string();
    if !trimmed.is_empty() {
        return Ok(PdfResult {
            warning,
            text: trimmed,
            image_data: Vec::new(),
        });
    }

    Err(format!("no extractable content in PDF {label}").into())
}

fn clamp_pdf_page(page: usize, total: usize, label: &str) -> (usize, Option<String>) {
    if page == 0 || total == 0 {
        return (page, None);
    }
    if page <= total {
        return (page, None);
    }
    (
        total,
        Some(format!(
            "warning: page {page} out of range for PDF {label}, showing last page {total}"
        )),
    )
}

fn extract_text_all_pages(
    document: &Document,
    raw_fallback: &CidToUnicode,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut out = String::new();
    for page_number in document.get_pages().keys().copied() {
        let page_text = extract_page_text(document, page_number, raw_fallback)?;
        let page_text = page_text.trim();
        if page_text.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(page_text);
        if out.len() > MAX_PDF_TEXT_BYTES {
            return Err(format!(
                "extracted PDF text exceeds {} MiB limit",
                MAX_PDF_TEXT_BYTES / (1024 * 1024)
            )
            .into());
        }
    }
    Ok(out)
}

fn extract_page_text(
    document: &Document,
    page_number: u32,
    raw_fallback: &CidToUnicode,
) -> Result<String, Box<dyn std::error::Error>> {
    let pages = document.get_pages();
    let page_id = *pages
        .get(&page_number)
        .ok_or_else(|| format!("page {page_number} not found"))?;
    let content = document
        .get_page_content_with_limit(page_id, MAX_PDF_DECOMPRESSED_STREAM_BYTES)
        .map_err(|err| {
            format!(
                "failed to read page content within {} byte limit: {err}",
                MAX_PDF_DECOMPRESSED_STREAM_BYTES
            )
        })?;
    if content.len() > MAX_PDF_DECOMPRESSED_STREAM_BYTES {
        return Err(format!(
            "page content exceeds {} MiB limit",
            MAX_PDF_DECOMPRESSED_STREAM_BYTES / (1024 * 1024)
        )
        .into());
    }
    let operations = Content::decode(&content)?.operations;
    let font_cmaps = collect_font_cmaps(document, page_id)?;
    Ok(extract_text_from_operations(
        &operations,
        &font_cmaps,
        raw_fallback,
    ))
}

fn collect_font_cmaps(
    document: &Document,
    page_id: lopdf::ObjectId,
) -> Result<HashMap<Vec<u8>, CidToUnicode>, Box<dyn std::error::Error>> {
    let mut result = HashMap::new();
    if let Ok(fonts) = document.get_page_fonts(page_id) {
        for (name, font) in fonts {
            if let Ok(to_unicode) = font.get(b"ToUnicode")
                && let Ok(reference) = to_unicode.as_reference()
                && let Ok(object) = document.get_object(reference)
                && let Ok(stream) = object.as_stream()
            {
                let content = match stream.decompressed_content_with_limit(MAX_PDF_CMAP_BYTES) {
                    Ok(content) => content,
                    Err(lopdf::Error::Decompress(
                        lopdf::DecompressError::MemoryLimitExceeded { .. },
                    )) => {
                        return Err(format!(
                            "font ToUnicode CMap exceeds {MAX_PDF_CMAP_BYTES} byte limit"
                        )
                        .into());
                    }
                    Err(_) => continue,
                };
                let cmap = parse_cmap(&String::from_utf8_lossy(&content));
                if !cmap.is_empty() {
                    result.insert(name.clone(), cmap);
                }
            }
        }
    }
    Ok(result)
}

fn extract_largest_image(
    document: &Document,
    page_number: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let page_id = *document
        .get_pages()
        .get(&page_number)
        .ok_or_else(|| format!("page {page_number} not found"))?;
    let mut images = document.get_page_images(page_id)?;
    // Decoding can be expensive (multi-MiB inflate + PNG encode); try images
    // largest-first and stop at the first that decodes.
    images.sort_by_key(|image| std::cmp::Reverse(image.width.saturating_mul(image.height)));
    for image in images {
        let filters = image.filters.clone().unwrap_or_default();
        if let Ok(data) = decode_pdf_image(&filters, image.content, image.origin_dict) {
            return Ok(data);
        }
    }
    Err(String::from("no decodable images found on page").into())
}

fn decode_pdf_image(
    filters: &[String],
    content: &[u8],
    dict: &lopdf::Dictionary,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if filters
        .iter()
        .any(|f| matches!(f.as_str(), "DCTDecode" | "JPXDecode"))
        || ::image::guess_format(content).is_ok()
    {
        // Encoded images are passed through unchanged, but validate them first
        // so a corrupt or unsupported candidate does not block smaller images.
        imgutil::decode_with_limits(content)?;
        return Ok(content.to_vec());
    }
    if filters.iter().any(|f| f == "CCITTFaxDecode") {
        return decode_ccitt_image(filters, content, dict);
    }
    if filters.iter().any(|f| f == "FlateDecode") {
        return decode_flate_raw_image(content, dict);
    }
    Err(String::from("unsupported PDF image filter").into())
}

/// Decode an image whose last (or only) filter is CCITTFaxDecode.
/// If FlateDecode precedes it in the filter list, apply zlib first.
fn decode_ccitt_image(
    filters: &[String],
    content: &[u8],
    dict: &lopdf::Dictionary,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let (width, height) = pdf_image_dimensions(dict)?;

    // Apply FlateDecode if it appears before CCITTFaxDecode
    let ccitt_data: Vec<u8> = if filters
        .iter()
        .position(|f| f == "FlateDecode")
        .map(|fi| {
            filters
                .iter()
                .position(|f| f == "CCITTFaxDecode")
                .map(|ci| fi < ci)
                .unwrap_or(false)
        })
        .unwrap_or(false)
    {
        inflate_zlib_limited(content)?
    } else {
        content.to_vec()
    };

    // Extract CCITTFaxDecode parameters from DecodeParms (may be an array per-filter)
    let ccitt_parms = extract_ccitt_decode_parms(filters, dict);
    let k = ccitt_parms.0; // K < 0 → Group4, K = 0 → Group3-1D, K > 0 → Group3-2D
    let black_is1 = ccitt_parms.1; // true → PhotometricInterpretation = 1 (BlackIsZero)
    let columns = ccitt_parms.2.unwrap_or(width);

    let tiff_bytes = build_ccitt_tiff(width, height, columns, k, black_is1, &ccitt_data);
    let img = ::image::load_from_memory_with_format(&tiff_bytes, ::image::ImageFormat::Tiff)?;
    imgutil::encode_png(&img)
}

/// Extract (K, BlackIs1, Columns) from PDF DecodeParms for the CCITTFaxDecode filter.
fn extract_ccitt_decode_parms(
    filters: &[String],
    dict: &lopdf::Dictionary,
) -> (i64, bool, Option<u32>) {
    let ccitt_idx = filters.iter().position(|f| f == "CCITTFaxDecode");
    let parms_obj = dict.get(b"DecodeParms").ok();
    let ccitt_dict = match parms_obj {
        Some(Object::Dictionary(d)) => Some(d),
        Some(Object::Array(arr)) => ccitt_idx
            .and_then(|i| arr.get(i))
            .and_then(|o| o.as_dict().ok()),
        _ => None,
    };
    let k = ccitt_dict
        .and_then(|d| d.get(b"K").ok())
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0);
    let black_is1 = ccitt_dict
        .and_then(|d| d.get(b"BlackIs1").ok())
        .and_then(|o| o.as_bool().ok())
        .unwrap_or(false);
    let columns = ccitt_dict
        .and_then(|d| d.get(b"Columns").ok())
        .and_then(|o| o.as_i64().ok())
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0);
    (k, black_is1, columns)
}

/// Build a minimal TIFF file in memory wrapping raw CCITT bitstream data.
fn build_ccitt_tiff(
    _width: u32,
    height: u32,
    columns: u32,
    k: i64,
    black_is1: bool,
    data: &[u8],
) -> Vec<u8> {
    // Compression: K < 0 → Fax4 (4), else Fax3 (3)
    let compression: u16 = if k < 0 { 4 } else { 3 };
    // PhotometricInterpretation: 0 = WhiteIsZero (CCITT default), 1 = BlackIsZero
    let photometric: u16 = if black_is1 { 1 } else { 0 };

    let n_entries: u16 = if k < 0 { 9 } else { 10 }; // Fax4 has no T4Options entry
    let ifd_size: u32 = 2 + n_entries as u32 * 12 + 4;
    let data_offset: u32 = 8 + ifd_size;

    let mut b: Vec<u8> = Vec::with_capacity(data_offset as usize + data.len());

    // Header (little-endian)
    b.extend_from_slice(b"II");
    b.extend_from_slice(&42u16.to_le_bytes());
    b.extend_from_slice(&8u32.to_le_bytes()); // IFD at offset 8

    // IFD entry count
    b.extend_from_slice(&n_entries.to_le_bytes());

    // Inline helpers (using macros to avoid borrow-conflict with closures)
    macro_rules! short_entry {
        ($tag:expr, $val:expr) => {
            b.extend_from_slice(&($tag as u16).to_le_bytes());
            b.extend_from_slice(&3u16.to_le_bytes()); // type SHORT
            b.extend_from_slice(&1u32.to_le_bytes()); // count 1
            b.extend_from_slice(&($val as u32).to_le_bytes()); // value
        };
    }
    macro_rules! long_entry {
        ($tag:expr, $val:expr) => {
            b.extend_from_slice(&($tag as u16).to_le_bytes());
            b.extend_from_slice(&4u16.to_le_bytes()); // type LONG
            b.extend_from_slice(&1u32.to_le_bytes()); // count 1
            b.extend_from_slice(&($val as u32).to_le_bytes());
        };
    }

    short_entry!(0x0100u16, columns); // ImageWidth
    short_entry!(0x0101u16, height); // ImageLength
    short_entry!(0x0102u16, 1u32); // BitsPerSample = 1
    short_entry!(0x0103u16, compression); // Compression
    short_entry!(0x0106u16, photometric); // PhotometricInterpretation
    long_entry!(0x0111u16, data_offset); // StripOffsets
    short_entry!(0x0115u16, 1u32); // SamplesPerPixel = 1
    long_entry!(0x0116u16, height); // RowsPerStrip
    long_entry!(0x0117u16, data.len() as u32); // StripByteCounts
    if k >= 0 {
        let t4opts: u32 = if k > 0 { 1 } else { 0 }; // bit 0: 0=1D, 1=2D
        long_entry!(0x0124u16, t4opts); // T4Options
    }

    b.extend_from_slice(&0u32.to_le_bytes()); // next IFD offset = 0 (end)
    b.extend_from_slice(data);
    b
}

/// Decode a FlateDecode-only raw-pixel image stream.
fn decode_flate_raw_image(
    content: &[u8],
    dict: &lopdf::Dictionary,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let (width, height) = pdf_image_dimensions(dict)?;
    let color_space = dict.get(b"ColorSpace").ok();
    let bits_per_component = dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(8);
    if bits_per_component != 8 {
        return Err(String::from("unsupported PDF image bit depth").into());
    }
    let components = match color_space {
        Some(Object::Name(name)) if name == b"DeviceGray" => 1,
        Some(Object::Name(name)) if name == b"DeviceRGB" => 3,
        Some(Object::Array(array))
            if array.first().and_then(|o| o.as_name().ok()) == Some(b"DeviceRGB") =>
        {
            3
        }
        Some(Object::Array(array))
            if array.first().and_then(|o| o.as_name().ok()) == Some(b"DeviceGray") =>
        {
            1
        }
        _ => 3,
    };
    let expected = pdf_image_buffer_len(width, height, components)?;
    let content = inflate_zlib_limited(content)?;
    if content.len() < expected {
        return Err(String::from("truncated PDF image data").into());
    }
    let image = if components == 1 {
        let buffer = GrayImage::from_raw(width, height, content[..expected].to_vec())
            .ok_or_else(|| String::from("invalid gray image buffer"))?;
        DynamicImage::ImageLuma8(buffer)
    } else {
        let buffer = RgbImage::from_raw(width, height, content[..expected].to_vec())
            .ok_or_else(|| String::from("invalid RGB image buffer"))?;
        DynamicImage::ImageRgb8(buffer)
    };
    imgutil::encode_png(&image)
}

fn inflate_zlib_limited(content: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    let mut decoder = ZlibDecoder::new(content);
    let mut limited = decoder
        .by_ref()
        .take((MAX_PDF_DECOMPRESSED_STREAM_BYTES + 1) as u64);
    let mut buf = Vec::new();
    limited.read_to_end(&mut buf)?;
    if buf.len() > MAX_PDF_DECOMPRESSED_STREAM_BYTES {
        return Err(format!(
            "PDF image stream exceeds {} MiB limit",
            MAX_PDF_DECOMPRESSED_STREAM_BYTES / (1024 * 1024)
        )
        .into());
    }
    Ok(buf)
}

fn pdf_image_dimensions(
    dict: &lopdf::Dictionary,
) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    let width = dict.get(b"Width")?.as_i64()?;
    let height = dict.get(b"Height")?.as_i64()?;
    if width <= 0 || height <= 0 {
        return Err(format!("invalid PDF image dimensions {width}x{height}").into());
    }
    let width = u32::try_from(width)
        .map_err(|_| format!("invalid PDF image dimensions {width}x{height}"))?;
    let height = u32::try_from(height)
        .map_err(|_| format!("invalid PDF image dimensions {width}x{height}"))?;
    if !imgutil::check_limits(width, height) {
        return Err(format!("PDF image dimensions {width}x{height} exceed limits").into());
    }
    Ok((width, height))
}

fn pdf_image_buffer_len(
    width: u32,
    height: u32,
    components: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    let expected = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(components as u64))
        .ok_or_else(|| String::from("PDF image buffer size overflow"))?;
    usize::try_from(expected).map_err(|_| String::from("PDF image buffer size overflow").into())
}

fn sanitize_text(input: &str) -> String {
    input
        .chars()
        .filter(|ch| matches!(ch, '\n' | '\t' | '\r') || (!ch.is_control() && *ch != '\u{7f}'))
        .collect()
}

fn scan_cmaps_from_raw(data: &[u8]) -> CidToUnicode {
    if data.len() > MAX_PDF_TEXT_BYTES {
        return CidToUnicode::new();
    }
    parse_cmap(&String::from_utf8_lossy(data))
}

fn parse_cmap(data: &str) -> CidToUnicode {
    let range_section =
        RE_RANGE_SECTION.get_or_init(|| Regex::new(r"(?s)beginbfrange\s*(.*?)endbfrange").unwrap());
    let range_entry = RE_RANGE_ENTRY
        .get_or_init(|| Regex::new(r"<([0-9A-Fa-f]+)><([0-9A-Fa-f]+)><([0-9A-Fa-f]+)>").unwrap());
    let char_section =
        RE_CHAR_SECTION.get_or_init(|| Regex::new(r"(?s)beginbfchar\s*(.*?)endbfchar").unwrap());
    let char_entry =
        RE_CHAR_ENTRY.get_or_init(|| Regex::new(r"<([0-9A-Fa-f]+)>\s*<([0-9A-Fa-f]+)>").unwrap());
    let mut cmap = CidToUnicode::new();

    for section in range_section.captures_iter(data) {
        for entry in range_entry.captures_iter(&section[1]) {
            let lo = u16::from_str_radix(&entry[1], 16).ok();
            let hi = u16::from_str_radix(&entry[2], 16).ok();
            let dst = u32::from_str_radix(&entry[3], 16).ok();
            if let (Some(lo), Some(hi), Some(dst)) = (lo, hi, dst) {
                for cid in lo..=hi {
                    if let Some(ch) = char::from_u32(dst + u32::from(cid - lo)) {
                        cmap.insert(cid, ch);
                    }
                }
            }
        }
    }

    for section in char_section.captures_iter(data) {
        for entry in char_entry.captures_iter(&section[1]) {
            let src = u16::from_str_radix(&entry[1], 16).ok();
            let dst = u32::from_str_radix(&entry[2], 16).ok();
            if let (Some(src), Some(dst)) = (src, dst)
                && let Some(ch) = char::from_u32(dst)
            {
                cmap.insert(src, ch);
            }
        }
    }

    cmap
}

fn extract_text_from_operations(
    operations: &[Operation],
    font_cmaps: &HashMap<Vec<u8>, CidToUnicode>,
    raw_fallback: &CidToUnicode,
) -> String {
    let runs = collect_pdf_text_runs(operations, font_cmaps, raw_fallback);
    render_pdf_text_runs(&runs)
}

fn collect_pdf_text_runs(
    operations: &[Operation],
    font_cmaps: &HashMap<Vec<u8>, CidToUnicode>,
    raw_fallback: &CidToUnicode,
) -> Vec<PdfTextRun> {
    let mut runs = Vec::new();
    let mut current_font = Vec::new();
    let mut font_size = DEFAULT_PDF_FONT_SIZE;
    let mut line_leading = font_size * 1.2;
    let mut cur_x = f64::NAN;
    let mut cur_y = f64::NAN;
    let mut line_start_x = f64::NAN;
    let mut line_start_y = f64::NAN;

    for (order, operation) in operations.iter().enumerate() {
        match operation.operator.as_str() {
            "ET" => {
                cur_x = f64::NAN;
                cur_y = f64::NAN;
            }
            "T*" => {
                if !line_start_x.is_nan() {
                    cur_x = line_start_x;
                }
                if !line_start_y.is_nan() {
                    line_start_y -= line_leading;
                    cur_y = line_start_y;
                }
            }
            "Tm" if operation.operands.len() >= 6 => {
                if let Some(x) = as_f64(&operation.operands[4]) {
                    cur_x = x;
                    line_start_x = x;
                }
                if let Some(y) = as_f64(&operation.operands[5]) {
                    cur_y = y;
                    line_start_y = y;
                }
            }
            "Td" | "TD" if operation.operands.len() >= 2 => {
                if let Some(dx) = as_f64(&operation.operands[0]) {
                    line_start_x = if line_start_x.is_nan() {
                        dx
                    } else {
                        line_start_x + dx
                    };
                    cur_x = line_start_x;
                }
                if let Some(dy) = as_f64(&operation.operands[1]) {
                    line_start_y = if line_start_y.is_nan() {
                        dy
                    } else {
                        line_start_y + dy
                    };
                    cur_y = line_start_y;
                    if dy != 0.0 {
                        line_leading = dy.abs();
                    }
                }
            }
            "Tf" => {
                if let Some(Object::Name(name)) = operation.operands.first() {
                    current_font = name.clone();
                }
                if let Some(size) = operation.operands.get(1).and_then(as_f64)
                    && size > 0.0
                {
                    font_size = size;
                    line_leading = font_size * 1.2;
                }
            }
            "Tj" => {
                if let Some(text) = decode_pdf_text(
                    operation.operands.first(),
                    current_cmap(font_cmaps, raw_fallback, &current_font),
                ) {
                    append_pdf_run(
                        &mut runs,
                        text,
                        cur_x,
                        cur_y,
                        font_size,
                        order,
                        &mut cur_x,
                        line_start_x,
                    );
                }
            }
            "TJ" => {
                if let Some(Object::Array(items)) = operation.operands.first() {
                    let mut text = String::new();
                    for item in items {
                        if let Some(part) = decode_pdf_text(
                            Some(item),
                            current_cmap(font_cmaps, raw_fallback, &current_font),
                        ) {
                            text.push_str(&part);
                        }
                    }
                    append_pdf_run(
                        &mut runs,
                        text,
                        cur_x,
                        cur_y,
                        font_size,
                        order,
                        &mut cur_x,
                        line_start_x,
                    );
                }
            }
            _ => {}
        }
    }
    runs
}

#[allow(clippy::too_many_arguments)]
fn append_pdf_run(
    runs: &mut Vec<PdfTextRun>,
    text: String,
    cur_x: f64,
    cur_y: f64,
    font_size: f64,
    order: usize,
    mutable_cur_x: &mut f64,
    line_start_x: f64,
) {
    let text = normalize_pdf_run_text(&text);
    if text.is_empty() {
        return;
    }
    let x = if cur_x.is_nan() { line_start_x } else { cur_x };
    let width = estimate_pdf_text_width(&text, font_size);
    runs.push(PdfTextRun {
        text,
        x,
        y: cur_y,
        font_size,
        width,
        order,
    });
    if !mutable_cur_x.is_nan() {
        *mutable_cur_x += width;
    }
}

fn as_f64(object: &Object) -> Option<f64> {
    match object {
        Object::Integer(value) => Some(*value as f64),
        Object::Real(value) => Some(*value as f64),
        _ => None,
    }
}

fn current_cmap<'a>(
    font_cmaps: &'a HashMap<Vec<u8>, CidToUnicode>,
    raw_fallback: &'a CidToUnicode,
    current_font: &[u8],
) -> Option<&'a CidToUnicode> {
    font_cmaps
        .get(current_font)
        .or_else(|| (!raw_fallback.is_empty()).then_some(raw_fallback))
}

fn decode_pdf_text(object: Option<&Object>, cmap: Option<&CidToUnicode>) -> Option<String> {
    let object = object?;
    if let Some(text) = decode_hex_like_object(object, cmap) {
        return Some(text);
    }
    match object {
        Object::String(bytes, _) => Some(decode_pdf_string(bytes)),
        Object::Name(name) => Some(String::from_utf8_lossy(name).into_owned()),
        Object::Array(_) => None,
        _ => None,
    }
}

fn decode_hex_like_object(object: &Object, cmap: Option<&CidToUnicode>) -> Option<String> {
    let cmap = cmap?;
    let Object::String(bytes, lopdf::StringFormat::Hexadecimal) = object else {
        return None;
    };
    if bytes.len() % 2 != 0 {
        return None;
    }
    let mut out = String::new();
    // Length is verified even above, so the remainder from as_chunks is empty.
    for chunk in bytes.as_chunks::<2>().0 {
        let cid = u16::from_be_bytes(*chunk);
        if let Some(ch) = cmap.get(&cid) {
            out.push(*ch);
        }
    }
    Some(out)
}

fn decode_pdf_string(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 1;
            match bytes[i] {
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'b' => out.push('\u{0008}'),
                b'f' => out.push('\u{000C}'),
                b'(' | b')' | b'\\' => out.push(bytes[i] as char),
                b'0'..=b'7' => {
                    let mut value = u32::from(bytes[i] - b'0');
                    for _ in 0..2 {
                        if i + 1 < bytes.len()
                            && bytes[i + 1].is_ascii_digit()
                            && bytes[i + 1] < b'8'
                        {
                            i += 1;
                            value = value * 8 + u32::from(bytes[i] - b'0');
                        }
                    }
                    out.push((value as u8) as char);
                }
                byte => out.push(byte as char),
            }
        } else {
            out.push(bytes[i] as char);
        }
        i += 1;
    }
    out
}

fn normalize_pdf_run_text(text: &str) -> String {
    let re = RE_WHITESPACE.get_or_init(|| Regex::new(r"[ \t]+").unwrap());
    re.replace_all(text.trim(), " ").into_owned()
}

fn estimate_pdf_text_width(text: &str, font_size: f64) -> f64 {
    text.chars()
        .map(|ch| {
            if ch.is_whitespace() {
                font_size * 0.35
            } else if ch.is_ascii() {
                font_size * 0.55
            } else {
                font_size
            }
        })
        .sum()
}

fn render_pdf_text_runs(runs: &[PdfTextRun]) -> String {
    if runs.is_empty() {
        return String::new();
    }
    let mut lines = group_pdf_text_runs(runs);
    let base_x = lines
        .iter()
        .filter_map(|line| first_known_x(&line.runs))
        .fold(f64::NAN, |acc, x| if acc.is_nan() { x } else { acc.min(x) });
    let mut out = String::new();
    for (i, line) in lines.iter_mut().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        render_pdf_line(&mut out, line, base_x);
    }
    out.trim_end_matches('\n').to_string()
}

fn group_pdf_text_runs(runs: &[PdfTextRun]) -> Vec<PdfTextLine> {
    let mut lines: Vec<PdfTextLine> = Vec::new();
    for run in runs {
        if let Some(line) = lines.iter_mut().find(|line| same_pdf_text_line(line, run)) {
            line.runs.push(run.clone());
            line.y = if line.y.is_nan() {
                run.y
            } else {
                (line.y + run.y) / 2.0
            };
            line.font_size = line.font_size.max(run.font_size);
        } else {
            lines.push(PdfTextLine {
                runs: vec![run.clone()],
                y: run.y,
                font_size: run.font_size,
            });
        }
    }
    lines
}

fn same_pdf_text_line(line: &PdfTextLine, run: &PdfTextRun) -> bool {
    if line.y.is_nan() || run.y.is_nan() {
        return false;
    }
    (line.y - run.y).abs() <= (line.font_size.max(run.font_size) * 0.6).max(1.5)
}

fn first_known_x(runs: &[PdfTextRun]) -> Option<f64> {
    runs.iter()
        .find_map(|run| (!run.x.is_nan()).then_some(run.x))
}

fn render_pdf_line(out: &mut String, line: &mut PdfTextLine, base_x: f64) {
    line.runs.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.order.cmp(&b.order))
    });
    let font_size = if line.font_size > 0.0 {
        line.font_size
    } else {
        DEFAULT_PDF_FONT_SIZE
    };
    let space_width = (font_size * 0.5).max(1.0);
    let indent_width = (font_size * 4.0).max(1.0);
    if let Some(x) = first_known_x(&line.runs)
        && !base_x.is_nan()
    {
        let indent = (((x - base_x) / indent_width).round() as i32).clamp(0, 12) as usize;
        for _ in 0..indent {
            out.push(' ');
        }
    }

    let mut prev_end_x = f64::NAN;
    for (i, run) in line.runs.iter().enumerate() {
        if i > 0
            && !prev_end_x.is_nan()
            && !run.x.is_nan()
            && run.x - prev_end_x > space_width * 0.25
        {
            out.push(' ');
        }
        out.push_str(&run.text);
        if !run.x.is_nan() {
            prev_end_x = run.x + run.width;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use lopdf::{Stream, dictionary};
    use std::io::Write;

    const SAMPLE_TEXT_CONTENT: &str = "BT\n/F1 12 Tf\n1 0 0 1 72 720 Tm\n<0001000200030004> Tj\nET";
    const SAMPLE_TEXT_CMAP: &str = "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n1 begincodespacerange\n<0001> <0004>\nendcodespacerange\n4 beginbfchar\n<0001> <4F60>\n<0002> <597D>\n<0003> <4E16>\n<0004> <754C>\nendbfchar\nendcmap\nend\nend";

    fn sample_text_pdf_data() -> Vec<u8> {
        build_test_pdf(SAMPLE_TEXT_CONTENT, Some(SAMPLE_TEXT_CMAP), true)
    }

    /// `font_has_to_unicode: false` emits the cmap stream without linking it to
    /// the font, so text extraction must rely on the raw-scan fallback.
    fn build_test_pdf(content: &str, cmap: Option<&str>, font_has_to_unicode: bool) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        ];
        objects.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string());
        objects.push(if cmap.is_some() && font_has_to_unicode {
            "<< /Type /Font /Subtype /Type0 /BaseFont /Helvetica /ToUnicode 6 0 R >>".to_string()
        } else {
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string()
        });
        objects.push(stream_object(content));
        if let Some(cmap) = cmap {
            objects.push(stream_object(cmap));
        }

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

    fn stream_object(body: &str) -> String {
        format!("<< /Length {} >>\nstream\n{}\nendstream", body.len(), body)
    }

    fn zlib_bytes(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn xref_stream_pdf(content: &[u8]) -> Vec<u8> {
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.5\n");
        let object_offset = pdf.len();
        pdf.extend_from_slice(b"1 0 obj\n");
        pdf.extend_from_slice(
            format!(
                "<< /Type /XRef /Size 1 /W [1 1 1] /Root 1 0 R /Filter /FlateDecode /Length {} >>\n",
                content.len()
            )
            .as_bytes(),
        );
        pdf.extend_from_slice(b"stream\n");
        pdf.extend_from_slice(content);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        pdf.extend_from_slice(format!("startxref\n{object_offset}\n%%EOF").as_bytes());
        pdf
    }

    fn build_pdf_doc_with_content_streams(streams: Vec<Stream>) -> Document {
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let content_refs = streams
            .into_iter()
            .map(|stream| Object::Reference(doc.add_object(stream)))
            .collect::<Vec<_>>();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => dictionary! {},
            "Contents" => Object::Array(content_refs),
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc
    }

    #[test]
    fn load_pdf_lenient_bounds_xref_stream_decompression() {
        let compressed = zlib_bytes(&[0; MAX_PDF_DECOMPRESSED_STREAM_BYTES + 1]);
        assert!(compressed.len() < MAX_PDF_DECOMPRESSED_STREAM_BYTES);
        let pdf = xref_stream_pdf(&compressed);

        let err = load_pdf_lenient(&pdf).unwrap_err();

        assert!(matches!(
            err,
            lopdf::Error::Decompress(lopdf::DecompressError::MemoryLimitExceeded { .. })
        ));
    }

    #[test]
    fn sanitize_text_works() {
        assert_eq!(sanitize_text("Hello, world!"), "Hello, world!");
        assert_eq!(sanitize_text("line1\nline2\ttab\r"), "line1\nline2\ttab\r");
        assert_eq!(sanitize_text("\x1b[31mred\x1b[0m"), "[31mred[0m");
        assert_eq!(sanitize_text("hel\0lo"), "hello");
    }

    #[test]
    fn parse_cmap_works() {
        let cmap = parse_cmap(
            "beginbfrange\n<0001><0002><4F60>\nendbfrange\nbeginbfchar\n<0003> <597D>\nendbfchar",
        );
        assert_eq!(cmap.get(&0x0001), Some(&'你'));
        assert_eq!(cmap.get(&0x0002), Some(&'佡'));
        assert_eq!(cmap.get(&0x0003), Some(&'好'));
    }

    #[test]
    fn decode_pdf_string_works() {
        assert_eq!(decode_pdf_string(br"hello\nworld"), "hello\nworld");
        assert_eq!(decode_pdf_string(br"\101\102"), "AB");
    }

    #[test]
    fn extract_text_from_content_stream() {
        let content = Content {
            operations: vec![
                Operation::new(
                    "Tf",
                    vec![Object::Name(b"F1".to_vec()), Object::Integer(12)],
                ),
                Operation::new(
                    "Tj",
                    vec![Object::String(
                        vec![0x4F, 0x60, 0x59, 0x7D],
                        lopdf::StringFormat::Hexadecimal,
                    )],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        let mut cmap = CidToUnicode::new();
        cmap.insert(0x4F60, '你');
        cmap.insert(0x597D, '好');
        let mut font_cmaps = HashMap::new();
        font_cmaps.insert(b"F1".to_vec(), cmap);
        assert_eq!(
            extract_text_from_operations(&content.operations, &font_cmaps, &CidToUnicode::new()),
            "你好"
        );
    }

    #[test]
    fn extracts_text_from_tj_array() {
        let content = Content {
            operations: vec![
                Operation::new(
                    "Tf",
                    vec![Object::Name(b"F1".to_vec()), Object::Integer(12)],
                ),
                Operation::new(
                    "Tm",
                    vec![
                        1.into(),
                        0.into(),
                        0.into(),
                        1.into(),
                        72.into(),
                        720.into(),
                    ],
                ),
                Operation::new(
                    "TJ",
                    vec![Object::Array(vec![
                        Object::string_literal("hel"),
                        Object::Integer(-20),
                        Object::string_literal("lo"),
                    ])],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        assert_eq!(
            extract_text_from_operations(
                &content.operations,
                &HashMap::new(),
                &CidToUnicode::new()
            ),
            "hello"
        );
    }

    #[test]
    fn preserves_tm_line_breaks() {
        let content = Content {
            operations: vec![
                Operation::new(
                    "Tf",
                    vec![Object::Name(b"F1".to_vec()), Object::Integer(12)],
                ),
                Operation::new(
                    "Tm",
                    vec![1.into(), 0.into(), 0.into(), 1.into(), 0.into(), 100.into()],
                ),
                Operation::new("Tj", vec![Object::string_literal("first")]),
                Operation::new(
                    "Tm",
                    vec![1.into(), 0.into(), 0.into(), 1.into(), 0.into(), 80.into()],
                ),
                Operation::new("Tj", vec![Object::string_literal("second")]),
                Operation::new("ET", vec![]),
            ],
        };
        assert_eq!(
            extract_text_from_operations(
                &content.operations,
                &HashMap::new(),
                &CidToUnicode::new()
            ),
            "first\nsecond"
        );
    }

    #[test]
    fn preserves_indent() {
        let content = Content {
            operations: vec![
                Operation::new(
                    "Tf",
                    vec![Object::Name(b"F1".to_vec()), Object::Integer(12)],
                ),
                Operation::new(
                    "Tm",
                    vec![1.into(), 0.into(), 0.into(), 1.into(), 0.into(), 100.into()],
                ),
                Operation::new("Tj", vec![Object::string_literal("base")]),
                Operation::new(
                    "Tm",
                    vec![1.into(), 0.into(), 0.into(), 1.into(), 24.into(), 80.into()],
                ),
                Operation::new("Tj", vec![Object::string_literal("indented")]),
                Operation::new("ET", vec![]),
            ],
        };
        assert_eq!(
            extract_text_from_operations(
                &content.operations,
                &HashMap::new(),
                &CidToUnicode::new()
            ),
            "base\n indented"
        );
    }

    #[test]
    fn preserves_tstar_line_breaks() {
        let content = Content {
            operations: vec![
                Operation::new(
                    "Tf",
                    vec![Object::Name(b"F1".to_vec()), Object::Integer(12)],
                ),
                Operation::new(
                    "Tm",
                    vec![1.into(), 0.into(), 0.into(), 1.into(), 0.into(), 100.into()],
                ),
                Operation::new("Tj", vec![Object::string_literal("first")]),
                Operation::new("T*", vec![]),
                Operation::new("Tj", vec![Object::string_literal("second")]),
                Operation::new("ET", vec![]),
            ],
        };
        assert_eq!(
            extract_text_from_operations(
                &content.operations,
                &HashMap::new(),
                &CidToUnicode::new()
            ),
            "first\nsecond"
        );
    }

    #[test]
    fn text_without_tf_uses_default_font_metrics() {
        // No Tf at all: leading, line grouping, and indentation must fall back
        // to DEFAULT_PDF_FONT_SIZE (12pt → leading 14.4, indent step 48).
        let content = Content {
            operations: vec![
                Operation::new(
                    "Tm",
                    vec![1.into(), 0.into(), 0.into(), 1.into(), 0.into(), 100.into()],
                ),
                Operation::new("Tj", vec![Object::string_literal("first")]),
                Operation::new("T*", vec![]),
                Operation::new("Tj", vec![Object::string_literal("second")]),
                Operation::new(
                    "Tm",
                    vec![1.into(), 0.into(), 0.into(), 1.into(), 96.into(), 60.into()],
                ),
                Operation::new("Tj", vec![Object::string_literal("indented")]),
                Operation::new("ET", vec![]),
            ],
        };
        assert_eq!(
            extract_text_from_operations(
                &content.operations,
                &HashMap::new(),
                &CidToUnicode::new()
            ),
            // 14.4 leading keeps the T* line separate; 96 / 48 = 2 spaces of indent
            "first\nsecond\n  indented"
        );
    }

    #[test]
    fn prepare_pdf_sample_text() {
        let data = sample_text_pdf_data();
        let result = prepare_pdf(&data, "sample-text.pdf", 0).unwrap();
        assert_eq!(result.text.trim(), "你好世界");
        assert!(result.image_data.is_empty());
    }

    #[test]
    fn prepare_pdf_uses_raw_cmap_fallback_when_font_lacks_tounicode() {
        let data = build_test_pdf(SAMPLE_TEXT_CONTENT, Some(SAMPLE_TEXT_CMAP), false);
        let result = prepare_pdf(&data, "raw-fallback.pdf", 0).unwrap();
        assert_eq!(result.text.trim(), "你好世界");
    }

    #[test]
    fn current_cmap_prefers_font_then_nonempty_fallback() {
        let mut font_map = CidToUnicode::new();
        font_map.insert(1, 'A');
        let mut cmaps = HashMap::new();
        cmaps.insert(b"F1".to_vec(), font_map);
        let mut fallback = CidToUnicode::new();
        fallback.insert(1, 'B');

        assert_eq!(
            current_cmap(&cmaps, &fallback, b"F1").unwrap().get(&1),
            Some(&'A'),
            "the font's own cmap wins"
        );
        assert_eq!(
            current_cmap(&cmaps, &fallback, b"F2").unwrap().get(&1),
            Some(&'B'),
            "unknown fonts use the non-empty fallback"
        );
        assert!(
            current_cmap(&cmaps, &CidToUnicode::new(), b"F2").is_none(),
            "an empty fallback yields no cmap"
        );
    }

    #[test]
    fn prepare_pdf_out_of_range_clamps_to_last_page() {
        let data = sample_text_pdf_data();
        let result = prepare_pdf(&data, "sample-text.pdf", 999999).unwrap();
        assert!(result.warning.unwrap().contains("showing last page"));
    }

    #[test]
    fn extract_page_text_rejects_oversized_content_stream() {
        // MAX_PDF_DECOMPRESSED_STREAM_BYTES is 128 bytes in cfg(test).
        // Build a PDF whose content stream is longer than 128 bytes.
        let oversized_content = "BT ".to_string() + &"A ".repeat(100) + "ET";
        assert!(
            oversized_content.len() > 128,
            "sanity: content must exceed test limit"
        );
        let pdf_data = build_test_pdf(&oversized_content, None, false);
        let err = prepare_pdf(&pdf_data, "big.pdf", 1).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("page content") || msg.contains("limit"),
            "expected 'page content' or 'limit' in error, got: {msg}"
        );
    }

    #[test]
    fn extract_page_text_bounds_flate_decompression() {
        let compressed = zlib_bytes(&[b' '; MAX_PDF_DECOMPRESSED_STREAM_BYTES + 1]);
        assert!(
            compressed.len() < MAX_PDF_DECOMPRESSED_STREAM_BYTES,
            "fixture should be small before decompression"
        );
        let doc = build_pdf_doc_with_content_streams(vec![Stream::new(
            dictionary! { "Filter" => "FlateDecode" },
            compressed,
        )]);

        let err = extract_page_text(&doc, 1, &CidToUnicode::new()).unwrap_err();
        assert!(
            err.to_string().contains("limit"),
            "expected bounded decompression error, got: {err}"
        );
    }

    #[test]
    fn extract_page_text_reads_flate_content_within_limit() {
        let content = zlib_bytes(b"BT (hello) Tj ET");
        let doc = build_pdf_doc_with_content_streams(vec![Stream::new(
            dictionary! { "Filter" => "FlateDecode" },
            content,
        )]);

        let text = extract_page_text(&doc, 1, &CidToUnicode::new()).unwrap();

        assert_eq!(text, "hello");
    }

    #[test]
    fn extract_page_text_separates_multiple_content_streams_after_comment() {
        let doc = build_pdf_doc_with_content_streams(vec![
            Stream::new(
                lopdf::Dictionary::new(),
                b"BT (hello) Tj % comment at stream end".to_vec(),
            ),
            Stream::new(lopdf::Dictionary::new(), b"(world) Tj ET".to_vec()),
        ]);

        let text = extract_page_text(&doc, 1, &CidToUnicode::new()).unwrap();

        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn collect_font_cmaps_bounds_tounicode_decompression() {
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let cmap_id = doc.add_object(Stream::new(
            dictionary! { "Filter" => "FlateDecode" },
            zlib_bytes(&[b'A'; MAX_PDF_CMAP_BYTES + 1]),
        ));
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type0",
            "BaseFont" => "Helvetica",
            "ToUnicode" => cmap_id,
        });
        let content_id = doc.add_object(Stream::new(lopdf::Dictionary::new(), Vec::new()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => dictionary! {
                "Font" => dictionary! { "F1" => font_id },
            },
            "Contents" => content_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );

        let err = collect_font_cmaps(&doc, page_id).unwrap_err();

        assert!(err.to_string().contains("ToUnicode CMap"));
        assert!(err.to_string().contains("limit"));
    }

    #[test]
    fn decode_flate_raw_image_rejects_invalid_dimensions_before_allocating() {
        let dict = dictionary! {
            "Width" => -1,
            "Height" => 1,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
        };

        let err = decode_flate_raw_image(&[0, 0, 0], &dict).unwrap_err();

        assert!(err.to_string().contains("invalid PDF image dimensions"));
    }

    #[test]
    fn decode_flate_raw_image_rejects_dimensions_over_image_limits() {
        let dict = dictionary! {
            "Width" => 10001,
            "Height" => 10000,
            "ColorSpace" => "DeviceGray",
            "BitsPerComponent" => 8,
        };

        let err = decode_flate_raw_image(&[], &dict).unwrap_err();

        assert!(err.to_string().contains("exceed limits"));
    }

    #[test]
    fn decode_flate_raw_image_inflates_raw_pixels() {
        let dict = dictionary! {
            "Width" => 1,
            "Height" => 1,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
        };
        let content = zlib_bytes(&[255, 0, 0]);

        let png = decode_flate_raw_image(&content, &dict).unwrap();

        assert!(imgutil::is_png(&png));
    }

    /// Build an in-memory document whose single page holds the given DeviceRGB
    /// images as (width, height, filter, raw stream bytes).
    fn build_pdf_doc_with_images(images: &[(i64, i64, &str, Vec<u8>)]) -> Document {
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let mut xobject = lopdf::Dictionary::new();
        for (i, (width, height, filter, data)) in images.iter().enumerate() {
            let dict = dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => *width,
                "Height" => *height,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
                "Filter" => Object::Name(filter.as_bytes().to_vec()),
            };
            let id = doc.add_object(Stream::new(dict, data.clone()));
            xobject.set(format!("Im{i}").into_bytes(), id);
        }
        let content_id = doc.add_object(Stream::new(lopdf::Dictionary::new(), Vec::new()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => dictionary! { "XObject" => Object::Dictionary(xobject) },
            "Contents" => content_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc
    }

    fn png_dimensions(data: &[u8]) -> (u32, u32) {
        let img = ::image::load_from_memory(data).unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn extract_largest_image_picks_largest_decodable() {
        let doc = build_pdf_doc_with_images(&[
            (1, 1, "FlateDecode", zlib_bytes(&[10, 20, 30])),
            (2, 1, "FlateDecode", zlib_bytes(&[1, 2, 3, 4, 5, 6])),
        ]);
        let png = extract_largest_image(&doc, 1).unwrap();
        assert!(imgutil::is_png(&png));
        assert_eq!(png_dimensions(&png), (2, 1));
    }

    #[test]
    fn extract_largest_image_accepts_valid_dct_jpeg() {
        let image = ::image::RgbImage::from_pixel(2, 2, ::image::Rgb([20, 40, 60]));
        let mut jpeg = Vec::new();
        ::image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&DynamicImage::ImageRgb8(image))
            .unwrap();
        let doc = build_pdf_doc_with_images(&[
            (2, 2, "DCTDecode", jpeg),
            (1, 1, "FlateDecode", zlib_bytes(&[10, 20, 30])),
        ]);

        let encoded = extract_largest_image(&doc, 1).unwrap();

        assert_eq!(png_dimensions(&encoded), (2, 2));
    }

    #[test]
    fn extract_largest_image_falls_back_when_largest_is_corrupt() {
        let doc = build_pdf_doc_with_images(&[
            (4, 4, "FlateDecode", b"not zlib data".to_vec()),
            (1, 1, "FlateDecode", zlib_bytes(&[10, 20, 30])),
        ]);
        let png = extract_largest_image(&doc, 1).unwrap();
        assert!(imgutil::is_png(&png));
        assert_eq!(png_dimensions(&png), (1, 1));
    }

    #[test]
    fn extract_largest_image_errors_when_nothing_decodes() {
        let doc = build_pdf_doc_with_images(&[(2, 2, "FlateDecode", b"broken".to_vec())]);
        let err = extract_largest_image(&doc, 1).unwrap_err();
        assert!(err.to_string().contains("no decodable images"));
    }

    #[test]
    fn extract_largest_image_skips_corrupt_dct_and_unsupported_jpx() {
        let doc = build_pdf_doc_with_images(&[
            (4, 4, "DCTDecode", b"not a jpeg".to_vec()),
            (3, 3, "JPXDecode", b"not a jpeg 2000 image".to_vec()),
            (1, 1, "FlateDecode", zlib_bytes(&[10, 20, 30])),
        ]);

        let png = extract_largest_image(&doc, 1).unwrap();

        assert!(imgutil::is_png(&png));
        assert_eq!(png_dimensions(&png), (1, 1));
    }

    #[test]
    fn decode_ccitt_image_caps_preceding_flate_inflation() {
        let dict = dictionary! {
            "Width" => 1,
            "Height" => 1,
        };
        let content = zlib_bytes(&[0u8; MAX_PDF_DECOMPRESSED_STREAM_BYTES + 1]);
        let filters = vec!["FlateDecode".to_string(), "CCITTFaxDecode".to_string()];

        let err = decode_ccitt_image(&filters, &content, &dict).unwrap_err();

        assert!(err.to_string().contains("PDF image stream exceeds"));
    }
}
