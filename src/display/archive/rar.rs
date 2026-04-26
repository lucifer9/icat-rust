use rar5::RarArchive;

use super::{MAX_ARCHIVE_SCAN_BYTES, choose_image_index, has_image_extension};

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
    let image_entry_indexes: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| has_image_extension(&e.name))
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
