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
    send_image(png_data, "f=100", image_width, image_height, size, tmux)
}

pub fn send_static_image_rgba_zlib(
    zlib_data: &[u8],
    image_width: u32,
    image_height: u32,
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let format_keys = format!("f=32,o=z,s={image_width},v={image_height}");
    send_image(
        zlib_data,
        &format_keys,
        image_width,
        image_height,
        size,
        tmux,
    )
}

fn send_image(
    data: &[u8],
    format_keys: &str,
    image_width: u32,
    image_height: u32,
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let tmux_image_id = tmux.then(generate_image_id);
    write_image(
        &mut stdout,
        data,
        format_keys,
        image_width,
        image_height,
        size,
        tmux_image_id,
    )?;
    stdout.flush()?;
    Ok(())
}

/// Writes `data` as chunked Kitty graphics commands. Under tmux (an image id is
/// given) the image is placed with Unicode placeholders, so the cell grid must fit
/// the diacritics table.
fn write_image(
    writer: &mut dyn Write,
    data: &[u8],
    format_keys: &str,
    image_width: u32,
    image_height: u32,
    size: Size,
    tmux_image_id: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let placement = match tmux_image_id {
        Some(image_id) => {
            let cell_width = (size.pixel_width / size.cols.max(1)).max(1);
            let cell_height = (size.pixel_height / size.rows.max(1)).max(1);
            let cols = image_width.div_ceil(cell_width) as usize;
            let rows = image_height.div_ceil(cell_height) as usize;
            if cols >= NUMBER_TO_DIACRITIC.len() || rows >= NUMBER_TO_DIACRITIC.len() {
                return Err(format!(
                    "image too large for Unicode placeholders: maximum size is {}x{} cells",
                    NUMBER_TO_DIACRITIC.len() - 1,
                    NUMBER_TO_DIACRITIC.len() - 1
                )
                .into());
            }
            Some((image_id, cols, rows))
        }
        None => None,
    };

    let (esc_prefix, esc_suffix, first_header) = match placement {
        Some((image_id, cols, rows)) => {
            writer.write_all(b"\r")?;
            (
                "\x1bPtmux;\x1b\x1b_G",
                "\x1b\x1b\\\x1b\\",
                format!("a=T,q=2,{format_keys},U=1,c={cols},r={rows},i={image_id},"),
            )
        }
        None => ("\x1b_G", "\x1b\\", format!("a=T,q=2,{format_keys},")),
    };

    let chunk_count = data.len().div_ceil(RAW_CHUNK_SIZE);
    let mut encoded = [0u8; CHUNK_SIZE];
    for (index, chunk) in data.chunks(RAW_CHUNK_SIZE).enumerate() {
        writer.write_all(esc_prefix.as_bytes())?;
        if index == 0 {
            writer.write_all(first_header.as_bytes())?;
        }
        let more = u8::from(index + 1 < chunk_count);
        write!(writer, "m={more};")?;
        let encoded_len =
            base64::engine::general_purpose::STANDARD_NO_PAD.encode_slice(chunk, &mut encoded)?;
        writer.write_all(&encoded[..encoded_len])?;
        writer.write_all(esc_suffix.as_bytes())?;
    }

    if let Some((image_id, cols, rows)) = placement {
        write_unicode_placeholders(writer, image_id, cols, rows)?;
    }
    writer.write_all(b"\n")?;
    Ok(())
}

fn write_unicode_placeholders(
    writer: &mut dyn Write,
    image_id: u32,
    cols: usize,
    rows: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let r = (image_id >> 16) & 0xFF;
    let g = (image_id >> 8) & 0xFF;
    let b = image_id & 0xFF;
    let id_diacritic = NUMBER_TO_DIACRITIC[(image_id >> 24) as usize];

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

    const TMUX_SIZE: Size = Size {
        pixel_width: 8,
        pixel_height: 16,
        cols: 1,
        rows: 1,
    };

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
    fn image_chunks_round_trip_at_base64_boundaries() {
        for len in [
            0,
            1,
            2,
            3,
            RAW_CHUNK_SIZE - 1,
            RAW_CHUNK_SIZE,
            RAW_CHUNK_SIZE + 1,
            RAW_CHUNK_SIZE + 2,
            2 * RAW_CHUNK_SIZE,
            2 * RAW_CHUNK_SIZE + 2,
        ] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            for tmux in [false, true] {
                let mut output = Vec::new();
                write_image(
                    &mut output,
                    &data,
                    "f=100",
                    8,
                    16,
                    TMUX_SIZE,
                    tmux.then_some(0x0102_0304),
                )
                .unwrap();
                let output = String::from_utf8(output).unwrap();
                let (prefix, suffix) = if tmux {
                    ("\x1bPtmux;\x1b\x1b_G", "\x1b\x1b\\\x1b\\")
                } else {
                    ("\x1b_G", "\x1b\\")
                };
                let chunks: Vec<_> = output.split(prefix).skip(1).collect();
                assert_eq!(chunks.len(), len.div_ceil(RAW_CHUNK_SIZE));
                let mut decoded = Vec::new();
                for (index, chunk) in chunks.iter().enumerate() {
                    let (command, _) = chunk.split_once(suffix).unwrap();
                    let (header, payload) = command.split_once(';').unwrap();
                    let more = usize::from(index + 1 < chunks.len());
                    assert!(header.ends_with(&format!("m={more}")));
                    assert!(payload.len() <= CHUNK_SIZE);
                    assert!(!payload.contains('='));
                    decoded.extend(
                        base64::engine::general_purpose::STANDARD_NO_PAD
                            .decode(payload)
                            .unwrap(),
                    );
                }
                assert_eq!(decoded, data, "len={len}, tmux={tmux}");
            }
        }
    }

    #[test]
    fn write_image_non_tmux_single_chunk() {
        let mut buf = Vec::new();
        write_image(
            &mut buf,
            &[0, 1, 2],
            "f=32,o=z,s=2,v=3",
            2,
            3,
            TMUX_SIZE,
            None,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "\x1b_Ga=T,q=2,f=32,o=z,s=2,v=3,m=0;AAEC\x1b\\\n"
        );
    }

    #[test]
    fn write_image_tmux_with_placeholders() {
        let mut buf = Vec::new();
        write_image(
            &mut buf,
            &[0, 1, 2],
            "f=100",
            8,
            16,
            TMUX_SIZE,
            Some(0x0102_0304),
        )
        .unwrap();
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
    fn write_image_tmux_rejects_oversized_placeholder_grid() {
        let mut buf = Vec::new();
        let size = Size {
            pixel_width: 1,
            pixel_height: 1,
            cols: 1,
            rows: 1,
        };
        let err = write_image(
            &mut buf,
            &[0, 1, 2],
            "f=100",
            NUMBER_TO_DIACRITIC.len() as u32,
            1,
            size,
            Some(0x0102_0304),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Unicode placeholders"));
        assert!(buf.is_empty());
    }
}
