use zip::ZipArchive;

use crate::imgutil;

use super::{choose_image_index, has_image_extension};

pub(super) fn read_zip_image_bytes(
    path: &str,
    index: Option<usize>,
) -> Result<(Vec<u8>, Option<String>), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut image_indexes = Vec::new();
    for i in 0..archive.len() {
        let file = archive.by_index_raw(i)?;
        if !file.is_dir() && has_image_extension(file.name()) {
            image_indexes.push(i);
        }
    }
    let selection = choose_image_index(image_indexes.len(), index, path)?;
    let mut file = archive.by_index(image_indexes[selection.index])?;
    let data = imgutil::read_limited(&mut file)?;
    Ok((data, selection.warning))
}
