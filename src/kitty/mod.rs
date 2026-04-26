mod diacritics;

use std::fmt::Write as _;
use std::io::{self, Write};

use base64::Engine;
use rand::RngExt;

use crate::term::Size;
use diacritics::NUMBER_TO_DIACRITIC;

pub const PLACEHOLDER_CHAR: char = '\u{10EEEE}';
pub const CHUNK_SIZE: usize = 4096;
const RAW_CHUNK_SIZE: usize = CHUNK_SIZE / 4 * 3;

pub fn generate_image_id() -> u32 {
    let mut rng = rand::rng();
    loop {
        let id: u32 = rng.random();
        if id != 0 && id & 0xFF00_0000 != 0 && id & 0x00FF_FF00 != 0 {
            return id;
        }
    }
}

pub fn send_static_image(
    png_data: &[u8],
    image_width: u32,
    image_height: u32,
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let image_id = if tmux { generate_image_id() } else { 0 };
    write_static_image(
        &mut stdout,
        png_data,
        image_width,
        image_height,
        size,
        tmux,
        image_id,
    )?;
    stdout.flush()?;
    Ok(())
}

pub fn write_static_image(
    writer: &mut dyn Write,
    png_data: &[u8],
    image_width: u32,
    image_height: u32,
    size: Size,
    tmux: bool,
    image_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cols = 0usize;
    let mut rows = 0usize;
    if tmux {
        let cell_width = (size.pixel_width / size.cols.max(1)).max(1);
        let cell_height = (size.pixel_height / size.rows.max(1)).max(1);
        cols = image_width.div_ceil(cell_width) as usize;
        rows = image_height.div_ceil(cell_height) as usize;
        if cols >= NUMBER_TO_DIACRITIC.len() || rows >= NUMBER_TO_DIACRITIC.len() {
            return Err(format!(
                "image too large for Unicode placeholders: maximum size is {}x{} cells",
                NUMBER_TO_DIACRITIC.len() - 1,
                NUMBER_TO_DIACRITIC.len() - 1
            )
            .into());
        }
        writer.write_all(b"\r")?;
    }

    let first_header = if tmux {
        format!("a=T,q=2,f=100,U=1,c={cols},r={rows},i={image_id},")
    } else {
        String::from("a=T,q=2,f=100,")
    };
    let (esc_prefix, esc_suffix) = if tmux {
        ("\x1bPtmux;\x1b\x1b_G", "\x1b\x1b\\\x1b\\")
    } else {
        ("\x1b_G", "\x1b\\")
    };

    let mut first = true;
    for chunk in png_data.chunks(RAW_CHUNK_SIZE) {
        writer.write_all(esc_prefix.as_bytes())?;
        if first {
            writer.write_all(first_header.as_bytes())?;
            first = false;
        }
        let more = if std::ptr::eq(chunk.as_ptr_range().end, png_data.as_ptr_range().end) {
            0
        } else {
            1
        };
        write!(writer, "m={more};")?;
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(chunk);
        writer.write_all(encoded.as_bytes())?;
        writer.write_all(esc_suffix.as_bytes())?;
    }

    if tmux {
        write_unicode_placeholders(writer, image_id, cols, rows)?;
    }
    writer.write_all(b"\n")?;
    Ok(())
}

pub fn send_static_image_rgba_zlib(
    zlib_data: &[u8],
    image_width: u32,
    image_height: u32,
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let image_id = if tmux { generate_image_id() } else { 0 };
    write_static_image_rgba_zlib(
        &mut stdout,
        zlib_data,
        image_width,
        image_height,
        size,
        tmux,
        image_id,
    )?;
    stdout.flush()?;
    Ok(())
}

pub fn write_static_image_rgba_zlib(
    writer: &mut dyn Write,
    zlib_data: &[u8],
    image_width: u32,
    image_height: u32,
    size: Size,
    tmux: bool,
    image_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cols = 0usize;
    let mut rows = 0usize;
    if tmux {
        let cell_width = (size.pixel_width / size.cols.max(1)).max(1);
        let cell_height = (size.pixel_height / size.rows.max(1)).max(1);
        cols = image_width.div_ceil(cell_width) as usize;
        rows = image_height.div_ceil(cell_height) as usize;
        if cols >= NUMBER_TO_DIACRITIC.len() || rows >= NUMBER_TO_DIACRITIC.len() {
            return Err(format!(
                "image too large for Unicode placeholders: maximum size is {}x{} cells",
                NUMBER_TO_DIACRITIC.len() - 1,
                NUMBER_TO_DIACRITIC.len() - 1
            )
            .into());
        }
        writer.write_all(b"\r")?;
    }

    let first_header = if tmux {
        format!(
            "a=T,q=2,f=32,o=z,s={image_width},v={image_height},U=1,c={cols},r={rows},i={image_id},"
        )
    } else {
        format!("a=T,q=2,f=32,o=z,s={image_width},v={image_height},")
    };
    let (esc_prefix, esc_suffix) = if tmux {
        ("\x1bPtmux;\x1b\x1b_G", "\x1b\x1b\\\x1b\\")
    } else {
        ("\x1b_G", "\x1b\\")
    };

    let mut first = true;
    for chunk in zlib_data.chunks(RAW_CHUNK_SIZE) {
        writer.write_all(esc_prefix.as_bytes())?;
        if first {
            writer.write_all(first_header.as_bytes())?;
            first = false;
        }
        let more = if std::ptr::eq(chunk.as_ptr_range().end, zlib_data.as_ptr_range().end) {
            0
        } else {
            1
        };
        write!(writer, "m={more};")?;
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(chunk);
        writer.write_all(encoded.as_bytes())?;
        writer.write_all(esc_suffix.as_bytes())?;
    }

    if tmux {
        write_unicode_placeholders(writer, image_id, cols, rows)?;
    }
    writer.write_all(b"\n")?;
    Ok(())
}

pub fn write_unicode_placeholders(
    writer: &mut dyn Write,
    image_id: u32,
    cols: usize,
    rows: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if cols >= NUMBER_TO_DIACRITIC.len() || rows >= NUMBER_TO_DIACRITIC.len() {
        return Err(format!(
            "image too large for Unicode placeholders: maximum size is {}x{} cells",
            NUMBER_TO_DIACRITIC.len() - 1,
            NUMBER_TO_DIACRITIC.len() - 1
        )
        .into());
    }

    let r = (image_id >> 16) & 0xFF;
    let g = (image_id >> 8) & 0xFF;
    let b = image_id & 0xFF;
    let id_idx = ((image_id >> 24) & 0xFF) as usize;
    let id_diacritic = NUMBER_TO_DIACRITIC
        .get(id_idx)
        .copied()
        .unwrap_or(NUMBER_TO_DIACRITIC[0]);

    let mut out = String::new();
    write!(&mut out, "\x1b[38:2:{r}:{g}:{b}m")?;
    for (row, &row_diacritic) in NUMBER_TO_DIACRITIC[..rows].iter().enumerate() {
        for &col_diacritic in NUMBER_TO_DIACRITIC[..cols].iter() {
            out.push(PLACEHOLDER_CHAR);
            out.push(row_diacritic);
            out.push(col_diacritic);
            out.push(id_diacritic);
        }
        if row + 1 < rows {
            out.push_str("\n\r");
        }
    }
    out.push_str("\x1b[39m");
    writer.write_all(out.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_image_id_constraints() {
        for _ in 0..100 {
            let id = generate_image_id();
            assert_ne!(id, 0);
            assert_ne!(id & 0xFF00_0000, 0);
            assert_ne!(id & 0x00FF_FF00, 0);
        }
    }

    #[test]
    fn generate_image_id_uniqueness() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            seen.insert(generate_image_id());
        }
        assert!(seen.len() >= 990, "{}", seen.len());
    }

    #[test]
    fn raw_base64_no_padding() {
        for len in [1_usize, 2, 3, 4, 5, 6, 10, 100, 1000] {
            let data: Vec<u8> = (0..len as u8).collect();
            let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(data);
            assert!(!encoded.contains('='));
        }
    }

    #[test]
    fn diacritics_table() {
        assert_eq!(NUMBER_TO_DIACRITIC.len(), 297);
        assert_eq!(NUMBER_TO_DIACRITIC[0], '\u{0305}');
        assert_eq!(NUMBER_TO_DIACRITIC[296], '\u{1D244}');
    }

    #[test]
    fn write_static_image_non_tmux_single_chunk() {
        let mut buf = Vec::new();
        write_static_image(
            &mut buf,
            &[0, 1, 2],
            1,
            1,
            Size {
                pixel_width: 0,
                pixel_height: 0,
                cols: 0,
                rows: 0,
            },
            false,
            0,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "\x1b_Ga=T,q=2,f=100,m=0;AAEC\x1b\\\n"
        );
    }

    #[test]
    fn write_static_image_non_tmux_multi_chunk() {
        let mut buf = Vec::new();
        let png = vec![1; RAW_CHUNK_SIZE + 1];
        write_static_image(
            &mut buf,
            &png,
            1,
            1,
            Size {
                pixel_width: 0,
                pixel_height: 0,
                cols: 0,
                rows: 0,
            },
            false,
            0,
        )
        .unwrap();
        let got = String::from_utf8(buf).unwrap();
        assert_eq!(got.matches("\x1b_G").count(), 2);
        assert!(got.contains("a=T,q=2,f=100,m=1;"));
        assert!(got.contains("\x1b_Gm=0;"));
    }

    #[test]
    fn write_static_image_rgba_zlib_non_tmux_single_chunk() {
        let mut buf = Vec::new();
        write_static_image_rgba_zlib(
            &mut buf,
            &[0, 1, 2],
            2,
            3,
            Size {
                pixel_width: 0,
                pixel_height: 0,
                cols: 0,
                rows: 0,
            },
            false,
            0,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "\x1b_Ga=T,q=2,f=32,o=z,s=2,v=3,m=0;AAEC\x1b\\\n"
        );
    }

    #[test]
    fn write_static_image_rgba_zlib_non_tmux_multi_chunk() {
        let mut buf = Vec::new();
        let data = vec![1; RAW_CHUNK_SIZE + 1];
        write_static_image_rgba_zlib(
            &mut buf,
            &data,
            2,
            3,
            Size {
                pixel_width: 0,
                pixel_height: 0,
                cols: 0,
                rows: 0,
            },
            false,
            0,
        )
        .unwrap();
        let got = String::from_utf8(buf).unwrap();
        assert_eq!(got.matches("\x1b_G").count(), 2);
        assert!(got.contains("a=T,q=2,f=32,o=z,s=2,v=3,m=1;"));
        assert!(got.contains("\x1b_Gm=0;"));
    }

    #[test]
    fn write_static_image_rgba_zlib_tmux_with_placeholders() {
        let mut buf = Vec::new();
        let size = Size {
            pixel_width: 8,
            pixel_height: 16,
            cols: 1,
            rows: 1,
        };
        let image_id = 0x0102_0304;
        write_static_image_rgba_zlib(&mut buf, &[0, 1, 2], 8, 16, size, true, image_id).unwrap();
        let expected = format!(
            "\r\x1bPtmux;\x1b\x1b_Ga=T,q=2,f=32,o=z,s=8,v=16,U=1,c=1,r=1,i=16909060,m=0;AAEC\x1b\x1b\\\x1b\\\x1b[38:2:2:3:4m{}{}{}{}\x1b[39m\n",
            PLACEHOLDER_CHAR,
            NUMBER_TO_DIACRITIC[0],
            NUMBER_TO_DIACRITIC[0],
            NUMBER_TO_DIACRITIC[1]
        );
        assert_eq!(String::from_utf8(buf).unwrap(), expected);
    }

    #[test]
    fn write_static_image_tmux_with_placeholders() {
        let mut buf = Vec::new();
        let size = Size {
            pixel_width: 8,
            pixel_height: 16,
            cols: 1,
            rows: 1,
        };
        let image_id = 0x0102_0304;
        write_static_image(&mut buf, &[0, 1, 2], 8, 16, size, true, image_id).unwrap();
        let expected = format!(
            "\r\x1bPtmux;\x1b\x1b_Ga=T,q=2,f=100,U=1,c=1,r=1,i=16909060,m=0;AAEC\x1b\x1b\\\x1b\\\x1b[38:2:2:3:4m{}{}{}{}\x1b[39m\n",
            PLACEHOLDER_CHAR,
            NUMBER_TO_DIACRITIC[0],
            NUMBER_TO_DIACRITIC[0],
            NUMBER_TO_DIACRITIC[1]
        );
        assert_eq!(String::from_utf8(buf).unwrap(), expected);
    }

    #[test]
    fn write_static_image_tmux_rejects_oversized_placeholder_grid() {
        let mut buf = Vec::new();
        let size = Size {
            pixel_width: 1,
            pixel_height: 1,
            cols: 1,
            rows: 1,
        };
        let err = write_static_image(
            &mut buf,
            &[0, 1, 2],
            NUMBER_TO_DIACRITIC.len() as u32,
            1,
            size,
            true,
            0x0102_0304,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Unicode placeholders"));
    }

    #[test]
    fn write_unicode_placeholders_rejects_oversized_grid() {
        let mut buf = Vec::new();
        assert!(
            write_unicode_placeholders(&mut buf, 0x0102_0304, NUMBER_TO_DIACRITIC.len(), 1)
                .is_err()
        );
    }
}
