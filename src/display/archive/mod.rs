use std::fmt;
use std::path::Path;

use crate::display::image;
use crate::imgutil;
use crate::term::Size;

mod rar;
mod sevenz;
mod tar;
mod zip;

use rar::read_rar_image_bytes;
#[cfg(test)]
use rar::{RarEntryInfo, validate_rar_selection_bounds};
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

pub fn read_image_bytes(
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

fn has_image_extension(name: &str) -> bool {
    matches!(
        Path::new(name)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase()),
        Some(ext) if is_image_extension(&ext)
    )
}

fn is_image_extension(ext: &str) -> bool {
    matches!(
        ext,
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tiff" | "tif"
    )
}

fn detect_format(path: &str) -> Option<ArchiveFormat> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 8];
    let n = std::io::Read::read(&mut f, &mut header).ok()?;
    let h = &header[..n];

    if h.len() >= 4 && &h[..4] == b"PK\x03\x04" {
        return Some(ArchiveFormat::Zip);
    }
    if h.len() >= 4 && &h[..4] == b"PK\x05\x06" {
        return Some(ArchiveFormat::Zip);
    }
    if h.len() >= 6 && &h[..6] == b"\x37\x7a\xbc\xaf\x27\x1c" {
        return Some(ArchiveFormat::SevenZip);
    }
    if h.len() >= 4 && &h[..4] == b"Rar!" {
        return Some(ArchiveFormat::Rar);
    }
    if h.len() >= 2 && &h[..2] == b"\x1f\x8b" {
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
        return Err(format!("no images found in archive {path}").into());
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
            warning: Some(format!(
                "warning: index {index} out of range for archive {path}, showing last item {total}"
            )),
        });
    }
    Ok(Selection {
        index: rand::random_range(0..total),
        warning: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::image::{DynamicImage, ImageBuffer, Rgba};
    use std::io::Write;

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
    fn has_image_extension_works() {
        for name in [
            "a.png",
            "a.jpg",
            "a.jpeg",
            "a.gif",
            "a.bmp",
            "a.webp",
            "a.tiff",
            "a.tif",
            "photo.PNG",
            "photo.WebP",
        ] {
            assert!(has_image_extension(name), "{name}");
        }
        for name in [
            "doc.txt", "file.pdf", "app.exe", "arch.zip", "src.rs", "noext",
        ] {
            assert!(!has_image_extension(name), "{name}");
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
    fn detect_format_works() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("sample.zip");
        write_zip_fixture(&zip_path, &[("a.png", &png_bytes(1, 1))]);
        assert_eq!(
            detect_format(zip_path.to_str().unwrap()),
            Some(ArchiveFormat::Zip)
        );

        let tgz_path = dir.path().join("sample.tar.gz");
        write_tar_fixture(&tgz_path, true, &[("a.png", &png_bytes(1, 1))]);
        assert_eq!(
            detect_format(tgz_path.to_str().unwrap()),
            Some(ArchiveFormat::TarGz)
        );

        let tar_path = dir.path().join("sample.tar");
        write_tar_fixture(&tar_path, false, &[("a.png", &png_bytes(1, 1))]);
        assert_eq!(
            detect_format(tar_path.to_str().unwrap()),
            Some(ArchiveFormat::Tar)
        );

        let plain = dir.path().join("plain.bin");
        std::fs::write(&plain, b"not an archive").unwrap();
        assert_eq!(detect_format(plain.to_str().unwrap()), None);
    }

    #[test]
    fn test_detect_format_reads_only_header() {
        let dir = tempfile::tempdir().unwrap();

        // ZIP magic at bytes 0-3, followed by arbitrary padding
        let zip_path = dir.path().join("header_only.zip");
        let mut zip_data = b"PK\x03\x04".to_vec();
        zip_data.extend_from_slice(&[0u8; 100]);
        std::fs::write(&zip_path, &zip_data).unwrap();
        assert_eq!(
            detect_format(zip_path.to_str().unwrap()),
            Some(ArchiveFormat::Zip)
        );

        // ZIP end-of-central-directory magic PK\x05\x06
        let zip_ecd_path = dir.path().join("header_ecd.zip");
        let mut zip_ecd_data = b"PK\x05\x06".to_vec();
        zip_ecd_data.extend_from_slice(&[0u8; 100]);
        std::fs::write(&zip_ecd_path, &zip_ecd_data).unwrap();
        assert_eq!(
            detect_format(zip_ecd_path.to_str().unwrap()),
            Some(ArchiveFormat::Zip)
        );

        // gzip magic
        let gz_path = dir.path().join("header_only.tar.gz");
        let mut gz_data = b"\x1f\x8b".to_vec();
        gz_data.extend_from_slice(&[0u8; 100]);
        std::fs::write(&gz_path, &gz_data).unwrap();
        assert_eq!(
            detect_format(gz_path.to_str().unwrap()),
            Some(ArchiveFormat::TarGz)
        );

        // RAR magic
        let rar_path = dir.path().join("header_only.rar");
        let mut rar_data = b"Rar!".to_vec();
        rar_data.extend_from_slice(&[0u8; 100]);
        std::fs::write(&rar_path, &rar_data).unwrap();
        assert_eq!(
            detect_format(rar_path.to_str().unwrap()),
            Some(ArchiveFormat::Rar)
        );

        // TAR fallback by extension (no magic bytes)
        let tar_path = dir.path().join("header_only.tar");
        std::fs::write(&tar_path, b"no magic bytes here").unwrap();
        assert_eq!(
            detect_format(tar_path.to_str().unwrap()),
            Some(ArchiveFormat::Tar)
        );

        // Unknown format with no matching magic or extension
        let unknown_path = dir.path().join("unknown.bin");
        std::fs::write(&unknown_path, b"completely unknown").unwrap();
        assert_eq!(detect_format(unknown_path.to_str().unwrap()), None);
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
    fn test_read_sevenzip_image_bytes_by_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.7z");
        let img = png_bytes(2, 2);
        write_sevenzip_fixture(&path, &[("photo.png", &img)]);
        let (data, warning) = read_image_bytes(path.to_str().unwrap(), Some(1)).unwrap();
        assert!(!data.is_empty());
        assert_eq!(data, img);
        assert!(warning.is_none());
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
        let img = png_bytes(1, 1);
        write_sevenzip_fixture(&path, &[("a.png", &img)]);
        let (data, warning) = read_image_bytes(path.to_str().unwrap(), Some(99)).unwrap();
        assert!(!data.is_empty());
        assert!(warning.is_some());
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

    fn write_rar_fixture(path: &std::path::Path, files: &[(&str, &[u8])]) {
        use rar5::RarArchive;
        let mut ar = RarArchive::create(path).expect("create rar archive");
        for (name, data) in files {
            ar.add_bytes(name, data, 0).expect("add bytes to rar");
        }
        ar.close().expect("close rar archive");
    }

    #[test]
    fn test_read_rar_image_bytes_by_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.rar");
        let first = png_bytes(1, 1);
        let second = png_bytes(2, 2);
        write_rar_fixture(
            &path,
            &[
                ("note.txt", b"ignore me"),
                ("a.png", &first),
                ("b.jpg", &second),
            ],
        );
        let (data, warning) = read_rar_image_bytes(path.to_str().unwrap(), Some(2)).unwrap();
        assert_eq!(warning, None);
        assert_eq!(data, second);
    }

    #[test]
    fn test_read_rar_image_bytes_out_of_range_clamps_last() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample_oor.rar");
        let first = png_bytes(1, 1);
        let second = png_bytes(2, 2);
        write_rar_fixture(&path, &[("a.png", &first), ("b.png", &second)]);
        let (data, warning) = read_rar_image_bytes(path.to_str().unwrap(), Some(99)).unwrap();
        assert_eq!(data, second);
        assert!(warning.is_some());
    }

    #[test]
    fn test_read_rar_image_bytes_no_images_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noimg.rar");
        write_rar_fixture(&path, &[("readme.txt", b"nothing to see here")]);
        let err = read_rar_image_bytes(path.to_str().unwrap(), None).unwrap_err();
        assert!(err.to_string().contains("no images found"));
    }

    #[test]
    fn test_rar_chain_bounds_count_target_packed_size() {
        let entries = vec![RarEntryInfo {
            name: "a.png".to_string(),
            unpacked_size: 1,
            packed_size: MAX_ARCHIVE_SCAN_BYTES as u64 + 1,
            is_solid: false,
        }];

        let err = validate_rar_selection_bounds(&entries, 0).unwrap_err();

        assert_eq!(err.to_string(), "archive entry exceeds size limit");
    }

    #[test]
    fn test_rar_chain_bounds_count_solid_prefix() {
        let entries = vec![
            RarEntryInfo {
                name: "note.txt".to_string(),
                unpacked_size: MAX_ARCHIVE_SCAN_BYTES as u64,
                packed_size: 1,
                is_solid: false,
            },
            RarEntryInfo {
                name: "a.png".to_string(),
                unpacked_size: 1,
                packed_size: 1,
                is_solid: true,
            },
        ];

        let err = validate_rar_selection_bounds(&entries, 1).unwrap_err();

        assert_eq!(err.to_string(), "archive solid chain exceeds size limit");
    }
}
