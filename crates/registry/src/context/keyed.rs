//! the keyed file set: exactly the files an evaluation may read from the checkout, each with the
//! digest the cache key is built from. which files those are, why the lock enters the key as its
//! pins rather than as files, and what listing them costs are in docs/explanation/architecture.md,
//! under the context cache. what's in the set derives from `layer.rs`, the one definition of the
//! files the layer reads, so the set and what the layer reads can't drift apart.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use shikumi_lock::DIRECTORY as LOCK;

use crate::layer::{MANIFEST, NIX_EXTENSION};

/// every tracked file that could be keyed: the manifest, every `.nix` file and the lock's files,
/// relative to `root`, sorted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tracked {
    files: Vec<String>,
}

impl Tracked {
    /// lists the tracked files under `root` with the `git` on the caller's `PATH`. `root` can be a
    /// directory inside a larger repository; only what's under it is listed, relative to it.
    pub(crate) fn list(root: &Path) -> Result<Self, TrackedError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["ls-files", "-z", "--cached", "--"])
            .arg(format!(":(glob)**/*.{NIX_EXTENSION}"))
            .arg(LOCK)
            .output()
            .map_err(TrackedError::Spawn)?;
        if !output.status.success() {
            return Err(TrackedError::Git(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        Self::from_listing(&output.stdout)
    }

    /// reads `git ls-files -z` output.
    pub(crate) fn from_listing(listing: &[u8]) -> Result<Self, TrackedError> {
        let mut files = listing
            .split(|&b| b == 0)
            .filter(|file| !file.is_empty())
            .map(|file| {
                std::str::from_utf8(file)
                    .map(str::to_owned)
                    .map_err(|_| TrackedError::NotUtf8(String::from_utf8_lossy(file).into_owned()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        files.sort_unstable();
        files.dedup();
        Ok(Self { files })
    }

    pub(crate) fn has(&self, file: &str) -> bool {
        self.files
            .binary_search_by(|f| f.as_str().cmp(file))
            .is_ok()
    }

    /// the lock's tracked files.
    pub(crate) fn lock(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(String::as_str).filter(|file| {
            file.strip_prefix(LOCK)
                .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    /// every tracked file with one of `names`, wherever it is, sorted: what the plan tests for, since
    /// it looks for each listed project's declaration and the catalog by their names.
    pub(crate) fn named(&self, names: &[&str]) -> Vec<String> {
        self.files
            .iter()
            .filter(|file| {
                let name = file
                    .rsplit_once('/')
                    .map_or(file.as_str(), |(_, name)| name);
                names.contains(&name)
            })
            .cloned()
            .collect()
    }

    /// every tracked `.nix` file inside one of `directories`, sorted. a directory is a path relative
    /// to the root, as a declaration file's parent is.
    pub(crate) fn keyed<'d>(&self, directories: impl IntoIterator<Item = &'d str>) -> Vec<String> {
        let directories: BTreeSet<&str> = directories.into_iter().collect();
        let inside = |file: &str| {
            let mut at = file;
            while let Some((parent, _)) = at.rsplit_once('/') {
                if directories.contains(parent) {
                    return true;
                }
                at = parent;
            }
            false
        };
        self.files
            .iter()
            .filter(|file| {
                Path::new(file).extension() == Some(OsStr::new(NIX_EXTENSION)) && inside(file)
            })
            .cloned()
            .collect()
    }
}

/// the directory a file the layer named is in, relative to the root, as the keyed set takes it.
pub(crate) fn directory_of(file: &str) -> Option<&str> {
    file.rsplit_once('/').map(|(directory, _)| directory)
}

#[derive(Debug)]
pub enum TrackedError {
    Spawn(io::Error),
    Git(String),
    NotUtf8(String),
    /// the manifest exists but Git doesn't track it, so no evaluation may read it.
    Untracked(PathBuf),
}

impl fmt::Display for TrackedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(
                f,
                "running git to list the tracked files: {error}; Shikumi reads only what Git tracks"
            ),
            Self::Git(stderr) => write!(
                f,
                "git couldn't list the tracked files, and Shikumi reads only what Git tracks: {stderr}"
            ),
            Self::NotUtf8(file) => write!(f, "the tracked file {file} isn't named in UTF-8"),
            Self::Untracked(root) => write!(
                f,
                "{} isn't tracked by Git in {}, and Shikumi reads only what Git tracks",
                MANIFEST.name,
                root.display()
            ),
        }
    }
}

impl std::error::Error for TrackedError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(files: &[&str]) -> Tracked {
        let listing: Vec<u8> = files
            .iter()
            .flat_map(|file| file.bytes().chain([0]))
            .collect();
        Tracked::from_listing(&listing).unwrap()
    }

    #[test]
    fn keyed_files_are_the_nix_files_inside_keyed_directories_and_named_ones_anywhere() {
        let all = tracked(&[
            "shikumi.nix",
            "apps/web/project.nix",
            "apps/web/lib/helpers.nix",
            "apps/webby/project.nix",
            "apps/other.nix",
            "catalog/catalog.nix",
            "README.nix",
            "shikumi.lock/lock.json",
            "shikumi.lock/toolchain/nixpkgs.jsonl",
        ]);
        assert_eq!(
            all.keyed(["apps/web", "catalog"]),
            [
                "apps/web/lib/helpers.nix",
                "apps/web/project.nix",
                "catalog/catalog.nix",
            ]
        );
        assert!(all.keyed([]).is_empty());
        assert_eq!(
            all.named(&["project.nix", "catalog.nix"]),
            [
                "apps/web/project.nix",
                "apps/webby/project.nix",
                "catalog/catalog.nix"
            ]
        );
        let lock: Vec<&str> = all.lock().collect();
        assert_eq!(
            lock,
            [
                "shikumi.lock/lock.json",
                "shikumi.lock/toolchain/nixpkgs.jsonl"
            ]
        );
    }

    #[test]
    fn a_tracked_name_that_is_not_utf8_is_refused() {
        assert!(matches!(
            Tracked::from_listing(b"ok.nix\0bad\xff.nix\0"),
            Err(TrackedError::NotUtf8(_))
        ));
    }

    #[test]
    fn a_directory_is_a_file_parent() {
        assert_eq!(directory_of("apps/web/project.nix"), Some("apps/web"));
        assert_eq!(directory_of("project.nix"), None);
    }
}
