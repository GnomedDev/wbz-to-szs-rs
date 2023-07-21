use std::io::{Read, Seek};

use crate::{Error, U8Header, U8Node};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Parser<T: Read + Seek>(T);

impl<T: Read + Seek> Parser<T> {
    pub fn new(reader: T) -> Self {
        Self(reader)
    }

    pub fn into_inner(self) -> T {
        self.0
    }

    pub fn position(&mut self) -> std::io::Result<u32> {
        self.0
            .stream_position()
            .map(u32::try_from)
            .map(Result::unwrap)
    }

    pub fn set_position(&mut self, pos: u32) -> std::io::Result<()> {
        self.0.seek(std::io::SeekFrom::Start(pos as u64)).map(drop)
    }

    pub fn read<const N: usize>(&mut self) -> Result<[u8; N], std::io::Error> {
        let mut buf = [0; N];
        self.0.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn read_byte(&mut self) -> Result<u8, std::io::Error> {
        self.read().map(|[b]| b)
    }

    pub fn read_bool(&mut self) -> Result<bool, Error> {
        let byte = self.read::<1>().map_err(Error::FileOperationFailed)?;
        match byte[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::InvalidBool(byte[0])),
        }
    }

    pub fn read_u24(&mut self) -> Result<ux::u24, std::io::Error> {
        let bytes = self.read::<3>()?;
        let padded = [0, bytes[0], bytes[1], bytes[2]];

        Ok(ux::u24::new(u32::from_be_bytes(padded)))
    }

    pub fn read_u32(&mut self) -> Result<u32, std::io::Error> {
        let bytes = self.read::<4>()?;
        Ok(u32::from_be_bytes(bytes))
    }

    /// Reads a null terminated string from the string table.
    ///
    /// Does not change the position of the buffer, as that is reset after reading.
    pub fn read_string(&mut self, table_start: u32, table_offset: u32) -> Result<String, Error> {
        let starting_pos = self.position().map_err(Error::FileOperationFailed)?;
        self.set_position(table_start + table_offset)
            .map_err(Error::FileOperationFailed)?;

        let mut out = String::new();
        loop {
            let byte = self.read_byte().map_err(Error::FileOperationFailed)?;
            if byte == b'\0' {
                self.set_position(starting_pos)
                    .map_err(Error::FileOperationFailed)?;
                return Ok(out);
            }

            let byte_str = [byte];
            out.push_str(std::str::from_utf8(&byte_str).map_err(Error::InvalidString)?);
        }
    }

    pub fn read_u8_header<const MAGIC: u32>(&mut self) -> Result<U8Header, Error> {
        let header = U8Header {
            magic: self.read().map_err(Error::FileOperationFailed)?,
            node_offset: self.read_u32().map_err(Error::FileOperationFailed)?,
            meta_size: self.read_u32().map_err(Error::FileOperationFailed)?,
            data_offset: self.read_u32().map_err(Error::FileOperationFailed)?,
        };

        // Skip the padding
        self.read::<16>().map_err(Error::FileOperationFailed)?;

        let correct_magic = MAGIC.to_ne_bytes();
        if header.magic == correct_magic {
            Ok(header)
        } else {
            Err(Error::InvalidWU8Magic {
                found_magic: header.magic,
            })
        }
    }

    pub fn read_node(&mut self) -> Result<U8Node, Error> {
        Ok(U8Node {
            is_dir: self.read_bool()?,
            name_offset: self.read_u24().map_err(Error::FileOperationFailed)?,
            data_offset: self.read_u32().map_err(Error::FileOperationFailed)?,
            size: self.read_u32().map_err(Error::FileOperationFailed)?,
        })
    }
}

impl<T: AsRef<[u8]>> AsMut<T> for Parser<std::io::Cursor<T>> {
    fn as_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }
}
