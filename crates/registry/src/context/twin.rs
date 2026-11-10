//! the twin of a private store: the same copies at another location, for an evaluator that reads a
//! store at its real location, as Lix does (lix 2.95.4 libstore/local-fs-store.hh:70-74 and
//! libexpr/eval.cc:566-572, 2.96.0 local-fs-store.hh:70-74 and eval.cc:563-569). what the twin is
//! for, and how evaluations use it, are in docs/explanation/architecture.md, under the context
//! cache.
//!
//! the twin is real directories holding hard links to the store's files, not links to the store:
//! every implementation refuses a store whose root, or a directory above it, is a symlink (cppnix
//! 2.35.3 libstore/local-store.cc:186-197, determinate 3.24.2 246-256, lix 2.95.4 and 2.96.0
//! 160-173), and Lix reads a symlinked store path at the copy it points to (lix 2.95.4
//! libexpr/eval.cc:503-515, 2.96.0 500-512), outside the twin, which pure mode refuses. a hard link
//! shares its file's filesystem, so the twin lies in the cache beside the store, at a path of
//! another depth and length.

use std::io;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use super::{CacheError, io_at, store_directory};

/// a private store's twin.
#[derive(Debug, Clone)]
pub(crate) struct Twin {
    root: PathBuf,
}

/// the twin's root under its directory, a level below it so the root's path differs in depth from
/// the store's as well as in length.
const LOCATED: &str = "located";

impl Twin {
    /// the twin kept in `directory`, which the cache names for the store it mirrors.
    pub(crate) fn at(directory: PathBuf) -> Self {
        Self {
            root: directory.join(LOCATED),
        }
    }

    /// the twin's root, which `local?root=` takes.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// mirrors each of `paths`, store paths whose copies lie under `store`, into the twin, unless
    /// it's there already. a copy is mirrored under a name of this process's and renamed into place,
    /// so a run that finds it there finds it whole.
    pub(crate) fn mirror(&self, store: &Path, paths: &[&str]) -> Result<(), CacheError> {
        let directory = store_directory(&self.root);
        std::fs::create_dir_all(&directory).map_err(io_at("making", &directory))?;
        for path in paths {
            let name = Path::new(path).file_name().ok_or_else(|| {
                io_at("mirroring", Path::new(path))(io::Error::other("it isn't a store path"))
            })?;
            let target = directory.join(name);
            if std::fs::symlink_metadata(&target).is_ok() {
                continue;
            }
            let staging = directory.join(super::owned(".mirror"));
            let source = store_directory(store).join(name);
            let mirrored = link_tree(&source, &staging).and_then(|()| {
                match std::fs::rename(&staging, &target).map_err(io_at("mirroring", &target)) {
                    // another run mirrored the same copy first; either is the same tree.
                    Err(_) if target.exists() => {
                        let _ = super::remove_tree(&staging);
                        Ok(())
                    }
                    other => other,
                }
            });
            if mirrored.is_err() {
                let _ = super::remove_tree(&staging);
            }
            mirrored?;
        }
        Ok(())
    }

    /// removes every mirrored copy whose original `store` doesn't hold, and what a run that died
    /// left mirroring.
    pub(crate) fn prune(&self, store: &Path) {
        let directory = store_directory(&self.root);
        super::sweep(&directory);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            // a copy still being mirrored, under a live run's own name, isn't the store's.
            if name.to_str().and_then(super::owner).is_some() {
                continue;
            }
            if std::fs::symlink_metadata(store_directory(store).join(&name)).is_err() {
                let _ = super::remove_tree(&entry.path());
            }
        }
    }

    /// removes the whole twin, with the store it mirrors.
    pub(crate) fn remove(&self) {
        if let Some(twin) = self.root.parent() {
            let _ = super::remove_tree(twin);
        }
    }
}

/// a tree at `to` whose directories are new and whose files are hard links to `from`'s, and whose
/// symlinks say what `from`'s say.
fn link_tree(from: &Path, to: &Path) -> Result<(), CacheError> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut links = Vec::new();
    walk(
        from,
        Path::new(""),
        &mut directories,
        &mut files,
        &mut links,
    )
    .map_err(io_at("reading", from))?;
    std::fs::create_dir(to).map_err(io_at("mirroring", to))?;
    for directory in &directories {
        std::fs::create_dir(to.join(directory)).map_err(io_at("mirroring", &to.join(directory)))?;
    }
    for (link, target) in &links {
        std::os::unix::fs::symlink(target, to.join(link))
            .map_err(io_at("mirroring", &to.join(link)))?;
    }
    files.par_iter().try_for_each(|file| {
        std::fs::hard_link(from.join(file), to.join(file))
            .map_err(io_at("mirroring", &to.join(file)))
    })
}

fn walk(
    root: &Path,
    relative: &Path,
    directories: &mut Vec<PathBuf>,
    files: &mut Vec<PathBuf>,
    links: &mut Vec<(PathBuf, PathBuf)>,
) -> io::Result<()> {
    for entry in std::fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            directories.push(path.clone());
            walk(root, &path, directories, files, links)?;
        } else if kind.is_symlink() {
            links.push((path.clone(), std::fs::read_link(root.join(&path))?));
        } else {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;
    use crate::layer::STORE_NAMES;

    // a keyed copy's store path and the copy of it in the store at `store`.
    fn keyed(store: &Path, hash: &str) -> (String, PathBuf) {
        let name = format!("{hash}-{}", STORE_NAMES.keyed);
        (
            format!(
                "{}/{name}",
                shikumi_derivation::StoreDir::default().as_str()
            ),
            store_directory(store).join(name),
        )
    }

    fn tree(root: &Path) {
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/c.nix"), "1").unwrap();
        std::fs::write(root.join("d.nix"), "2").unwrap();
        std::os::unix::fs::symlink("d.nix", root.join("e")).unwrap();
    }

    // the twin holds the same tree, NAR for NAR, at another location, sharing the files' inodes.
    #[test]
    fn a_mirrored_copy_is_the_same_tree_elsewhere() {
        let cache = tempfile::tempdir().unwrap();
        let store = cache.path().join("stores/0123456789abcdef");
        let (path, copy) = keyed(&store, "aaaa");
        tree(&copy);
        let twin = Twin::at(cache.path().join("twins/0123456789abcdef"));
        twin.mirror(&store, &[&path]).unwrap();
        let (_, mirrored) = keyed(twin.root(), "aaaa");
        assert_eq!(
            super::super::tree::directory(&copy).unwrap(),
            super::super::tree::directory(&mirrored).unwrap()
        );
        assert_eq!(
            std::fs::metadata(copy.join("d.nix")).unwrap().ino(),
            std::fs::metadata(mirrored.join("d.nix")).unwrap().ino()
        );
        assert_ne!(
            twin.root().to_string_lossy().len(),
            store.to_string_lossy().len()
        );
        // a second mirror of the same copy finds it there.
        twin.mirror(&store, &[&path]).unwrap();
    }

    // a copy the store has let go goes from the twin with the next maintenance; one a live run is
    // still mirroring stays.
    #[test]
    fn a_copy_the_store_lost_is_pruned() {
        let cache = tempfile::tempdir().unwrap();
        let store = cache.path().join("stores/0123456789abcdef");
        let (lost, lost_copy) = keyed(&store, "aaaa");
        let (kept, kept_copy) = keyed(&store, "bbbb");
        tree(&lost_copy);
        tree(&kept_copy);
        let twin = Twin::at(cache.path().join("twins/0123456789abcdef"));
        twin.mirror(&store, &[&lost, &kept]).unwrap();
        let mirroring = store_directory(twin.root()).join(super::super::owned(".mirror"));
        std::fs::create_dir(&mirroring).unwrap();
        super::super::remove_tree(&lost_copy).unwrap();
        twin.prune(&store);
        let mut left: Vec<_> = std::fs::read_dir(store_directory(twin.root()))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        left.sort();
        assert_eq!(left, [mirroring, keyed(twin.root(), "bbbb").1]);
    }
}
