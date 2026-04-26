use std::io::Read;

use crate::imgutil;

use super::{MAX_ARCHIVE_SCAN_BYTES, has_image_extension};

pub(super) fn read_tar_gz_image_bytes(
    path: &str,
    index: Option<usize>,
) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    read_tar_single_pass(decoder, index, path)
}

pub(super) fn read_tar_image_bytes(
    path: &str,
    index: Option<usize>,
) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    read_tar_single_pass(file, index, path)
}

fn read_tar_single_pass<R: Read>(
    reader: R,
    index: Option<usize>,
    path: &str,
) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error>> {
    let mut archive = tar::Archive::new(reader);
    let mut image_count = 0usize;
    let mut chosen = None;
    let mut last = None;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path_name = entry.path()?.to_string_lossy().into_owned();
        if entry.header().entry_type().is_dir() || !has_image_extension(&path_name) {
            continue;
        }
        image_count += 1;
        let size = entry.size();
        if size > imgutil::MAX_INPUT_BYTES as u64 {
            return Err(String::from("archive entry exceeds size limit").into());
        }
        let data = imgutil::read_limited((&mut entry).take(MAX_ARCHIVE_SCAN_BYTES as u64))?;
        if let Some(index) = index {
            if image_count == index {
                return Ok((data, None));
            }
            last = Some(data);
        } else if rand::random_range(0..image_count) == 0 {
            chosen = Some(data);
        }
    }

    if let Some(index) = index {
        let data = last.ok_or_else(|| format!("no images found in archive {path}"))?;
        let warning = if index > image_count {
            Some(format!(
                "warning: index {index} out of range for archive {path}, showing last item {image_count}"
            ))
        } else {
            None
        };
        return Ok((data, warning));
    }
    Ok((
        chosen.ok_or_else(|| format!("no images found in archive {path}"))?,
        None,
    ))
}
