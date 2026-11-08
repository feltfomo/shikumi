use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use sha2::Digest;

use crate::contract::Sha256;

/// a generated file tree held in memory, keyed by `/`-separated relative path. the ordered map is
/// what makes writing and digesting independent of generation order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    files: BTreeMap<String, String>,
}

impl Tree {
    pub(crate) fn insert(&mut self, path: String, contents: String) {
        let previous = self.files.insert(path, contents);
        debug_assert!(previous.is_none(), "a generator wrote one path twice");
    }

    pub fn files(&self) -> impl Iterator<Item = (&str, &str)> {
        self.files.iter().map(|(p, c)| (p.as_str(), c.as_str()))
    }

    pub fn get(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    /// paths and contents are length-prefixed so no two different trees hash the same input.
    pub fn digest(&self) -> Sha256 {
        let mut hasher = sha2::Sha256::new();
        for (path, contents) in &self.files {
            hasher.update((path.len() as u64).to_le_bytes());
            hasher.update(path.as_bytes());
            hasher.update((contents.len() as u64).to_le_bytes());
            hasher.update(contents.as_bytes());
        }
        Sha256::from_bytes(hasher.finalize().into())
    }

    /// writes into `root`, which must not exist yet or be empty, so a generator never mixes its
    /// output with files it didn't write.
    pub fn write_new(&self, root: &Path) -> io::Result<()> {
        if root.exists() && fs::read_dir(root)?.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} exists and isn't empty", root.display()),
            ));
        }
        for (path, contents) in &self.files {
            let target = root.join(path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(target, contents)?;
        }
        Ok(())
    }
}
