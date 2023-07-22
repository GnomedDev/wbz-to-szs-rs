//! # WBZ Converter
//!
//! This library allows you to convert [WBZ](https://wiki.tockdom.com/wiki/WBZ) files and [WU8](https://wiki.tockdom.com/wiki/WU8_(File_Format))
//! files into [U8](https://wiki.tockdom.com/wiki/U8_(File_Format)) files, and back, for use in Mario Kart Wii modding.
//!
//! This library has not been fully tested, so here be dragons.

#![warn(clippy::pedantic)]
#![allow(clippy::cast_lossless, clippy::similar_names)]

use std::{
    cell::RefCell,
    io::{Cursor, Read, Seek, Write},
    path::Path,
    rc::Rc,
};

use log::{debug, warn};

use crate::{
    iterator::{U8Iterator, U8NodeItem},
    parser::Parser,
    passes::{derive_starting_key, perform_header_pass, perform_pass_one, perform_pass_two},
};

mod iterator;
mod parser;
mod passes;

const U8_MAGIC: [u8; 4] = [0x55, 0xAA, 0x38, 0x2D];
const WU8_MAGIC: u32 = u32::from_ne_bytes(*b"WU8a");

#[derive(Debug, Clone, Copy)]
pub(crate) struct U8Node {
    is_dir: bool,
    name_offset: ux::u24,
    data_offset: u32,
    size: u32,
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("BZip (de)compression failed")]
    BZip(bzip2::Error),
    #[error("The file provided is above 4GB in size")]
    FileTooBig(std::num::TryFromIntError),
    #[error("Underlying error when reading from file")]
    FileOperationFailed(std::io::Error),
    #[error("WBZ file did not contain valid magic")]
    InvalidWBZMagic { found_magic: [u8; 8] },
    #[error("WU8 file did not contain valid magic")]
    InvalidWU8Magic { found_magic: [u8; 4] },
    #[error("U8 file did not contain valid magic")]
    InvalidU8Magic { found_magic: [u8; 4] },
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
pub fn decode_wbz(
    wbz_file: impl Read + Seek + Copy,
    autoadd_path: &Path,
) -> Result<Vec<u8>, Error> {
    debug!("Checking signature of WBZ");
    let mut parser = Parser::new(wbz_file);
    let magic_bytes = parser.read::<8>().map_err(Error::FileOperationFailed)?;

    if magic_bytes != *b"WBZaWU8a" {
        return Err(Error::InvalidWBZMagic {
            found_magic: magic_bytes,
        });
    }

    parser.read::<8>().map_err(Error::FileOperationFailed)?;

    debug!("Decompressing WU8 file");
    let mut wu8_file = Vec::new();
    bzip2::read::BzDecoder::new(wbz_file)
        .read_to_end(&mut wu8_file)
        .map_err(Error::FileOperationFailed)?;

    decode_wu8(&mut wu8_file, autoadd_path)?;
    Ok(wu8_file)
}

/// Compresses a U8 file into the equivalent WBZ file.
///
/// `u8_file` will also be mutated to contain the decompressed WU8 file.
///
/// # Errors
/// Errors if the file is an invalid U8 file, which includes invalid magic or a too large file.
///
/// See [`Error`] for all possible failure states.
pub fn encode_wbz(
    u8_file: &mut [u8],
    mut wbz_file: impl Write,
    autoadd_path: &Path,
) -> Result<(), Error> {
    debug!("Checking signature of U8 file");
    let magic_bytes = u8_file[0..4]
        .try_into()
        .map_err(|_| std::io::ErrorKind::Other.into())
        .map_err(Error::FileOperationFailed)?;

    if magic_bytes != U8_MAGIC {
        return Err(Error::InvalidWU8Magic {
            found_magic: magic_bytes,
        });
    }

    iterate_wu8(u8_file, autoadd_path, true)?;
    let wu8_file = u8_file;
    let wu8_len: u32 = wu8_file.len().try_into().map_err(Error::FileTooBig)?;

    wbz_file
        .write_all(b"WBZa")
        .and_then(|_| wbz_file.write_all(&wu8_file[0..8]))
        .and_then(|_| wbz_file.write_all(&wu8_len.to_be_bytes()))
        .map_err(Error::FileOperationFailed)?;

    bzip2::write::BzEncoder::new(wbz_file, bzip2::Compression::best())
        .write_all(wu8_file)
        .map_err(Error::FileOperationFailed)
}

/// Decodes a WU8 file into the equivalent U8 file **in place**.
///
/// # Errors
/// Errors if the file is an invalid WU8 file, which includes invalid magic or a too large file.
///
/// See [`Error`] for all possible failure states.
pub fn decode_wu8(wu8_file: &mut [u8], autoadd_path: &Path) -> Result<(), Error> {
    iterate_wu8(wu8_file, autoadd_path, false)
}

/// Encodes a U8 file into the equivalent WU8 file **in place**.
///
/// # Errors
/// Errors if the file is an invalid U8 file, which includes invalid magic or a too large file.
///
/// See [`Error`] for all possible failure states.
pub fn encode_wu8(u8_file: &mut [u8], autoadd_path: &Path) -> Result<(), Error> {
    iterate_wu8(u8_file, autoadd_path, true)
}

fn iterate_wu8(file: &mut [u8], autoadd_path: &Path, encode: bool) -> Result<(), Error> {
    let size: u32 = file.len().try_into().map_err(Error::FileTooBig)?;
    let starting_key = derive_starting_key(size);

    let mut reader = RefCell::new(Parser::new(Cursor::new(file)));

    debug!("Parsing header");
    let (starting_key, mut derived_key, header, root_node) = {
        let reader = reader.get_mut();
        let header = if encode {
            reader.read_u8_header::<{ u32::from_le_bytes(U8_MAGIC) }>()?
        } else {
            reader.read_u8_header::<WU8_MAGIC>()?
        };

        let start_pos = reader.position().map_err(Error::FileOperationFailed)?;
        assert_eq!(start_pos, header.node_offset);

        if !encode {
            // First pass, XOR all node and string table bytes with base key
            perform_header_pass(reader.as_mut(), starting_key, start_pos, header.meta_size);
        }

        // Now, get the initial node to find the node table size
        debug!("Calculating offsets for header data");
        let root_node = reader.read_node()?;
        reader
            .set_position(start_pos)
            .map_err(Error::FileOperationFailed)?;

        (starting_key, starting_key, header, root_node)
    };

    // Calculate the metadata for offsets and sizes
    let node_header_size = root_node.size * 12;
    let string_table_start = header.node_offset + node_header_size;

    let mut reader = Rc::new(reader);
    warn!("Starting decode pass 1 (XOR all object files with auto-add library)");
    for item in U8Iterator::new(
        reader.clone(),
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
            reader.borrow_mut().as_mut(),
            &original_data,
            node,
            starting_key,
        );
    }

    {
        // get_mut is safe as the only other handle to the Rc was
        // held by the U8Iterator, which has just been dropped.
        let file = Rc::get_mut(&mut reader).unwrap().get_mut();
        file.set_position(header.node_offset)
            .map_err(Error::FileOperationFailed)?;
    }

    warn!("Starting pass 2 (XOR all non-object files with derived key {derived_key})");

    for item in U8Iterator::new(
        reader.clone(),
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
        perform_pass_two(reader.borrow_mut().as_mut(), node, derived_key);
    }

    if encode {
        // Last pass, XOR all node and string table bytes with base key
        let file = Rc::get_mut(&mut reader).unwrap().get_mut().as_mut();
        perform_header_pass(file, starting_key, header.node_offset, header.meta_size);
    }

    let file_refcell = Rc::try_unwrap(reader).unwrap();
    let file = file_refcell.into_inner().into_inner().into_inner();

    // Setup new magic
    if encode {
        file[0..4].copy_from_slice(&WU8_MAGIC.to_le_bytes());
    } else {
        file[0..4].copy_from_slice(&U8_MAGIC);
    }

    Ok(())
}
