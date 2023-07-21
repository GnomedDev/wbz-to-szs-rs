//! # WBZ Converter
//!
//! This library allows you to convert [WBZ](https://wiki.tockdom.com/wiki/WBZ) files and [WU8](https://wiki.tockdom.com/wiki/WU8_(File_Format))
//! files into [U8](https://wiki.tockdom.com/wiki/U8_(File_Format)) files, for use in Mario Kart Wii modding.
//!
//! Currently only decompress/decode functionality is implemented, and has not been fully tested, so here be dragons.

#![warn(clippy::pedantic)]
#![allow(clippy::cast_lossless)]

use std::{
    cell::RefCell,
    io::{Cursor, Read, Seek},
    path::Path,
    rc::Rc,
};

use derivative::Derivative;
use log::{debug, warn};

use crate::{
    decrypt::{derive_starting_key, perform_header_pass, perform_pass_one, perform_pass_two},
    iterator::{U8Iterator, U8NodeItem},
    parser::Parser,
};

mod decrypt;
mod iterator;
mod parser;

const U8_MAGIC: [u8; 4] = [0x55, 0xAA, 0x38, 0x2D];
const WU8_MAGIC: u32 = u32::from_ne_bytes(*b"WU8a");

#[derive(Derivative)]
#[derivative(Debug)]
pub(crate) struct U8Header {
    magic: [u8; 4],
    node_offset: u32,
    meta_size: u32,
    data_offset: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct U8Node {
    is_dir: bool,
    name_offset: ux::u24,
    data_offset: u32,
    size: u32,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("BZip decompression failed")]
    BZip(std::io::Error),
    #[error("The file provided is above 4GB in size")]
    FileTooBig(std::num::TryFromIntError),
    #[error("Underlying error when reading from file")]
    FileOperationFailed(std::io::Error),
    #[error("WBZ file did not contain valid magic")]
    InvalidWBZMagic { found_magic: [u8; 8] },
    #[error("WU8 file did not contain valid magic")]
    InvalidWU8Magic { found_magic: [u8; 4] },
    #[error("WBZ file contained an invalid string")]
    InvalidString(std::str::Utf8Error),
    #[error("WBZ file contained an invalid boolean")]
    InvalidBool(u8),
}

/// Decompresses a WBZ file into the equivalent U8 file.
///
/// # Errors
/// Errors if the file is an invalid WBZ file, which includes invalid magic or a too large file.
///
/// See [`Error`] for all possible failure states.
#[allow(clippy::missing_panics_doc)]
pub fn decode_wbz(wbz_file: impl Read + Seek, autoadd_path: &Path) -> Result<Vec<u8>, Error> {
    let mut wbz_reader = Parser::new(wbz_file);

    debug!("Checking signature of WBZ");
    let magic_bytes = wbz_reader.read::<8>().map_err(Error::FileOperationFailed)?;
    if magic_bytes != *b"WBZaWU8a" {
        return Err(Error::InvalidWBZMagic {
            found_magic: magic_bytes,
        });
    }

    wbz_reader.read::<8>().map_err(Error::FileOperationFailed)?; // Skip to the start of the bzip'd data.

    debug!("Decompressing WU8 file");
    let mut wu8_reader_raw = Cursor::new(Vec::new());
    std::io::copy(
        &mut bzip2_rs::DecoderReader::new(wbz_reader.into_inner()),
        &mut wu8_reader_raw,
    )
    .map_err(Error::BZip)?;

    let mut wu8_file = wu8_reader_raw.into_inner();
    decode_wu8(&mut wu8_file, autoadd_path)?;
    Ok(wu8_file)
}

/// Decodes a WU8 file into the equivalent U8 file **in place**.
///
/// # Errors
/// Errors if the file is an invalid WU8 file, which includes invalid magic or a too large file.
///
/// See [`Error`] for all possible failure states.
#[allow(clippy::similar_names, clippy::missing_panics_doc)]
pub fn decode_wu8(wu8_file: &mut [u8], autoadd_path: &Path) -> Result<(), Error> {
    let mut wu8_reader = RefCell::new(Parser::new(Cursor::new(wu8_file)));

    let (starting_key, mut derived_key, header, root_node) = {
        debug!("Parsing WU8 Header");
        let wu8_reader = wu8_reader.get_mut();
        let header = wu8_reader.read_u8_header::<WU8_MAGIC>()?;
        let size: u32 = wu8_reader
            .as_mut()
            .len()
            .try_into()
            .map_err(Error::FileTooBig)?;

        let start_pos = wu8_reader.position().map_err(Error::FileOperationFailed)?;
        assert_eq!(start_pos, header.node_offset);

        // First pass, XOR all node and string table bytes with base key;
        debug!("Decrypting node header data");
        let starting_key = derive_starting_key(size);

        perform_header_pass(
            wu8_reader.as_mut(),
            starting_key,
            start_pos as usize,
            header.meta_size as usize,
        );

        // Now, get the initial node to find the node table size
        debug!("Calculating offsets for header data");
        let root_node = wu8_reader.read_node()?;
        wu8_reader
            .set_position(start_pos)
            .map_err(Error::FileOperationFailed)?;

        (starting_key, starting_key, header, root_node)
    };

    // Calculate the metadata for offsets and sizes
    let node_header_size = root_node.size * 12;
    let string_table_start = header.node_offset + node_header_size;

    let mut wu8_reader = Rc::new(wu8_reader);
    warn!("Starting decode pass 1 (XOR all object files with auto-add library)");
    for item in U8Iterator::new(
        wu8_reader.clone(),
        root_node.size,
        string_table_start,
        autoadd_path,
    ) {
        let (node, name, original_data) = match item {
            U8NodeItem::File {
                node,
                name,
                original_data: Some(original_data),
            } => (node, name, original_data),
            U8NodeItem::Error(err) => return Err(err),
            _ => continue,
        };

        let original_size = original_data.len();
        derived_key ^= original_data[original_size / 2]
            ^ original_data[original_size / 3]
            ^ original_data[original_size / 4];

        debug!("Starting {name} auto-add XOR");
        perform_pass_one(
            wu8_reader.borrow_mut().as_mut(),
            &original_data,
            node,
            starting_key,
        );
    }

    {
        // get_mut is safe as the only other handle to the Rc was
        // held by the U8Iterator, which has just been dropped.
        let file = Rc::get_mut(&mut wu8_reader).unwrap().get_mut();
        file.set_position(header.node_offset)
            .map_err(Error::FileOperationFailed)?;
    }

    warn!("Starting decode pass 2 (XOR all non-object files with derived key {derived_key})");

    for item in U8Iterator::new(
        wu8_reader.clone(),
        root_node.size,
        string_table_start,
        autoadd_path,
    ) {
        let (node, name) = match item {
            U8NodeItem::File {
                node,
                name,
                original_data: None,
            } => (node, name),
            U8NodeItem::Error(err) => return Err(err),
            _ => continue,
        };

        debug!("Starting {name} XOR");
        perform_pass_two(wu8_reader.borrow_mut().as_mut(), node, derived_key);
    }

    let u8_file = Rc::try_unwrap(wu8_reader)
        .unwrap()
        .into_inner()
        .into_inner()
        .into_inner();

    // Setup new magic
    u8_file[0..4].copy_from_slice(&U8_MAGIC);
    Ok(())
}
