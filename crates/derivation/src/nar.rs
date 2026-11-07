//! NAR serialization of a file tree held in memory, and its recursive sha256: the bytes a
//! content-addressed source is hashed and sent as.
//!
//! the format follows `dumpPath` in CppNix 2.35.3 `src/libutil/archive.cc`: length-prefixed
//! strings padded to eight bytes, and directory entries in byte order of their names.

use std::collections::BTreeMap;
use std::fmt;

use crate::Sha256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NarNode {
    Regular { executable: bool, contents: Vec<u8> },
    Symlink { target: String },
    Directory(BTreeMap<String, NarNode>),
}

/// a serialized NAR and the hash a daemon checks it against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nar {
    pub bytes: Vec<u8>,
    pub hash: Sha256,
}

impl NarNode {
    pub fn regular(executable: bool, contents: Vec<u8>) -> Self {
        Self::Regular {
            executable,
            contents,
        }
    }

    /// checks every entry name and symlink target, so a tree that can't exist on disk is refused
    /// before it's hashed.
    fn validate(&self) -> Result<(), NarError> {
        match self {
            Self::Regular { .. } => Ok(()),
            Self::Symlink { target } if target.is_empty() || target.contains('\0') => {
                Err(NarError::SymlinkTarget(target.clone()))
            }
            Self::Symlink { .. } => Ok(()),
            Self::Directory(entries) => {
                for (name, node) in entries {
                    if name.is_empty()
                        || name == "."
                        || name == ".."
                        || name.contains('/')
                        || name.contains('\0')
                    {
                        return Err(NarError::EntryName(name.clone()));
                    }
                    node.validate()?;
                }
                Ok(())
            }
        }
    }

    /// serializes and hashes the tree, refusing one no filesystem can hold: its address would be
    /// one no daemon produces.
    pub fn to_nar(&self) -> Result<Nar, NarError> {
        self.validate()?;
        let mut writer = NarWriter::default();
        writer.string(b"nix-archive-1");
        writer.node(self);
        let hash = Sha256::digest(&writer.bytes);
        Ok(Nar {
            bytes: writer.bytes,
            hash,
        })
    }
}

#[derive(Debug, Default)]
struct NarWriter {
    bytes: Vec<u8>,
}

impl NarWriter {
    fn raw(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn string(&mut self, bytes: &[u8]) {
        let len = u64::try_from(bytes.len()).expect("a length in memory fits in u64");
        self.raw(&len.to_le_bytes());
        self.raw(bytes);
        let padding = (8 - bytes.len() % 8) % 8;
        self.raw(&[0u8; 8][..padding]);
    }

    fn node(&mut self, node: &NarNode) {
        self.string(b"(");
        self.string(b"type");
        match node {
            NarNode::Regular {
                executable,
                contents,
            } => {
                self.string(b"regular");
                if *executable {
                    self.string(b"executable");
                    self.string(b"");
                }
                self.string(b"contents");
                self.string(contents);
            }
            NarNode::Symlink { target } => {
                self.string(b"symlink");
                self.string(b"target");
                self.string(target.as_bytes());
            }
            NarNode::Directory(entries) => {
                self.string(b"directory");
                for (name, child) in entries {
                    self.string(b"entry");
                    self.string(b"(");
                    self.string(b"name");
                    self.string(name.as_bytes());
                    self.string(b"node");
                    self.node(child);
                    self.string(b")");
                }
            }
        }
        self.string(b")");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NarError {
    EntryName(String),
    SymlinkTarget(String),
}

impl fmt::Display for NarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntryName(name) => write!(
                f,
                "'{name}' can't be a directory entry name: it's empty, '.', '..', or holds '/' or NUL"
            ),
            Self::SymlinkTarget(target) => {
                write!(f, "symlink target {target:?} is empty or holds NUL")
            }
        }
    }
}

impl std::error::Error for NarError {}

#[cfg(test)]
mod tests {
    use super::*;

    // `nix-hash --type sha256 --base32` (recursive by default) of a file holding "hello\n" on Lix
    // 2.95.4. a change here moves every source path and every recursive fixed output.
    #[test]
    fn a_regular_file_hashes_as_nix_hash_does() {
        let nar = NarNode::regular(false, b"hello\n".to_vec())
            .to_nar()
            .unwrap();
        assert_eq!(
            nar.hash.to_base32(),
            "04zwf782yjwnh3q6hz5izfd6jyip8kgw6g6yj43fiqhbyhdd0dqw"
        );
        assert_eq!(nar.bytes.len() % 8, 0);
    }

    // the same for a directory with an executable, a symlink and a subdirectory, on Lix 2.95.4.
    #[test]
    fn a_directory_hashes_as_nix_hash_does() {
        let tree = NarNode::Directory(BTreeMap::from([
            (
                "run".to_owned(),
                NarNode::regular(true, b"#!/bin/sh\n".to_vec()),
            ),
            (
                "link".to_owned(),
                NarNode::Symlink {
                    target: "run".to_owned(),
                },
            ),
            (
                "sub".to_owned(),
                NarNode::Directory(BTreeMap::from([(
                    "data".to_owned(),
                    NarNode::regular(false, Vec::new()),
                )])),
            ),
        ]));
        assert_eq!(
            tree.to_nar().unwrap().hash.to_base32(),
            "1bfh1v2a65d038zxqf58729lcqmlsa30zx2qvb7vr77hjv4ghmgb"
        );
    }

    #[test]
    fn impossible_entry_names_are_refused() {
        for bad in ["", ".", "..", "a/b", "a\0b"] {
            let tree = NarNode::Directory(BTreeMap::from([(
                bad.to_owned(),
                NarNode::regular(false, Vec::new()),
            )]));
            assert_eq!(tree.to_nar(), Err(NarError::EntryName(bad.to_owned())));
        }
    }
}
