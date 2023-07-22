use log::{debug, info};

use crate::U8Node;

pub(crate) fn derive_starting_key(size: u32) -> u8 {
    let [p0, p1, p2, p3] = size.to_le_bytes();
    let starting_key = p0 ^ p1 ^ p2 ^ p3;

    info!("Derived starting key: {starting_key}");
    starting_key
}

pub(crate) fn perform_header_pass(file: &mut [u8], key: u8, start_pos: u32, meta_size: u32) {
    debug!("Performing node header data pass");
    file.as_mut()
        .iter_mut()
        .skip(start_pos as usize)
        .take(meta_size as usize)
        .for_each(|byte| *byte ^= key);
}

pub(crate) fn perform_pass_one(
    wu8_raw: &mut [u8],
    original_data: &[u8],
    node: U8Node,
    starting_key: u8,
) {
    let mut original_index = 0;
    let mut archive_index = 0;
    while archive_index != node.size {
        if original_index == original_data.len() {
            original_index = 0;
        }

        let archive_offset = node.data_offset + archive_index;
        let enc = &mut wu8_raw[archive_offset as usize];
        *enc ^= starting_key ^ original_data[original_index];

        original_index += 1;
        archive_index += 1;
    }
}

pub(crate) fn perform_pass_two(wu8_raw: &mut [u8], node: U8Node, derived_key: u8) {
    wu8_raw
        .iter_mut()
        .skip(node.data_offset as usize)
        .take(node.size as usize)
        .for_each(|b| *b ^= derived_key);
}
