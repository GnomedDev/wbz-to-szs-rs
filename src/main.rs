use std::{
    cell::RefCell,
    io::{copy, BufReader, Cursor},
    rc::Rc,
};

use crate::parser::Parser;
use color_eyre::{eyre::Context, Result};
use derivative::Derivative;
use iterator::{U8Iterator, U8NodeItem};

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

    // "Base XOR Value"
    base_key: u8,
    // "XOR value for additional files"
    extra_key: u8,
}

impl WU8Decoder {
    pub fn new(file: RefCell<Parser<Cursor<Vec<u8>>>>) -> Result<Self> {
        println!("Parsing WU8 Header");

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
        println!("Derived starting key: {starting_key}");

        Ok(Self {
            file,
            header,
            base_key: starting_key,
            extra_key: starting_key,
        })
    }

    pub fn run(mut self) -> Result<Parser<Cursor<Vec<u8>>>> {
        let file = self.file.get_mut();
        let start_pos = file.position()?;
        assert_eq!(start_pos, self.header.node_offset);

        // First pass, XOR all node and string table bytes with base key;
        println!("Decrypting node header data");
        file.as_mut()
            .iter_mut()
            .skip(start_pos as usize)
            .take(self.header.meta_size as usize)
            .for_each(|byte| *byte ^= self.base_key);

        // Now, get the initial node to find the node table size
        println!("Calculating offsets for header data");
        let root_node = file.read_node()?;
        file.set_position(start_pos)?;

        // Calculate the metadata for offsets and sizes
        let node_header_size = root_node.size * 12;
        let string_table_start = self.header.node_offset + node_header_size;

        // Get rid of the long living mut guard.
        let mut file = Rc::new(self.file);

        println!("Starting to iterate over all files");
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
            self.extra_key ^= original_data[original_size / 2]
                ^ original_data[original_size / 3]
                ^ original_data[original_size / 4];

            println!(
                "Starting {name} auto-add XOR using key {} after mutating extra key to {}",
                self.base_key, self.extra_key
            );

            let mut original_index = 0;
            let mut archive_index = 0;
            let mut wu8_raw_ref = file.borrow_mut();
            let wu8_raw = wu8_raw_ref.as_mut();
            while archive_index != node.size {
                if original_index == (original_size) {
                    original_index = 0;
                }

                // TODO: Does header.data_offset need to be added?
                let archive_offset = node.data_offset + archive_index;
                let enc = &mut wu8_raw[archive_offset as usize];
                *enc ^= self.base_key ^ original_data[original_index];

                original_index += 1;
                archive_index += 1;
            }
        }

        println!("Iterating again!");
        {
            // get_mut is safe as the only other handle to the Rc was
            // held by the U8Iterator, which has just been dropped.
            let file = Rc::get_mut(&mut file).unwrap().get_mut();
            file.set_position(start_pos)?;
        }

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

            println!("Starting {name} XOR using key {}", self.extra_key);
            file.borrow_mut()
                .as_mut()
                .iter_mut()
                .skip(node.data_offset as usize)
                .take(node.size as usize)
                .for_each(|b| *b ^= self.extra_key);
        }

        Ok(Rc::try_unwrap(file).unwrap().into_inner())
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;

    let filename = "Aquadrom.wbz";
    let wbz_file = std::fs::File::open(filename)?;
    let mut wbz_reader = Parser::new(BufReader::new(wbz_file));

    println!("Checking signature of WBZ");
    assert_eq!(wbz_reader.read::<8>()?, *b"WBZaWU8a");
    wbz_reader.read::<8>()?; // Skip to the start of the bzip'd data.

    println!("Decompressing WU8 file");
    let mut wu8_reader_raw = Cursor::new(Vec::new());
    copy(
        &mut bzip2_rs::DecoderReader::new(wbz_reader.into_inner()),
        &mut wu8_reader_raw,
    )?;

    // Reset Cursor state completely
    let wu8_reader = RefCell::new(Parser::new(Cursor::new(wu8_reader_raw.into_inner())));

    // Create and run the decryption process
    let wu8_decoder = WU8Decoder::new(wu8_reader)?;
    let u8_reader = wu8_decoder.run()?;

    // Setup new magic
    let mut u8_file = u8_reader.into_inner().into_inner();
    u8_file[0..4].copy_from_slice(&U8_MAGIC);

    std::fs::write("Aquadrom.u8.new", u8_file)?;
    Ok(())
}
