use std::{
    cell::RefCell,
    io::Cursor,
    path::{Path, PathBuf},
    rc::Rc,
};

use arrayvec::ArrayVec;
use log::debug;

use crate::{parser::Parser, Error, U8Node};

pub(crate) enum U8NodeItem {
    File {
        node: U8Node,
        original_data: Option<Vec<u8>>,
        name: String,
    },
    Error(Error),
    Directory,
}

#[allow(clippy::module_name_repetitions)]
pub(crate) struct U8Iterator<'a, 'b> {
    file: Rc<RefCell<Parser<Cursor<&'b mut [u8]>>>>,
    dir_stack: ArrayVec<U8Node, 3>,
    string_table_start: u32,
    autoadd_path: &'a Path,
    node_count: u32,
    iteration: u32,
}

impl<'a, 'b> U8Iterator<'a, 'b> {
    pub fn new(
        file: Rc<RefCell<Parser<Cursor<&'b mut [u8]>>>>,
        nodes: u32,
        string_table_start: u32,
        autoadd_path: &'a Path,
    ) -> Self {
        Self {
            file,
            autoadd_path,
            iteration: 0,
            node_count: nodes,
            string_table_start,
            dir_stack: ArrayVec::new(),
        }
    }
}

impl Iterator for U8Iterator<'_, '_> {
    type Item = U8NodeItem;

    fn next(&mut self) -> Option<Self::Item> {
        if self.iteration == self.node_count {
            return None;
        }

        self.iteration += 1;

        let mut file = self.file.borrow_mut();
        let node = match file.read_node() {
            Ok(node) => node,
            Err(err) => return Some(U8NodeItem::Error(err)),
        };

        let name_offset: u32 = node.name_offset.into();
        // Skip root node
        if [0, 1].contains(&name_offset) {
            return Some(U8NodeItem::Directory);
        }

        let name = match file.read_string(self.string_table_start, name_offset) {
            Ok(name) => name,
            Err(err) => return Some(U8NodeItem::Error(err)),
        };

        while let Some(current_dir) = self.dir_stack.last() {
            if current_dir.size == self.iteration - 1 {
                let dir_off = current_dir.name_offset.into();

                let dir_res = file.read_string(self.string_table_start, dir_off);
                let dir_name = dir_res.as_deref().unwrap_or("No name!");

                debug!("Found the end of {dir_name}");
                self.dir_stack.pop();
            } else {
                break;
            }
        }

        if node.is_dir {
            debug!("Entering directory {name}");
            self.dir_stack.push(node);

            return Some(U8NodeItem::Directory);
        }

        let dir_iter = self
            .dir_stack
            .iter()
            .map(|node| file.read_string(self.string_table_start, node.name_offset.into()));

        let path = match std::iter::once(Ok(self.autoadd_path.to_string_lossy().into_owned()))
            .chain(dir_iter)
            .chain(std::iter::once(Ok(name.clone())))
            .collect::<Result<PathBuf, Error>>()
        {
            Ok(path) => path,
            Err(err) => return Some(U8NodeItem::Error(err)),
        };

        let original_data = match std::fs::read(path) {
            Ok(data) => Some(data),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Some(U8NodeItem::Error(Error::FileOperationFailed(err))),
        };

        Some(U8NodeItem::File {
            node,
            name,
            original_data,
        })
    }
}
