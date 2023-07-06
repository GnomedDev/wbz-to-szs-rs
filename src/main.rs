#![warn(clippy::pedantic)]
#![allow(clippy::cast_lossless)]

use std::{
    cell::RefCell,
    io::{copy, BufReader, Cursor},
    rc::Rc,
};

use crate::{
    iterator::{U8Iterator, U8NodeItem},
    parser::Parser,
};
use color_eyre::{
    eyre::{Context, ContextCompat},
    Result,
};
use derivative::Derivative;
use log::{debug, info, warn};

mod iterator;
mod parser;

const U8_MAGIC: [u8; 4] = [0x55, 0xAA, 0x38, 0x2D];
const WU8_MAGIC: u32 = u32::from_ne_bytes(*b"WU8a");

#[derive(Derivative)]
#[derivative(Debug)]
pub struct U8Header {
    magic: [u8; 4],
    node_offset: u32,
    meta_size: u32,
    data_offset: u32,
}

#[derive(Debug)]
pub struct U8Node {
    is_dir: bool,
    name_offset: ux::u24,
    data_offset: u32,
    size: u32,
}

#[derive(Debug)]
struct WU8Decoder {
    header: U8Header,
    file: RefCell<Parser<Cursor<Vec<u8>>>>,

    // Key derived from file size, used for pass one.
    starting_key: u8,
    // Key derived from starting_key and the file sizes
    // of the auto-add library files from pass one.
    derived_key: u8,
}

impl WU8Decoder {
    pub fn new(file: RefCell<Parser<Cursor<Vec<u8>>>>) -> Result<Self> {
        debug!("Parsing WU8 Header");

        let (header, size) = {
            let mut file = file.borrow_mut();
            let header = file.read_u8_header::<WU8_MAGIC>()?;
            let size: u32 = file
                .as_mut()
                .len()
                .try_into()
                .wrap_err("File cannot be above 4GB")?;

            (header, size)
        };

        let [p0, p1, p2, p3] = size.to_le_bytes();
        let starting_key = p0 ^ p1 ^ p2 ^ p3;
        info!("Derived starting key: {starting_key}");

        Ok(Self {
            file,
            header,
            starting_key,
            derived_key: starting_key,
        })
    }

    pub fn run(mut self) -> Result<Parser<Cursor<Vec<u8>>>> {
        let file = self.file.get_mut();
        let start_pos = file.position()?;
        assert_eq!(start_pos, self.header.node_offset);

        // First pass, XOR all node and string table bytes with base key;
        debug!("Decrypting node header data");
        file.as_mut()
            .iter_mut()
            .skip(start_pos as usize)
            .take(self.header.meta_size as usize)
            .for_each(|byte| *byte ^= self.starting_key);

        // Now, get the initial node to find the node table size
        debug!("Calculating offsets for header data");
        let root_node = file.read_node()?;
        file.set_position(start_pos)?;

        // Calculate the metadata for offsets and sizes
        let node_header_size = root_node.size * 12;
        let string_table_start = self.header.node_offset + node_header_size;

        // Get rid of the long living mut guard.
        let mut file = Rc::new(self.file);

        warn!("Starting decode pass 1 (XOR all object files with auto-add library)");
        for item in U8Iterator::new(file.clone(), root_node.size, string_table_start) {
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
            self.derived_key ^= original_data[original_size / 2]
                ^ original_data[original_size / 3]
                ^ original_data[original_size / 4];

            debug!("Starting {name} auto-add XOR");

            let mut original_index = 0;
            let mut archive_index = 0;
            let mut wu8_raw_ref = file.borrow_mut();
            let wu8_raw = wu8_raw_ref.as_mut();
            while archive_index != node.size {
                if original_index == (original_size) {
                    original_index = 0;
                }

                let archive_offset = node.data_offset + archive_index;
                let enc = &mut wu8_raw[archive_offset as usize];
                *enc ^= self.starting_key ^ original_data[original_index];

                original_index += 1;
                archive_index += 1;
            }
        }

        {
            // get_mut is safe as the only other handle to the Rc was
            // held by the U8Iterator, which has just been dropped.
            let file = Rc::get_mut(&mut file).unwrap().get_mut();
            file.set_position(start_pos)?;
        }

        warn!(
            "Starting decode pass 2 (XOR all non-object files with derived key {})",
            self.derived_key
        );

        for item in U8Iterator::new(file.clone(), root_node.size, string_table_start) {
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
            file.borrow_mut()
                .as_mut()
                .iter_mut()
                .skip(node.data_offset as usize)
                .take(node.size as usize)
                .for_each(|b| *b ^= self.derived_key);
        }

        Ok(Rc::try_unwrap(file).unwrap().into_inner())
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;

    let colours = fern::colors::ColoredLevelConfig::new();
    fern::Dispatch::new()
        .format(move |out, msg, rec| {
            out.finish(format_args!("[{}] {}", colours.color(rec.level()), msg));
        })
        .level(log::LevelFilter::Info)
        .chain(std::io::stdout())
        .apply()?;

    let mut filename = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .wrap_err("First argument must be a path to a WBZ file")?;

    let wbz_file = std::fs::File::open(&filename)?;
    let mut wbz_reader = Parser::new(BufReader::new(wbz_file));

    debug!("Checking signature of WBZ");
    assert_eq!(wbz_reader.read::<8>()?, *b"WBZaWU8a");
    wbz_reader.read::<8>()?; // Skip to the start of the bzip'd data.

    debug!("Decompressing WU8 file");
    let mut wu8_reader_raw = Cursor::new(Vec::new());
    copy(
        &mut bzip2_rs::DecoderReader::new(wbz_reader.into_inner()),
        &mut wu8_reader_raw,
    )?;

    // Reset Cursor state completely
    let wu8_reader = RefCell::new(Parser::new(Cursor::new(wu8_reader_raw.into_inner())));

    // Create and run the decryption process
    let wu8_decoder = WU8Decoder::new(wu8_reader)?;
    let mut u8_file = wu8_decoder.run()?.into_inner().into_inner();

    // Setup new magic
    u8_file[0..4].copy_from_slice(&U8_MAGIC);

    // Setup new filename
    let mut stem = filename.file_stem().unwrap().to_owned();
    stem.push(".u8");

    filename.set_file_name(stem);

    info!("Decoded WBZ file to U8 file");
    std::fs::write(filename, u8_file)?;
    Ok(())
}
