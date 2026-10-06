use std::io::{self, Read, Seek, SeekFrom};

use rar5::RarArchive;

use super::{MAX_ARCHIVE_SCAN_BYTES, choose_image_index, has_image_extension, is_image_extension};

const RAR4_SIGNATURE: &[u8; 7] = b"Rar!\x1a\x07\x00";
const RAR4_HEAD_FILE: u8 = 0x74;
const RAR4_HEAD_NEWSUB: u8 = 0x7a;
const RAR4_HEAD_ENDARC: u8 = 0x7b;
const RAR4_HD_ADD_SIZE: u16 = 0x8000;
const RAR4_FHD_LARGE: u16 = 0x0100;
const RAR4_ATTR_DIRECTORY: u32 = 0x10;
const RAR4_ATTR_UNIX_DIR: u32 = 0o040000;
const RAR4_OS_UNIX: u8 = 3;

#[derive(Debug, Clone)]
struct RarEntryInfo {
    name: String,
    unpacked_size: u64,
    packed_size: u64,
    is_solid: bool,
}

pub(super) fn read_rar_image_bytes(
    path: &str,
    index: Option<usize>,
) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error>> {
    let mut arc = RarArchive::open(path)?;

    let entries: Vec<RarEntryInfo> = arc
        .list()
        .iter()
        .filter(|e| !e.is_dir())
        .map(|e| RarEntryInfo {
            name: e.name().to_string(),
            unpacked_size: e.size(),
            packed_size: e.compressed_size(),
            is_solid: e.header.comp_solid,
        })
        .collect();
    let legacy_extensions = rar4_legacy_extensions(path)?;
    let image_entry_indexes: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(index, e)| rar_entry_has_image_extension(e, *index, &legacy_extensions))
        .map(|(entry_index, _)| entry_index)
        .collect();

    let sel = choose_image_index(image_entry_indexes.len(), index, path)?;
    let entry_index = image_entry_indexes[sel.index];
    let entry = &entries[entry_index];

    validate_rar_selection_bounds(&entries, entry_index)?;

    let data = arc.read(&entry.name)?;
    Ok((data, sel.warning))
}

fn rar_entry_has_image_extension(
    entry: &RarEntryInfo,
    index: usize,
    legacy_extensions: &[Option<String>],
) -> bool {
    has_image_extension(&entry.name)
        || legacy_extensions
            .get(index)
            .and_then(|ext| ext.as_deref())
            .is_some_and(is_image_extension)
}

fn validate_rar_selection_bounds(
    entries: &[RarEntryInfo],
    selected_index: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let selected = &entries[selected_index];
    if selected.unpacked_size > MAX_ARCHIVE_SCAN_BYTES as u64
        || selected.packed_size > MAX_ARCHIVE_SCAN_BYTES as u64
    {
        return Err("archive entry exceeds size limit".into());
    }

    // Solid entries can only be extracted by decoding every entry since the
    // chain start, so the whole prefix counts against the limit.
    let chain_start = rar_solid_chain_start(entries, selected_index);
    let mut packed_total = 0u64;
    let mut unpacked_total = 0u64;
    for entry in &entries[chain_start..=selected_index] {
        packed_total = packed_total.saturating_add(entry.packed_size);
        unpacked_total = unpacked_total.saturating_add(entry.unpacked_size);
        if packed_total > MAX_ARCHIVE_SCAN_BYTES as u64
            || unpacked_total > MAX_ARCHIVE_SCAN_BYTES as u64
        {
            return Err("archive solid chain exceeds size limit".into());
        }
    }

    Ok(())
}

fn rar_solid_chain_start(entries: &[RarEntryInfo], selected_index: usize) -> usize {
    let mut chain_start = selected_index;
    for index in (0..selected_index).rev() {
        if entries[index + 1].is_solid {
            chain_start = index;
        } else {
            break;
        }
    }
    chain_start
}

fn rar4_legacy_extensions(path: &str) -> io::Result<Vec<Option<String>>> {
    let mut file = std::fs::File::open(path)?;
    let mut signature = [0u8; 7];
    file.read_exact(&mut signature)?;
    if &signature != RAR4_SIGNATURE {
        return Ok(Vec::new());
    }

    let mut extensions = Vec::new();
    loop {
        let mut common = [0u8; 7];
        match file.read_exact(&mut common) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(err),
        }

        let header_type = common[2];
        let flags = u16::from_le_bytes([common[3], common[4]]);
        let header_size = u16::from_le_bytes([common[5], common[6]]) as usize;
        if header_size < 7 {
            break;
        }

        // File and service headers keep their data size inside the header body.
        let has_add_size = flags & RAR4_HD_ADD_SIZE != 0
            && header_type != RAR4_HEAD_FILE
            && header_type != RAR4_HEAD_NEWSUB;
        let add_size = if has_add_size {
            let mut size = [0u8; 4];
            file.read_exact(&mut size)?;
            u32::from_le_bytes(size) as u64
        } else {
            0
        };

        let consumed_after_common = if has_add_size { 4 } else { 0 };
        let ext_len = header_size.saturating_sub(7 + consumed_after_common);
        let mut ext = vec![0u8; ext_len];
        file.read_exact(&mut ext)?;

        match header_type {
            RAR4_HEAD_FILE => {
                let entry = parse_rar4_legacy_entry(&ext, flags);
                let packed_size = entry.as_ref().map_or(0, |entry| entry.packed_size);
                if !entry.as_ref().is_some_and(|entry| entry.is_dir) {
                    extensions.push(entry.and_then(|entry| entry.extension));
                }
                file.seek(SeekFrom::Current(packed_size as i64))?;
            }
            RAR4_HEAD_NEWSUB => {
                let packed_size =
                    parse_rar4_legacy_entry(&ext, flags).map_or(0, |entry| entry.packed_size);
                file.seek(SeekFrom::Current(packed_size as i64))?;
            }
            RAR4_HEAD_ENDARC => break,
            _ => {
                file.seek(SeekFrom::Current(add_size as i64))?;
            }
        }
    }

    Ok(extensions)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rar4LegacyEntry {
    extension: Option<String>,
    packed_size: u64,
    is_dir: bool,
}

fn parse_rar4_legacy_entry(ext: &[u8], flags: u16) -> Option<Rar4LegacyEntry> {
    let mut pos = 0usize;
    let packed_low = read_u32_le(ext, &mut pos)? as u64;
    let unpacked_low = read_u32_le(ext, &mut pos)? as u64;
    let host_os = *ext.get(pos)?;
    // Skip the host OS byte read above, then file CRC, time, unpack version, and method.
    pos += 1 + 10;
    let name_size = read_u16_le(ext, &mut pos)? as usize;
    let file_attr = read_u32_le(ext, &mut pos)?;

    let mut packed_size = packed_low;
    let mut unpacked_size = unpacked_low;
    if flags & RAR4_FHD_LARGE != 0 {
        packed_size |= (read_u32_le(ext, &mut pos)? as u64) << 32;
        unpacked_size |= (read_u32_le(ext, &mut pos)? as u64) << 32;
    }

    let name = ext.get(pos..pos + name_size)?;
    let is_dir = (host_os == RAR4_OS_UNIX && file_attr & (RAR4_ATTR_UNIX_DIR << 16) != 0)
        || file_attr & RAR4_ATTR_DIRECTORY != 0
        || (unpacked_size == 0
            && name
                .split(|byte| *byte == 0)
                .next()
                .is_some_and(|name| name.ends_with(b"/") || name.ends_with(b"\\")));
    Some(Rar4LegacyEntry {
        extension: rar4_ascii_extension(name),
        packed_size,
        is_dir,
    })
}

fn rar4_ascii_extension(name: &[u8]) -> Option<String> {
    let base_name = name.split(|byte| *byte == 0).next()?;
    let dot = base_name.iter().rposition(|byte| *byte == b'.')?;
    let ext = &base_name[dot + 1..];
    if ext.is_empty() || !ext.iter().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    Some(
        ext.iter()
            .map(|byte| char::from(byte.to_ascii_lowercase()))
            .collect(),
    )
}

fn read_u16_le(data: &[u8], pos: &mut usize) -> Option<u16> {
    let bytes: [u8; 2] = data.get(*pos..*pos + 2)?.try_into().ok()?;
    *pos += 2;
    Some(u16::from_le_bytes(bytes))
}

fn read_u32_le(data: &[u8], pos: &mut usize) -> Option<u32> {
    let bytes: [u8; 4] = data.get(*pos..*pos + 4)?.try_into().ok()?;
    *pos += 4;
    Some(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rar4_ascii_extension_uses_name_before_unicode_tail() {
        let name = b"\xbe\xed10/0001.JPG\0unicode-tail";

        assert_eq!(rar4_ascii_extension(name).as_deref(), Some("jpg"));
    }

    #[test]
    fn parse_rar4_legacy_entry_reads_name_after_method() {
        let mut ext = Vec::new();
        ext.extend_from_slice(&3u32.to_le_bytes());
        ext.extend_from_slice(&4u32.to_le_bytes());
        ext.push(0);
        ext.extend_from_slice(&0u32.to_le_bytes());
        ext.extend_from_slice(&0u32.to_le_bytes());
        ext.push(0x1d);
        ext.push(0x30);
        ext.extend_from_slice(&(b"dir/image.JPG\0tail".len() as u16).to_le_bytes());
        ext.extend_from_slice(&0u32.to_le_bytes());
        ext.extend_from_slice(b"dir/image.JPG\0tail");

        let entry = parse_rar4_legacy_entry(&ext, 0).unwrap();

        assert_eq!(entry.packed_size, 3);
        assert_eq!(entry.extension.as_deref(), Some("jpg"));
        assert!(!entry.is_dir);
    }

    fn rar4_header(header_type: u8, flags: u16, body: &[u8]) -> Vec<u8> {
        let mut header = vec![0, 0, header_type];
        header.extend_from_slice(&flags.to_le_bytes());
        header.extend_from_slice(&((7 + body.len()) as u16).to_le_bytes());
        header.extend_from_slice(body);
        header
    }

    #[test]
    fn rar4_legacy_extensions_skips_zero_add_size_field() {
        let name = b"image.JPG";
        let mut file_ext = Vec::new();
        file_ext.extend_from_slice(&0u32.to_le_bytes()); // packed size
        file_ext.extend_from_slice(&1u32.to_le_bytes()); // unpacked size
        file_ext.push(0); // host OS
        file_ext.extend_from_slice(&[0; 10]); // CRC, time, version, method
        file_ext.extend_from_slice(&(name.len() as u16).to_le_bytes());
        file_ext.extend_from_slice(&0u32.to_le_bytes()); // attributes
        file_ext.extend_from_slice(name);

        let mut data = RAR4_SIGNATURE.to_vec();
        // A non-file header whose ADD_SIZE field is present but zero.
        data.extend(rar4_header(0x73, RAR4_HD_ADD_SIZE, &0u32.to_le_bytes()));
        data.extend(rar4_header(RAR4_HEAD_FILE, 0, &file_ext));
        data.extend(rar4_header(RAR4_HEAD_ENDARC, 0, &[]));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.rar");
        std::fs::write(&path, data).unwrap();

        let extensions = rar4_legacy_extensions(path.to_str().unwrap()).unwrap();

        assert_eq!(extensions, vec![Some(String::from("jpg"))]);
    }

    #[test]
    fn rar_entry_extension_uses_legacy_rar4_fallback() {
        let entry = RarEntryInfo {
            name: "@w10/\u{be}i10_0001.jpg@w0\0\0jp".to_string(),
            unpacked_size: 1,
            packed_size: 1,
            is_solid: false,
        };

        assert!(!has_image_extension(&entry.name));
        assert!(rar_entry_has_image_extension(
            &entry,
            0,
            &[Some(String::from("jpg"))]
        ));
    }

    fn write_rar_fixture(path: &std::path::Path, files: &[(&str, &[u8])]) {
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
        write_rar_fixture(
            &path,
            &[
                ("note.txt", b"ignore me"),
                ("a.png", b"first image"),
                ("b.jpg", b"second image"),
            ],
        );
        let (data, warning) = read_rar_image_bytes(path.to_str().unwrap(), Some(2)).unwrap();
        assert_eq!(warning, None);
        assert_eq!(data, b"second image");
    }

    #[test]
    fn test_read_rar_image_bytes_out_of_range_clamps_last() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample_oor.rar");
        write_rar_fixture(
            &path,
            &[("a.png", b"first image"), ("b.png", b"second image")],
        );
        let (data, warning) = read_rar_image_bytes(path.to_str().unwrap(), Some(99)).unwrap();
        assert_eq!(data, b"second image");
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
