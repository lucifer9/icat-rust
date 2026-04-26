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
pub(super) struct RarEntryInfo {
    pub(super) name: String,
    pub(super) unpacked_size: u64,
    pub(super) packed_size: u64,
    pub(super) is_solid: bool,
}

pub(super) fn read_rar_image_bytes(
    path: &str,
    index: Option<usize>,
) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error>> {
    let mut arc = RarArchive::open(path)?;

    let entries: Vec<RarEntryInfo> = arc
        .list()
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.is_dir())
        .map(|(_, e)| RarEntryInfo {
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

    if image_entry_indexes.is_empty() {
        return Err(format!("no images found in archive {path}").into());
    }

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

pub(super) fn validate_rar_selection_bounds(
    entries: &[RarEntryInfo],
    selected_index: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let selected = entries
        .get(selected_index)
        .ok_or_else(|| String::from("archive entry index out of range"))?;
    if selected.unpacked_size > MAX_ARCHIVE_SCAN_BYTES as u64
        || selected.packed_size > MAX_ARCHIVE_SCAN_BYTES as u64
    {
        return Err("archive entry exceeds size limit".into());
    }

    let chain_start = rar_solid_chain_start(entries, selected_index);
    if chain_start == selected_index {
        return Ok(());
    }

    let mut packed_total = 0u64;
    let mut unpacked_total = 0u64;
    for entry in &entries[chain_start..=selected_index] {
        packed_total = packed_total
            .checked_add(entry.packed_size)
            .ok_or_else(|| String::from("archive solid chain size overflow"))?;
        unpacked_total = unpacked_total
            .checked_add(entry.unpacked_size)
            .ok_or_else(|| String::from("archive solid chain size overflow"))?;
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

        let add_size = if flags & RAR4_HD_ADD_SIZE != 0
            && header_type != RAR4_HEAD_FILE
            && header_type != RAR4_HEAD_NEWSUB
        {
            let mut size = [0u8; 4];
            file.read_exact(&mut size)?;
            u32::from_le_bytes(size) as u64
        } else {
            0
        };

        let consumed_after_common = if add_size > 0 { 4 } else { 0 };
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
    pos += 1;
    pos = pos.checked_add(10)?;
    let name_size = read_u16_le(ext, &mut pos)? as usize;
    let file_attr = read_u32_le(ext, &mut pos)?;

    let mut packed_size = packed_low;
    let mut unpacked_size = unpacked_low;
    if flags & RAR4_FHD_LARGE != 0 {
        packed_size |= (read_u32_le(ext, &mut pos)? as u64) << 32;
        unpacked_size |= (read_u32_le(ext, &mut pos)? as u64) << 32;
    }

    let name = ext.get(pos..pos.checked_add(name_size)?)?;
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
    let base_name = name.split(|byte| *byte == 0).next().unwrap_or(name);
    let dot = base_name.iter().rposition(|byte| *byte == b'.')?;
    let ext = &base_name[dot + 1..];
    if ext.is_empty() || !ext.iter().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    std::str::from_utf8(ext)
        .ok()
        .map(|ext| ext.to_ascii_lowercase())
}

fn read_u16_le(data: &[u8], pos: &mut usize) -> Option<u16> {
    let bytes: [u8; 2] = data.get(*pos..pos.checked_add(2)?)?.try_into().ok()?;
    *pos += 2;
    Some(u16::from_le_bytes(bytes))
}

fn read_u32_le(data: &[u8], pos: &mut usize) -> Option<u32> {
    let bytes: [u8; 4] = data.get(*pos..pos.checked_add(4)?)?.try_into().ok()?;
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
}
