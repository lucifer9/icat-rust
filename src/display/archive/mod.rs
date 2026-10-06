use std::fmt;

use crate::display::image;
use crate::imgutil::{self, has_image_extension, is_image_extension};
use crate::term::Size;

mod rar;
mod sevenz;
mod tar;
mod zip;

use rar::read_rar_image_bytes;
use sevenz::read_seven_zip_image_bytes;
use tar::{read_tar_gz_image_bytes, read_tar_image_bytes};
use zip::read_zip_image_bytes;

const MAX_ARCHIVE_SCAN_BYTES: usize = imgutil::MAX_INPUT_BYTES;

#[derive(Debug)]
pub struct NotArchiveError;

impl fmt::Display for NotArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not an archive")
    }
}

impl std::error::Error for NotArchiveError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Selection {
    index: usize,
    warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFormat {
    Zip,
    TarGz,
    Tar,
    SevenZip,
    Rar,
}

pub fn archive(
    path: &str,
    index: Option<usize>,
    size: Size,
    tmux: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let (data, warning) = read_image_bytes(path, index)?;
    if let Some(warning) = warning {
        eprintln!("{warning}");
    }
    image::image_from_bytes(&data, size, tmux)
}

fn read_image_bytes(
    path: &str,
    index: Option<usize>,
) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error>> {
    let Some(format) = detect_format(path) else {
        return Err(Box::new(NotArchiveError));
    };
    match format {
        ArchiveFormat::Zip => read_zip_image_bytes(path, index),
        ArchiveFormat::TarGz => read_tar_gz_image_bytes(path, index),
        ArchiveFormat::Tar => read_tar_image_bytes(path, index),
        ArchiveFormat::SevenZip => read_seven_zip_image_bytes(path, index),
        ArchiveFormat::Rar => read_rar_image_bytes(path, index),
    }
}

fn detect_format(path: &str) -> Option<ArchiveFormat> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 8];
    let n = std::io::Read::read(&mut f, &mut header).ok()?;
    let h = &header[..n];

    if h.starts_with(b"PK\x03\x04") || h.starts_with(b"PK\x05\x06") {
        return Some(ArchiveFormat::Zip);
    }
    if h.starts_with(b"\x37\x7a\xbc\xaf\x27\x1c") {
        return Some(ArchiveFormat::SevenZip);
    }
    if h.starts_with(b"Rar!") {
        return Some(ArchiveFormat::Rar);
    }
    if h.starts_with(b"\x1f\x8b") {
        return Some(ArchiveFormat::TarGz);
    }

    // Fall back to extension for TAR (no magic)
    if path.to_ascii_lowercase().ends_with(".tar") {
        return Some(ArchiveFormat::Tar);
    }

    None
}

fn choose_image_index(
    total: usize,
    index: Option<usize>,
    path: &str,
) -> Result<Selection, Box<dyn std::error::Error>> {
    if total == 0 {
        return Err(no_images_error(path));
    }
    if let Some(index) = index {
        if index <= total {
            return Ok(Selection {
                index: index - 1,
                warning: None,
            });
        }
        return Ok(Selection {
            index: total - 1,
            warning: Some(out_of_range_warning(index, total, path)),
        });
    }
    Ok(Selection {
        index: rand::random_range(0..total),
        warning: None,
    })
}

fn no_images_error(path: &str) -> Box<dyn std::error::Error> {
    format!("no images found in archive {path}").into()
}

fn out_of_range_warning(index: usize, total: usize, path: &str) -> String {
    format!("warning: index {index} out of range for archive {path}, showing last item {total}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::image::{DynamicImage, ImageBuffer, Rgba};
    use std::io::Write;
    use std::path::Path;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(width, height, Rgba([0, 0, 0, 255])));
        imgutil::encode_png(&image).unwrap()
    }

    fn write_zip_fixture(path: &Path, files: &[(&str, &[u8])]) {
        use ::zip::ZipWriter;
        use ::zip::write::SimpleFileOptions;
        let file = std::fs::File::create(path).unwrap();
        let mut zw = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        for (name, data) in files {
            zw.start_file(name, options).unwrap();
            zw.write_all(data).unwrap();
        }
        zw.finish().unwrap();
    }

    fn write_tar_fixture(path: &Path, gzip: bool, files: &[(&str, &[u8])]) {
        use ::tar::{Builder as TarBuilder, Header as TarHeader};
        let file = std::fs::File::create(path).unwrap();
        if gzip {
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut tb = TarBuilder::new(encoder);
            for (name, data) in files {
                let mut header = TarHeader::new_gnu();
                header.set_mode(0o644);
                header.set_size(data.len() as u64);
                header.set_cksum();
                tb.append_data(&mut header, *name, *data).unwrap();
            }
            tb.into_inner().unwrap().finish().unwrap();
        } else {
            let mut tb = TarBuilder::new(file);
            for (name, data) in files {
                let mut header = TarHeader::new_gnu();
                header.set_mode(0o644);
                header.set_size(data.len() as u64);
                header.set_cksum();
                tb.append_data(&mut header, *name, *data).unwrap();
            }
            tb.finish().unwrap();
        }
    }

    #[test]
    fn choose_image_index_behaviour() {
        assert_eq!(choose_image_index(5, Some(2), "test.zip").unwrap().index, 1);
        let sel = choose_image_index(2, Some(3), "test.zip").unwrap();
        assert_eq!(sel.index, 1);
        assert!(sel.warning.is_some());
        assert!(choose_image_index(0, None, "test.zip").is_err());
    }

    #[test]
    fn detect_format_reads_magic_header_then_tar_extension() {
        let dir = tempfile::tempdir().unwrap();
        let cases: [(&str, &[u8], Option<ArchiveFormat>); 7] = [
            ("local.zip", b"PK\x03\x04", Some(ArchiveFormat::Zip)),
            ("empty.zip", b"PK\x05\x06", Some(ArchiveFormat::Zip)),
            (
                "a.7z",
                b"\x37\x7a\xbc\xaf\x27\x1c",
                Some(ArchiveFormat::SevenZip),
            ),
            ("a.rar", b"Rar!", Some(ArchiveFormat::Rar)),
            ("a.tar.gz", b"\x1f\x8b", Some(ArchiveFormat::TarGz)),
            ("a.tar", b"no magic bytes here", Some(ArchiveFormat::Tar)),
            ("unknown.bin", b"completely unknown", None),
        ];
        for (name, header, expected) in cases {
            let path = dir.path().join(name);
            let mut data = header.to_vec();
            data.extend_from_slice(&[0u8; 100]);
            std::fs::write(&path, &data).unwrap();
            assert_eq!(detect_format(path.to_str().unwrap()), expected, "{name}");
        }
    }

    #[test]
    fn read_image_bytes_not_archive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.bin");
        std::fs::write(&path, b"definitely not an archive").unwrap();
        let err = read_image_bytes(path.to_str().unwrap(), None).unwrap_err();
        assert!(err.downcast_ref::<NotArchiveError>().is_some());
    }

    #[test]
    fn read_zip_image_bytes_by_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.zip");
        let first = png_bytes(1, 1);
        let second = png_bytes(2, 1);
        write_zip_fixture(
            &path,
            &[
                ("note.txt", b"ignore me"),
                ("images/a.png", &first),
                ("images/b.jpg", &second),
            ],
        );
        let (data, warning) = read_zip_image_bytes(path.to_str().unwrap(), Some(2)).unwrap();
        assert_eq!(warning, None);
        assert_eq!(data, second);
    }

    #[test]
    fn read_zip_image_bytes_out_of_range_clamps_last() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.zip");
        let first = png_bytes(1, 1);
        let second = png_bytes(2, 1);
        write_zip_fixture(
            &path,
            &[("images/a.png", &first), ("images/b.png", &second)],
        );
        let (data, warning) = read_zip_image_bytes(path.to_str().unwrap(), Some(3)).unwrap();
        assert_eq!(data, second);
        assert!(warning.is_some());
    }

    #[test]
    fn read_tar_image_bytes_by_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.tar");
        let first = png_bytes(1, 1);
        let second = png_bytes(2, 1);
        write_tar_fixture(
            &path,
            false,
            &[
                ("note.txt", b"ignore me"),
                ("a.png", &first),
                ("b.webp", &second),
            ],
        );
        let (data, warning) = read_tar_image_bytes(path.to_str().unwrap(), Some(2)).unwrap();
        assert_eq!(warning, None);
        assert_eq!(data, second);
    }

    #[test]
    fn read_tar_gz_image_bytes_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.tar.gz");
        let expected = png_bytes(3, 2);
        write_tar_fixture(&path, true, &[("a.png", &expected)]);
        let (data, warning) = read_tar_gz_image_bytes(path.to_str().unwrap(), None).unwrap();
        assert_eq!(warning, None);
        assert_eq!(data, expected);
    }

    #[test]
    fn read_tar_image_bytes_index_out_of_range_clamps_last() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.tar");
        let first = png_bytes(1, 1);
        let second = png_bytes(2, 1);
        write_tar_fixture(&path, false, &[("a.png", &first), ("b.png", &second)]);
        let (data, warning) = read_tar_image_bytes(path.to_str().unwrap(), Some(3)).unwrap();
        assert_eq!(data, second);
        assert!(warning.is_some());
    }

    fn write_sevenzip_fixture(path: &Path, files: &[(&str, &[u8])]) {
        use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
        let mut writer = ArchiveWriter::create(path).expect("create 7z writer");
        for (name, data) in files {
            let entry = ArchiveEntry::new_file(name);
            writer
                .push_archive_entry(entry, Some(std::io::Cursor::new(*data)))
                .expect("add file to 7z");
        }
        writer.finish().expect("finish 7z");
    }

    #[test]
    fn test_read_sevenzip_image_bytes_skips_preceding_entry_streaming() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_preceding.7z");
        let first = vec![b'x'; 1024];
        let second = png_bytes(2, 2);
        write_sevenzip_fixture(&path, &[("note.txt", &first), ("photo.png", &second)]);

        let (data, warning) = read_image_bytes(path.to_str().unwrap(), Some(1)).unwrap();

        assert_eq!(data, second);
        assert!(warning.is_none());
    }

    #[test]
    fn test_read_sevenzip_image_bytes_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_oor.7z");
        let first = png_bytes(1, 1);
        let second = png_bytes(2, 2);
        write_sevenzip_fixture(&path, &[("a.png", &first), ("b.png", &second)]);
        let (data, warning) = read_image_bytes(path.to_str().unwrap(), Some(99)).unwrap();
        assert_eq!(data, second);
        let w = warning.unwrap();
        assert!(w.contains("out of range"), "warning was: {w}");
    }

    #[test]
    fn test_read_zip_image_bytes_random_selection_returns_archive_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("two_images.zip");
        let img_a = png_bytes(1, 1);
        let img_b = png_bytes(2, 2);
        write_zip_fixture(&path, &[("a.png", &img_a), ("b.png", &img_b)]);
        let (data, warning) = read_image_bytes(path.to_str().unwrap(), None).unwrap();

        assert!(warning.is_none());
        assert!(
            data == img_a || data == img_b,
            "random selection should return one of the archive images"
        );
    }
}
