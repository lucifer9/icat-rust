use std::fs::File;
use std::io::{self, Read};

use sevenz_rust2::{
    Archive as SevenZipArchive, BlockDecoder as SevenZipBlockDecoder, Error as SevenZipError,
    Password,
};

use super::{MAX_ARCHIVE_SCAN_BYTES, choose_image_index, has_image_extension};

pub(super) fn read_seven_zip_image_bytes(
    path: &str,
    index: Option<usize>,
) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error>> {
    let archive = SevenZipArchive::open(path)?;
    let image_entries: Vec<_> = archive
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| !file.is_directory() && has_image_extension(file.name()))
        .map(|(file_index, file)| (file_index, file.size()))
        .collect();
    let selection = choose_image_index(image_entries.len(), index, path)?;
    let (target_file_index, target_size) = image_entries[selection.index];
    if target_size > MAX_ARCHIVE_SCAN_BYTES as u64 {
        return Err(String::from("archive entry exceeds size limit").into());
    }
    let target_file = &archive.files[target_file_index];
    if !target_file.has_stream() {
        return Ok((Vec::new(), selection.warning));
    }
    let block_index = archive.stream_map.file_block_index[target_file_index]
        .ok_or(SevenZipError::FileNotFound)?;
    let target_file_ptr = target_file as *const _;
    let mut file = File::open(path)?;
    let password = Password::empty();
    let mut total_read = 0usize;
    let mut data = None;
    SevenZipBlockDecoder::new(1, block_index, &archive, &password, &mut file).for_each_entries(
        &mut |entry, entry_reader| {
            if entry.is_directory() {
                return Ok(true);
            }
            let is_target = std::ptr::eq(entry, target_file_ptr);
            let entry_data = read_entry_limited(entry_reader, &mut total_read, is_target)?;
            if is_target {
                data = entry_data;
                return Ok(false);
            }
            Ok(true)
        },
    )?;
    let data = data.ok_or(SevenZipError::FileNotFound)?;
    Ok((data, selection.warning))
}

fn read_entry_limited(
    reader: &mut dyn Read,
    total_read: &mut usize,
    keep_data: bool,
) -> Result<Option<Vec<u8>>, SevenZipError> {
    // Reading one byte past the remaining budget detects an over-limit entry.
    let mut limited = reader.take((MAX_ARCHIVE_SCAN_BYTES - *total_read) as u64 + 1);
    let (read, data) = if keep_data {
        let mut data = Vec::new();
        (limited.read_to_end(&mut data)?, Some(data))
    } else {
        (io::copy(&mut limited, &mut io::sink())? as usize, None)
    };
    *total_read += read;
    if *total_read > MAX_ARCHIVE_SCAN_BYTES {
        return Err(SevenZipError::Other(
            "archive scan exceeds size limit".into(),
        ));
    }
    Ok(data)
}
