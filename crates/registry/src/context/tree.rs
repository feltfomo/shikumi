//! the NAR hash and size of a store copy, computed before the copy is made: the evaluation enters
//! each copy by its hash, the copying evaluation checks the copy against it, and an evaluation's
//! own store registers the copy with both. a keyed part's NAR comes from the bytes staging already
//! read, so its files are read once.

use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use shikumi_derivation::{NarNode, Sha256};

/// a tree's NAR hash and the NAR's size in bytes, as a store registers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Nar {
    pub(crate) hash: Sha256,
    pub(crate) size: u64,
}

/// the NAR of everything under `directory`, symlinks as themselves.
pub(crate) fn directory(directory: &Path) -> io::Result<Nar> {
    nar(&walk(directory)?)
}

/// the NAR of a tree holding exactly `files`, relative paths each with whether it's executable and
/// its bytes, and the directories on the way to them.
pub(crate) fn files<'f>(
    files: impl IntoIterator<Item = (&'f str, bool, &'f [u8])>,
) -> io::Result<Nar> {
    let mut tree = BTreeMap::new();
    for (file, executable, contents) in files {
        insert(
            &mut tree,
            file,
            NarNode::regular(executable, contents.to_vec()),
        )?;
    }
    nar(&NarNode::Directory(tree))
}

fn walk(path: &Path) -> io::Result<NarNode> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(path)?;
        return Ok(NarNode::Symlink {
            target: target.to_string_lossy().into_owned(),
        });
    }
    if metadata.is_dir() {
        let mut entries = BTreeMap::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            entries.insert(name, walk(&entry.path())?);
        }
        return Ok(NarNode::Directory(entries));
    }
    let executable = metadata.permissions().mode() & 0o100 != 0;
    Ok(NarNode::regular(executable, std::fs::read(path)?))
}

fn insert(tree: &mut BTreeMap<String, NarNode>, file: &str, node: NarNode) -> io::Result<()> {
    match file.split_once('/') {
        None => {
            tree.insert(file.to_owned(), node);
            Ok(())
        }
        Some((directory, rest)) => match tree
            .entry(directory.to_owned())
            .or_insert_with(|| NarNode::Directory(BTreeMap::new()))
        {
            NarNode::Directory(entries) => insert(entries, rest, node),
            _ => Err(io::Error::other(format!(
                "{directory} is both a file and a directory of the keyed set"
            ))),
        },
    }
}

fn nar(node: &NarNode) -> io::Result<Nar> {
    node.to_nar()
        .map(|nar| Nar {
            hash: nar.hash,
            size: nar.bytes.len() as u64,
        })
        .map_err(|error| io::Error::other(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // `nix-hash --type sha256 --sri` of this tree on Lix 2.95.4: a directory, a regular file, an
    // executable and a symlink, the four things a NAR distinguishes. a different value means the
    // NAR hashed here isn't Nix's, so every copy would be entered by a hash its store refuses.
    #[test]
    fn a_directory_hashes_as_nix_hashes_it() {
        let root = tempfile::tempdir().unwrap();
        let tree = root.path().join("t");
        std::fs::create_dir_all(tree.join("a")).unwrap();
        std::fs::write(tree.join("a/b.nix"), "1").unwrap();
        std::fs::write(tree.join("x.sh"), "2").unwrap();
        std::fs::set_permissions(
            tree.join("x.sh"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        std::os::unix::fs::symlink("a/b.nix", tree.join("link")).unwrap();
        assert_eq!(
            directory(&tree).unwrap().hash.to_sri(),
            "sha256-u741gTHGMCAtq98jOB8rvwuphsRrz8Jx/Iy5LANiSYY="
        );
    }

    // a staged copy's NAR comes from the bytes in hand; it has to be the NAR of the directory the
    // staging writes from them.
    #[test]
    fn a_file_set_hashes_as_the_directory_holding_only_it() {
        let root = tempfile::tempdir().unwrap();
        let set: [(&str, bool, &[u8]); 3] = [
            ("a/b.nix", false, b"1"),
            ("a/c/d.nix", true, b"2"),
            ("e.nix", false, b"3"),
        ];
        for (file, executable, contents) in set {
            let path = root.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
            let mode = if executable { 0o755 } else { 0o644 };
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(mode))
                .unwrap();
        }
        assert_eq!(files(set).unwrap(), directory(root.path()).unwrap());
    }
}
