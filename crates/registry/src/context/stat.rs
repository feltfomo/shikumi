//! the stat cache: each keyed file's digest, kept beside what `lstat` said of the file when it was
//! hashed, so a later run rehashes only the files whose stat changed. the rule that decides which
//! entries to trust, Git's racily clean rule against the cache file's own mtime, and what a hit
//! costs, are in docs/explanation/architecture.md, under the context cache.
//!
//! one cache per checkout, since inode numbers and times belong to one checkout's files. an entry is
//! kept for as long as Git tracks its file, whichever command last saw it, so one that reads part
//! of the keyed set leaves the rest for the next. loading a cache marks it used, beside it, since an
//! unchanged cache is never rewritten and its own mtime is the racy rule's reference.

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use shikumi_derivation::Sha256;

/// a time `lstat` gives in seconds and nanoseconds, as nanoseconds.
pub(crate) fn nanos(seconds: i64, nanos: i64) -> i128 {
    i128::from(seconds) * 1_000_000_000 + i128::from(nanos)
}

/// what `lstat` says of a file that a change to it has to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stat {
    size: u64,
    mtime: i128,
    ctime: i128,
    inode: u64,
    device: u64,
    mode: u32,
}

impl Stat {
    fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            size: metadata.size(),
            mtime: nanos(metadata.mtime(), metadata.mtime_nsec()),
            ctime: nanos(metadata.ctime(), metadata.ctime_nsec()),
            inode: metadata.ino(),
            device: metadata.dev(),
            mode: metadata.mode(),
        }
    }

    /// whether the file could have changed in the same timestamp tick the cache was written in.
    fn racy(&self, written: Option<i128>) -> bool {
        written.is_none_or(|written| self.mtime >= written || self.ctime >= written)
    }
}

/// a keyed file as the evaluation will see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seen {
    /// a regular file, its digest, and whether it's executable, which its store copy keeps.
    File { digest: Sha256, executable: bool },
    /// not a regular file: missing, a symlink or a directory. none of them is copied.
    Absent,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    stat: Stat,
    digest: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    /// the checkout the entries are for, so a cache of a checkout that's gone can be told apart.
    root: String,
    files: HashMap<String, Entry>,
}

/// a checkout's stat cache, loaded from `path`.
#[derive(Debug)]
pub(crate) struct StatCache {
    path: PathBuf,
    /// the cache file's mtime when it was loaded, against which every entry is judged racy.
    written: Option<i128>,
    cached: Written,
}

impl StatCache {
    /// the cache at `path` for the checkout at `root`. a cache that's missing, unreadable or for
    /// another root starts empty: it only ever saves work.
    pub(crate) fn load(path: PathBuf, root: &str) -> Self {
        let _ = std::fs::write(used(&path), []);
        let read = std::fs::read(&path).ok().and_then(|bytes| {
            let written = std::fs::metadata(&path).ok().map(|m| Stat::of(&m).mtime);
            serde_json::from_slice::<Written>(&bytes)
                .ok()
                .map(|cached| (written, cached))
        });
        match read {
            Some((written, cached)) if cached.root == root => Self {
                path,
                written,
                cached,
            },
            _ => Self {
                path,
                written: None,
                cached: Written {
                    root: root.to_owned(),
                    files: HashMap::new(),
                },
            },
        }
    }

    /// how each of `files`, relative to `root`, is seen, in the order given, and whether any entry
    /// changed. the files are stat'ed and, where needed, hashed on Rayon's pool.
    pub(crate) fn see(&self, root: &Path, files: &[String]) -> io::Result<(Vec<Seen>, Fresh)> {
        let behind = behind_symlinks(root, files);
        let seen: Vec<io::Result<(Seen, Option<Entry>)>> = files
            .par_iter()
            .map(|file| {
                if ancestors(file).any(|directory| behind.contains(directory)) {
                    Ok((Seen::Absent, None))
                } else {
                    self.see_one(&root.join(file), file)
                }
            })
            .collect();
        let mut out = Vec::with_capacity(files.len());
        let mut fresh = Fresh(Vec::new());
        for (file, result) in files.iter().zip(seen) {
            let (seen, entry) = result?;
            out.push(seen);
            if let Some(entry) = entry {
                fresh.0.push((file.clone(), entry));
            }
        }
        Ok((out, fresh))
    }

    fn see_one(&self, path: &Path, file: &str) -> io::Result<(Seen, Option<Entry>)> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((Seen::Absent, None));
            }
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_file() {
            return Ok((Seen::Absent, None));
        }
        let stat = Stat::of(&metadata);
        let executable = stat.mode & 0o100 != 0;
        if let Some(entry) = self.cached.files.get(file)
            && entry.stat == stat
            && !stat.racy(self.written)
            && let Ok(digest) = Sha256::parse_hex(&entry.digest)
        {
            return Ok((Seen::File { digest, executable }, None));
        }
        let digest = Sha256::digest(&std::fs::read(path)?);
        let entry = Entry {
            stat,
            digest: digest.to_hex(),
        };
        Ok((Seen::File { digest, executable }, Some(entry)))
    }

    /// keeps the entry of every file `tracked` says Git still tracks, with the fresh ones in
    /// place, and writes the cache when anything changed. a failed write costs only the next run's
    /// hashing, so it isn't an error.
    pub(crate) fn save(mut self, tracked: impl Fn(&str) -> bool, fresh: Fresh) {
        let before = self.cached.files.len();
        let changed = !fresh.0.is_empty();
        self.cached.files.retain(|file, _| tracked(file));
        let dropped = self.cached.files.len() != before;
        self.cached.files.extend(fresh.0);
        // an entry that was racy when loaded is still racy until the cache is written again, so
        // a cache holding one is rewritten too.
        let racy = self
            .cached
            .files
            .values()
            .any(|entry| entry.stat.racy(self.written));
        if changed || dropped || racy {
            let bytes = serde_json::to_vec(&self.cached).expect("the stat cache serializes");
            let _ = super::replace(&self.path, &bytes);
        }
    }
}

/// every directory, relative to `root`, between it and `files` that isn't a real directory: a
/// symlink, or anything else. a file reached through one is outside what Git tracks, however its
/// index entry names it, so it's absent. each directory is checked once, on Rayon's pool.
fn behind_symlinks<'f>(root: &Path, files: &'f [String]) -> HashSet<&'f str> {
    let directories: HashSet<&str> = files.iter().flat_map(|file| ancestors(file)).collect();
    directories
        .into_par_iter()
        .filter(|directory| {
            std::fs::symlink_metadata(root.join(directory))
                .map_or(true, |metadata| !metadata.file_type().is_dir())
        })
        .collect()
}

/// the directories a relative path is inside, nearest last: `a/b/c.nix` is inside `a` and `a/b`.
fn ancestors(file: &str) -> impl Iterator<Item = &str> {
    file.match_indices('/').map(|(at, _)| &file[..at])
}

/// the entries a run hashed afresh, for `save`.
#[derive(Debug, Default)]
pub(crate) struct Fresh(Vec<(String, Entry)>);

impl Fresh {
    pub(crate) fn extend(&mut self, more: Fresh) {
        self.0.extend(more.0);
    }
}

/// the marker beside a stat cache that its last load touches, which eviction goes by.
pub(crate) fn used(path: &Path) -> PathBuf {
    path.with_extension("used")
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    use super::*;

    fn set_times(path: &Path, at: SystemTime) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    fn digest_of(seen: Seen) -> Sha256 {
        match seen {
            Seen::File { digest, .. } => digest,
            Seen::Absent => panic!("a file the test wrote was seen as absent"),
        }
    }

    // a cache written after the files were last changed trusts their stat and reads nothing.
    #[test]
    fn a_hit_hashes_nothing_and_an_edit_is_hashed_again() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let files = vec!["a.nix".to_owned(), "b.nix".to_owned()];
        std::fs::write(root.path().join("a.nix"), "1").unwrap();
        std::fs::write(root.path().join("b.nix"), "2").unwrap();
        let past = SystemTime::now() - Duration::from_secs(60);
        for file in &files {
            set_times(&root.path().join(file), past);
        }
        let path = cache.path().join("stat.json");
        let load = || StatCache::load(path.clone(), "/root");
        let (first, fresh) = load().see(root.path(), &files).unwrap();
        assert_eq!(fresh.0.len(), 2);
        load().save(|_| true, fresh);
        // a cache written after the files last changed: their ctime is now, set by the test, and
        // a cache written in the same clock tick would rightly count them as racy.
        set_times(&path, SystemTime::now() + Duration::from_secs(1));

        let (again, fresh) = load().see(root.path(), &files).unwrap();
        assert_eq!(again, first);
        assert!(fresh.0.is_empty(), "a hit rehashed {:?}", fresh.0);

        std::fs::write(root.path().join("b.nix"), "3").unwrap();
        let (edited, fresh) = load().see(root.path(), &files).unwrap();
        assert_eq!(fresh.0.len(), 1);
        assert_eq!(edited[0], first[0]);
        assert_ne!(edited[1], first[1]);
    }

    // a cache that holds a file's current stat beside a digest of other content, which is what a
    // same-size write in the tick the cache was written leaves behind: every stat field matches. a
    // test can't make a real write keep ctime, so it writes that cache itself.
    fn stale(root: &Path, cache: &Path) -> (Vec<String>, PathBuf, Sha256) {
        let files = vec!["a.nix".to_owned()];
        std::fs::write(root.join("a.nix"), "bbbb").unwrap();
        let stat = Stat::of(&std::fs::symlink_metadata(root.join("a.nix")).unwrap());
        let old = Sha256::digest(b"aaaa");
        let written = Written {
            root: "/root".to_owned(),
            files: HashMap::from([(
                "a.nix".to_owned(),
                Entry {
                    stat,
                    digest: old.to_hex(),
                },
            )]),
        };
        let path = cache.join("stat.json");
        std::fs::write(&path, serde_json::to_vec(&written).unwrap()).unwrap();
        (files, path, old)
    }

    fn cache_written_at(path: &Path, file: &Path, offset: i64) {
        let mtime = std::fs::metadata(file).unwrap().modified().unwrap();
        let at = if offset >= 0 {
            mtime + Duration::from_secs(offset.unsigned_abs())
        } else {
            mtime - Duration::from_secs(offset.unsigned_abs())
        };
        set_times(path, at);
    }

    // written in the same tick as the file's last change, the entry is racily clean and its file is
    // hashed again, so the edit is seen.
    #[test]
    fn an_edit_that_keeps_the_stat_is_caught_while_it_is_racy() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let (files, path, old) = stale(root.path(), cache.path());
        cache_written_at(&path, &root.path().join("a.nix"), 0);
        let (seen, _) = StatCache::load(path, "/root")
            .see(root.path(), &files)
            .unwrap();
        assert_ne!(digest_of(seen[0]), old, "the edit hid behind its stat");
    }

    // the control: the same entry in a cache written well after the file changed is trusted, so the
    // racy rule is what caught the edit above, not a stat that differed.
    #[test]
    fn an_entry_older_than_its_cache_is_trusted() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let (files, path, old) = stale(root.path(), cache.path());
        cache_written_at(&path, &root.path().join("a.nix"), 1);
        let (seen, fresh) = StatCache::load(path, "/root")
            .see(root.path(), &files)
            .unwrap();
        assert_eq!(digest_of(seen[0]), old);
        assert!(fresh.0.is_empty());
    }

    // a file whose index entry Git still holds, in a directory since replaced by a symlink to one
    // outside the repository, would otherwise be read from wherever the symlink points.
    #[test]
    fn a_file_behind_a_symlinked_directory_is_absent() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("docs")).unwrap();
        std::fs::write(outside.path().join("x.nix"), "\"outside\"").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("docs/sub")).unwrap();
        std::fs::create_dir_all(root.path().join("app/real")).unwrap();
        std::fs::write(root.path().join("app/real/y.nix"), "1").unwrap();
        let files: Vec<String> = ["docs/sub/x.nix", "app/real/y.nix"]
            .map(str::to_owned)
            .into();
        let (seen, _) = StatCache::load(cache.path().join("s"), "/r")
            .see(root.path(), &files)
            .unwrap();
        assert_eq!(seen[0], Seen::Absent);
        assert!(matches!(seen[1], Seen::File { .. }));
    }

    #[test]
    fn a_symlink_a_directory_and_a_missing_file_are_absent() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("real.nix"), "x").unwrap();
        std::os::unix::fs::symlink("real.nix", root.path().join("link.nix")).unwrap();
        std::fs::create_dir(root.path().join("dir.nix")).unwrap();
        let files: Vec<String> = ["link.nix", "dir.nix", "gone.nix", "real.nix"]
            .map(str::to_owned)
            .into();
        let (seen, _) = StatCache::load(cache.path().join("s"), "/r")
            .see(root.path(), &files)
            .unwrap();
        assert_eq!(seen[..3], [Seen::Absent, Seen::Absent, Seen::Absent]);
        assert!(matches!(seen[3], Seen::File { .. }));
    }

    #[test]
    fn a_cache_for_another_checkout_is_not_read() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let files = vec!["a.nix".to_owned()];
        std::fs::write(root.path().join("a.nix"), "1").unwrap();
        set_times(
            &root.path().join("a.nix"),
            SystemTime::now() - Duration::from_secs(60),
        );
        let path = cache.path().join("stat.json");
        let (_, fresh) = StatCache::load(path.clone(), "/one")
            .see(root.path(), &files)
            .unwrap();
        StatCache::load(path.clone(), "/one").save(|_| true, fresh);
        let (_, fresh) = StatCache::load(path, "/two")
            .see(root.path(), &files)
            .unwrap();
        assert_eq!(fresh.0.len(), 1);
    }

    // a command that reads only part of the keyed set leaves the other files' entries for the next
    // command, which would otherwise hash them all again.
    #[test]
    fn a_run_that_reads_part_of_the_set_keeps_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let all: Vec<String> = ["plan.nix", "project.nix"].map(str::to_owned).into();
        for file in &all {
            std::fs::write(root.path().join(file), file).unwrap();
            set_times(
                &root.path().join(file),
                SystemTime::now() - Duration::from_secs(60),
            );
        }
        let path = cache.path().join("stat.json");
        let load = || StatCache::load(path.clone(), "/root");
        let (_, fresh) = load().see(root.path(), &all).unwrap();
        load().save(|_| true, fresh);
        let (_, fresh) = load().see(root.path(), &all[..1]).unwrap();
        let tracked = |file: &str| all.iter().any(|kept| kept == file);
        load().save(tracked, fresh);
        set_times(&path, SystemTime::now() + Duration::from_secs(1));
        let (_, fresh) = load().see(root.path(), &all).unwrap();
        assert!(fresh.0.is_empty(), "rehashed {:?}", fresh.0);
        // a file Git no longer tracks loses its entry.
        load().save(|file| file == "plan.nix", Fresh::default());
        set_times(&path, SystemTime::now() + Duration::from_secs(2));
        let (_, fresh) = load().see(root.path(), &all).unwrap();
        assert_eq!(fresh.0.len(), 1);
    }
}
