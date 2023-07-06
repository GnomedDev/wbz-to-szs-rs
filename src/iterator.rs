use std::{cell::RefCell, io::Cursor, path::PathBuf, rc::Rc};

use arrayvec::ArrayVec;

use crate::{parser::Parser, Result, U8Node};

pub enum U8NodeItem {
    File {
        node: U8Node,
        original_data: Option<Vec<u8>>,
        name: String,
    },
    Error(color_eyre::eyre::Error),
    Directory,
}

pub struct U8Iterator {
    file: Rc<RefCell<Parser<Cursor<Vec<u8>>>>>,
    dir_stack: ArrayVec<U8Node, 3>,
    string_table_start: u32,
    node_count: u32,
    iteration: u32,
}

impl U8Iterator {
    pub fn new(
        file: Rc<RefCell<Parser<Cursor<Vec<u8>>>>>,
        nodes: u32,
        string_table_start: u32,
    ) -> Self {
        Self {
            file,
            iteration: 0,
            node_count: nodes,
            string_table_start,
            dir_stack: ArrayVec::new(),
        }
    }
}

impl Iterator for U8Iterator {
    type Item = U8NodeItem;

    fn next(&mut self) -> Option<Self::Item> {
        if self.iteration == self.node_count {
            return None;
        } else {
            self.iteration += 1;
        }

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

        if let Some(current_dir) = self.dir_stack.last() {
            if current_dir.size == self.iteration - 1 {
                let dir_off = current_dir.name_offset.into();

                let dir_res = file.read_string(self.string_table_start, dir_off);
                let dir_name = dir_res.as_deref().unwrap_or("No name!");

                println!("Found the end of {dir_name}");
                self.dir_stack.pop();
            }
        }

        if node.is_dir {
            println!("Entering directory {name}");
            self.dir_stack.push(node);

            return Some(U8NodeItem::Directory);
        }

        let dir_iter = self
            .dir_stack
            .iter()
            .map(|node| file.read_string(self.string_table_start, node.name_offset.into()));

        let path = match std::iter::once(Ok(String::from("/usr/local/share/szs/auto-add/")))
            .chain(dir_iter)
            .chain(std::iter::once(Ok(name.clone())))
            .collect::<Result<PathBuf>>()
        {
            Ok(path) => path,
            Err(err) => return Some(U8NodeItem::Error(err)),
        };

        let original_data = match std::fs::read(path) {
            Ok(data) => Some(data),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Some(U8NodeItem::Error(err.into())),
        };

        Some(U8NodeItem::File {
            node,
            name,
            original_data,
        })
    }
}
