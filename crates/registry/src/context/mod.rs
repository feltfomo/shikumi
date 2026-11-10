//! the context cache: what each part of a composition evaluated to, kept under a key of everything
//! its evaluation could read or be changed by, and the private store those evaluations read their
//! copies from, with its roots, its locks, its temporaries and the maintenance that bounds them.
//! what a key covers, how runs share the store, where everything lives, what maintenance keeps and
//! what each step costs are in docs/explanation/architecture.md, under the context cache.
//!
//! every file here that another run could read while it's written is written under a name from
//! `owned` and renamed into place, so a reader sees the old file or the new one.

pub(crate) mod keyed;
pub(crate) mod stat;
mod state;
pub(crate) mod tree;
pub(crate) mod twin;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};
use shikumi_derivation::Sha256;
use shikumi_lock::NarHash;

use self::stat::{Seen, StatCache, nanos};
use self::tree::Nar;
use self::twin::Twin;
use crate::layer::STORE_NAMES;
use crate::produce::Setting;

/// the cache's layout version: a change to what an entry holds or how a key is built moves it to a
/// directory of its own, so an older Shikumi's entries are never read as a newer one's.
const LAYOUT: &str = "context-v2";

/// the most bytes of entries kept.
const ENTRY_BYTES: u64 = 512 * 1024 * 1024;

/// the most checkouts whose stat caches are kept.
const STAT_CACHES: usize = 256;

/// how many keyed copies a private store gathers before they're collected. a collection's cost
/// grows with the files it deletes, so collecting a few at a time spreads it over the misses that
/// made them rather than stopping one run for all of it.
const COPIES: usize = 8;

/// how long a pinned nixpkgs, or a private store, stays with no run using it.
const UNUSED: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// the directory under a user's cache directory that Shikumi's cache is in.
const NAME: &str = "shikumi";

/// the cache's own directories, under `<cache>/shikumi/<layout>`: entries, stat caches, the private
/// stores and their twins, the evaluator's empty configuration directory, its own cache directory
/// and the runs' temporaries.
const ENTRIES: &str = "entries";
const STORES: &str = "stores";
const TWINS: &str = "twins";
const STATS: &str = "stat";
const CONFIGURATION: &str = "conf";
const NIX_CACHE: &str = "nix";
const TEMPORARY: &str = "tmp";

/// where a private store keeps its copies, under the root `local?root=` takes (cppnix 2.35.3 and
/// determinate 3.24.2 src/libstore/include/nix/store/local-fs-store.hh:65-67, lix 2.95.4 and 2.96.0
/// lix/libstore/local-fs-store.hh:30-32).
const STORE_DIRECTORY: &str = "nix/store";

/// the locks a copy into the private store is made under, under its root.
const COPYING: &str = "nix/var/shikumi/copying";

/// the mark of the last run that used a private store, under its root.
const USED: &str = "used";

/// under a private store's state directory: each run's roots, the toolchain sources' pins, and the
/// lock a collection holds alone.
const RUN_ROOTS: &str = "gcroots/shikumi/run";
const PINS: &str = "gcroots/shikumi/pins";
const GC_LOCK: &str = "gc.lock";

/// the directory the copies of the store rooted at `root` lie in.
pub(crate) fn store_directory(root: &Path) -> PathBuf {
    root.join(STORE_DIRECTORY)
}

/// the evaluator the cache is keyed by. a store path names one build of one version, and a program
/// built anywhere else changes its stat when it's rebuilt; its default settings, its builtins and
/// `builtins.nixVersion` all come with the binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Identity {
    path: String,
    size: u64,
    mtime: i128,
    inode: u64,
}

impl Identity {
    /// the program at `program`, through every symlink to the file itself.
    pub(crate) fn of(program: &Path) -> io::Result<Self> {
        let path = std::fs::canonicalize(program)?;
        let metadata = std::fs::metadata(&path)?;
        Ok(Self {
            path: path.to_string_lossy().into_owned(),
            size: metadata.size(),
            mtime: nanos(metadata.mtime(), metadata.mtime_nsec()),
            inode: metadata.ino(),
        })
    }

    /// a program installed beside this one, as the same build of the same implementation.
    pub(crate) fn sibling(&self, program: &str) -> PathBuf {
        Path::new(&self.path).with_file_name(program)
    }

    /// the `nix-store` installed beside this program, which fills, registers and collects the
    /// private store.
    pub(crate) fn nix_store(&self) -> PathBuf {
        self.sibling("nix-store")
    }

    fn short(&self) -> String {
        digest_of(self).to_hex()[..16].to_owned()
    }
}

/// the part of a composition an entry holds.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Part<'a> {
    Plan,
    Projects { focus: Option<&'a str> },
    Catalog { file: &'a str, system: &'a str },
    Context { file: &'a str, system: &'a str },
}

/// everything a cached evaluation's result depends on.
#[derive(Debug, Serialize)]
pub(crate) struct Key<'a> {
    pub(crate) evaluator: &'a Identity,
    pub(crate) settings: &'a [Setting],
    #[serde(serialize_with = "hex")]
    pub(crate) layer: &'a Sha256,
    pub(crate) nixpkgs: &'a NarHash,
    #[serde(serialize_with = "files")]
    pub(crate) files: &'a [(String, Seen)],
    pub(crate) part: Part<'a>,
}

impl Key<'_> {
    pub(crate) fn digest(&self) -> Sha256 {
        digest_of(self)
    }
}

/// what a composition's checked document depends on: every part's key, the focus, and the
/// `shikumi` whose checker made it.
#[derive(Debug, Serialize)]
pub(crate) struct DocumentKey<'a> {
    pub(crate) checker: &'a Identity,
    pub(crate) focus: Option<&'a str>,
    #[serde(serialize_with = "hexes")]
    pub(crate) parts: &'a [Sha256],
}

impl DocumentKey<'_> {
    pub(crate) fn digest(&self) -> Sha256 {
        digest_of(self)
    }
}

/// a fact that holds for as long as what it's about stays the same, kept beside the entries so a
/// run asks it of a program only once.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Fact<'a> {
    /// whether the evaluator reads a store at its real location.
    ReadsAtRealLocation { evaluator: &'a Identity },
    /// the size of the NAR with this hash.
    NarSize { nar: &'a NarHash },
}

impl Fact<'_> {
    pub(crate) fn digest(&self) -> Sha256 {
        digest_of(self)
    }
}

fn digest_of(value: &impl Serialize) -> Sha256 {
    Sha256::digest(&serde_json::to_vec(value).expect("a key serializes"))
}

fn hex<S: Serializer>(digest: &&Sha256, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&digest.to_hex())
}

fn hexes<S: Serializer>(digests: &&[Sha256], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(digests.iter().map(Sha256::to_hex))
}

fn files<S: Serializer>(files: &&[(String, Seen)], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(files.iter().map(|(file, seen)| match seen {
        Seen::File { digest, executable } => (file.as_str(), Some(digest.to_hex()), *executable),
        Seen::Absent => (file.as_str(), None, false),
    }))
}

/// a path an evaluation's own store registers, with its NAR.
#[derive(Debug, Clone)]
pub(crate) struct Registered {
    pub(crate) path: String,
    pub(crate) nar: Nar,
}

/// an open cache.
#[derive(Debug)]
pub(crate) struct Cache {
    dir: PathBuf,
    store: PathBuf,
    /// the private store's state directory: its database, roots and collection lock.
    state: PathBuf,
    /// the temporary directory a store's state is moved to when its own would be too long.
    temporary: PathBuf,
}

impl Cache {
    /// opens the cache for `evaluator`, creating its directories.
    pub(crate) fn open(evaluator: &Identity) -> Result<Self, CacheError> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".cache"))
            })
            .ok_or(CacheError::NoDirectory)?;
        Self::open_in(&base, &std::env::temp_dir(), evaluator)
    }

    /// opens the cache under `base`, a user's cache directory, with `temporary` as the temporary
    /// directory.
    pub(crate) fn open_in(
        base: &Path,
        temporary: &Path,
        evaluator: &Identity,
    ) -> Result<Self, CacheError> {
        let dir = base.join(NAME).join(LAYOUT);
        let store = dir.join(STORES).join(evaluator.short());
        for directory in [ENTRIES, STATS, CONFIGURATION, NIX_CACHE, TEMPORARY] {
            create(&dir.join(directory))?;
        }
        create(&store)?;
        let store = std::fs::canonicalize(&store).unwrap_or(store);
        let state = state::of(&store, temporary).map_err(io_at("placing the state of", &store))?;
        create(&state.join(RUN_ROOTS))?;
        create(&state.join(PINS))?;
        create(&store.join(COPYING))?;
        // opening a store is using it, so no other evaluator's maintenance takes it while this run
        // fills it; a run that can't mark it only lets it age.
        let _ = std::fs::write(store.join(USED), []);
        Ok(Self {
            dir: std::fs::canonicalize(&dir).unwrap_or(dir),
            store,
            state,
            temporary: temporary.to_owned(),
        })
    }

    /// the cache's own directory, `<cache>/shikumi/<layout>`.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// the private store's state directory.
    #[cfg(test)]
    pub(crate) fn state(&self) -> &Path {
        &self.state
    }

    /// the private store's root, which `local?root=` takes.
    pub(crate) fn store(&self) -> &Path {
        &self.store
    }

    /// the private store's twin, the same copies at another location.
    pub(crate) fn twin(&self) -> Twin {
        self.twin_of(&self.store)
    }

    /// the twin of the store at `store`, one of this cache's: under the cache's twins, by the same
    /// name.
    fn twin_of(&self, store: &Path) -> Twin {
        Twin::at(
            self.dir
                .join(TWINS)
                .join(store.file_name().unwrap_or_default()),
        )
    }

    /// the private store, under its own database or, given `state`, under the one there.
    pub(crate) fn store_url(&self, state: Option<&Path>) -> OsString {
        match state {
            Some(state) => store_url(&self.store, Some(state)),
            None if self.state == self.store.join(state::STATE) => store_url(&self.store, None),
            None => {
                let mut url = store_url(&self.store, None);
                url.push("&state=");
                url.push(&self.state);
                url
            }
        }
    }

    /// a command running `program` with the evaluator's whole environment: an empty configuration
    /// directory and no user configuration files, so no `nix.conf` sets anything, and its own cache
    /// and temporary directories inside Shikumi's cache. nothing else of the caller's reaches it, so
    /// `builtins.getEnv` reads nothing.
    pub(crate) fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("NIX_CONF_DIR", self.dir.join(CONFIGURATION))
            .env("NIX_USER_CONF_FILES", "")
            .env("XDG_CACHE_HOME", self.dir.join(NIX_CACHE))
            .env("TMPDIR", self.dir.join(TEMPORARY))
            .current_dir("/");
        command
    }

    /// an empty directory of this process's own, removed when it's dropped.
    pub(crate) fn scratch(&self, kind: &str) -> Result<Scratch, CacheError> {
        let path = self.dir.join(TEMPORARY).join(owned(kind));
        std::fs::create_dir_all(&path).map_err(io_at("making", &path))?;
        Ok(Scratch(path))
    }

    /// a database of its own over the store at `root`, the private store or its twin, that
    /// registers only `registered`, which an evaluation opens the store under.
    pub(crate) fn evaluation_store(
        &self,
        root: &Path,
        nix_store: &Path,
        registered: &[Registered],
    ) -> Result<(Scratch, OsString), CacheError> {
        let scratch = self.scratch("eval")?;
        let url = store_url(root, Some(&scratch.0));
        self.load(nix_store, &url, registered)?;
        Ok((scratch, url))
    }

    /// loads `registered` into the database of the store at `url` as a database dump, which
    /// registers each path without reading it, since none is a derivation (cppnix 2.35.3
    /// src/nix/nix-store/nix-store.cc:571-615 and src/libstore/local-store.cc:730-731, determinate
    /// 3.24.2 nix-store.cc:572-616, lix 2.95.4 and 2.96.0 lix/legacy/nix-store.cc:625-664).
    fn load(
        &self,
        nix_store: &Path,
        url: &OsString,
        registered: &[Registered],
    ) -> Result<(), CacheError> {
        use std::io::Write;
        let dump: String = registered
            .iter()
            .map(|registered| {
                format!(
                    "{}\n{}\n{}\n\n0\n",
                    registered.path,
                    registered.nar.hash.to_base32(),
                    registered.nar.size
                )
            })
            .collect();
        let io = io_at("running", nix_store);
        let mut child = self
            .command(nix_store)
            .arg("--store")
            .arg(url)
            .arg("--load-db")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(io)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(dump.as_bytes())
                .map_err(io_at("running", nix_store))?;
        }
        let output = child
            .wait_with_output()
            .map_err(io_at("running", nix_store))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(CacheError::Store {
                action: "registering copies in",
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }

    /// holds, until what it returns is dropped, the lock of every store path in `paths`, so a run
    /// adding a path another run is adding waits for that copy to land, and then finds it valid.
    /// Lix checks a path's validity again after taking its own lock on it through the store's
    /// path-info cache, which holds a negative answer for `narinfo-cache-negative-ttl` once the
    /// process has queried the path and missed (lix 2.95.4 libstore/store-api.cc:629-638 and
    /// 726-745, local-store.cc:1302-1313; 2.96.0 store-api.cc:628-637 and 692-705,
    /// local-store.cc:1312-1323). then two runs adding one path at once each write it, and the
    /// second deletes the tree the first is reading. a path's lock is one of a fixed set, taken in
    /// order, so the files never gather and two runs can't each hold one the other waits for.
    pub(crate) fn copying(&self, paths: &[&str]) -> Result<Vec<File>, CacheError> {
        let stripes: BTreeSet<String> = paths
            .iter()
            .map(|path| Sha256::digest(path.as_bytes()).to_hex()[..2].to_owned())
            .collect();
        stripes
            .into_iter()
            .map(|stripe| {
                let path = self.store.join(COPYING).join(stripe);
                let lock = File::options()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(io_at("opening", &path))?;
                flock(&lock, libc::LOCK_EX).map_err(io_at("locking", &path))?;
                Ok(lock)
            })
            .collect()
    }

    /// roots `path` in the private store until the root is dropped.
    pub(crate) fn root(&self, path: &str) -> Result<Root, CacheError> {
        let link = self.state.join(RUN_ROOTS).join(owned("root"));
        // the link is made under a shared hold of the collection lock, which a collection holds
        // alone from before it reads the roots until it has deleted everything, so it either ends
        // before the link, and whatever it took is copied in again, or starts after it and keeps
        // the path. a link made while a collection runs is one that collection never reads, and a
        // copy the run then finds valid goes from under its evaluation.
        let _held = self.between_collections()?;
        std::os::unix::fs::symlink(path, &link).map_err(io_at("rooting a copy at", &link))?;
        Ok(Root(link))
    }

    /// a shared hold of the private store's collection lock, which a collection holds alone from
    /// before it reads the roots until it has deleted everything: until it's dropped, no collection
    /// runs. every implementation takes the lock with `flock`, and a copying evaluation asks it for
    /// a shared hold only, without waiting, to write its temporary root (cppnix 2.35.3
    /// libstore/gc.cc:105 and 394-395, determinate 3.24.2 gc.cc:150 and 474-475, lix 2.95.4 and
    /// 2.96.0 gc.cc:151 and 633-636), so a copy made under this hold proceeds, and only a
    /// collection waits for it.
    pub(crate) fn between_collections(&self) -> Result<HeldOff, CacheError> {
        let lock = self.gc_lock()?;
        flock(&lock, libc::LOCK_SH).map_err(io_at("locking", &self.state.join(GC_LOCK)))?;
        Ok(HeldOff { _lock: lock })
    }

    /// the private store's collection lock, opened as every implementation opens it (cppnix 2.35.3
    /// src/libstore/local-store.cc:402-406, determinate 3.24.2 452-456, lix 2.95.4
    /// lix/libstore/local-store.cc:331-336, lix 2.96.0 337-342).
    fn gc_lock(&self) -> Result<File, CacheError> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = self.state.join(GC_LOCK);
        File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(io_at("opening", &path))
    }

    /// roots `path`, a toolchain source, for `UNUSED` after the last run that pinned it, so a
    /// nixpkgs every run reads isn't copied in again after each collection, and marks the store
    /// used.
    pub(crate) fn pin(&self, path: &str) -> Result<(), CacheError> {
        let name = Path::new(path).file_name().unwrap_or_default();
        let pins = self.state.join(PINS);
        // a link of this run's own, renamed over the pin, which replaces it in one step: runs that
        // pin the same nixpkgs at once each leave the pin in place rather than racing to remove
        // and recreate it.
        let fresh = pins.join(owned(".pin"));
        std::os::unix::fs::symlink(path, &fresh).map_err(io_at("pinning at", &fresh))?;
        std::fs::rename(&fresh, pins.join(name))
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&fresh);
            })
            .map_err(io_at("pinning at", &pins.join(name)))?;
        let used = self.store.join(USED);
        std::fs::write(&used, []).map_err(io_at("marking", &used))
    }

    /// the stat cache of the checkout at `root`.
    pub(crate) fn stat(&self, root: &str) -> StatCache {
        let name = digest_of(&root).to_hex();
        StatCache::load(self.dir.join(STATS).join(format!("{name}.json")), root)
    }

    /// the entry under `key`, if there's one this Shikumi can read. a hit marks it used.
    pub(crate) fn get<T: DeserializeOwned>(&self, key: &Sha256) -> Option<T> {
        let path = self.entry(key);
        let bytes = std::fs::read(&path).ok()?;
        let value = serde_json::from_slice(&bytes).ok()?;
        if let Ok(file) = File::options().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(value)
    }

    /// keeps `value` under `key`. a failed write costs only the next run's evaluation.
    pub(crate) fn put<T: Serialize>(&self, key: &Sha256, value: &T) {
        let bytes = serde_json::to_vec(value).expect("an entry serializes");
        let _ = replace(&self.entry(key), &bytes);
    }

    fn entry(&self, key: &Sha256) -> PathBuf {
        self.dir
            .join(ENTRIES)
            .join(format!("{}.json", key.to_hex()))
    }

    /// bounds everything the cache keeps, after a run that evaluated: entries and stat caches,
    /// what a process that died left, toolchain sources and stores no run uses, and, once enough
    /// have gathered, the store's keyed copies. every step is best effort: what one leaves, the
    /// next run's maintenance takes.
    pub(crate) fn maintain(&self, nix_store: &Path) {
        evict(
            &self.dir.join(ENTRIES),
            |_, bytes| bytes > ENTRY_BYTES,
            Path::to_owned,
        );
        evict(
            &self.dir.join(STATS),
            |count, _| count > STAT_CACHES,
            stat::used,
        );
        for directory in [
            self.dir.join(TEMPORARY),
            self.dir.join(ENTRIES),
            self.dir.join(STATS),
            self.state.join(RUN_ROOTS),
            self.state.join(PINS),
            self.store.clone(),
        ] {
            sweep(&directory);
        }
        state::sweep(&self.temporary);
        self.forget_unused();
        // the lock is taken alone without waiting and let go before the collection takes it
        // itself, so a run whose shared hold comes in between makes the collection wait for that
        // run's copies, as architecture.md says.
        if self.copies() > COPIES && self.uncontended() {
            let mut command = self.command(nix_store);
            command.arg("--store").arg(self.store_url(None)).arg("--gc");
            // a collection that fails leaves the copies to the next one.
            let _ = command.output();
        }
        self.twin().prune(&self.store);
    }

    /// whether no run holds collections off: the collection lock can be taken alone at once. one
    /// that can't now is left to a later run's maintenance, rather than queued behind every copy in
    /// flight.
    fn uncontended(&self) -> bool {
        self.gc_lock()
            .is_ok_and(|lock| flock(&lock, libc::LOCK_EX | libc::LOCK_NB).is_ok())
    }

    fn copies(&self) -> usize {
        std::fs::read_dir(store_directory(&self.store)).map_or(0, |entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .ends_with(STORE_NAMES.keyed)
                })
                .count()
        })
    }

    /// drops the pin of every toolchain source, and every other evaluator's whole private store
    /// with its state and its twin, that no run has used for `UNUSED`.
    fn forget_unused(&self) {
        // a marker that can't be read isn't one judged old: a store another run has just made
        // has none until that run marks it.
        let unused = |path: &Path| {
            std::fs::symlink_metadata(path)
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|at| at.elapsed().is_ok_and(|elapsed| elapsed > UNUSED))
        };
        if let Ok(pins) = std::fs::read_dir(self.state.join(PINS)) {
            for pin in pins.filter_map(Result::ok) {
                if unused(&pin.path()) {
                    let _ = std::fs::remove_file(pin.path());
                }
            }
        }
        if let Ok(stores) = std::fs::read_dir(self.dir.join(STORES)) {
            for store in stores.filter_map(Result::ok) {
                if store.path() != self.store && unused(&store.path().join(USED)) {
                    state::forget(&store.path());
                    let _ = remove_tree(&store.path());
                    self.twin_of(&store.path()).remove();
                }
            }
        }
    }
}

/// the store at `root`, under its own database or, given `state`, under the one there.
fn store_url(root: &Path, state: Option<&Path>) -> OsString {
    let mut url = OsString::from("local?root=");
    url.push(root);
    if let Some(state) = state {
        url.push("&state=");
        url.push(state.join("state"));
        url.push("&log=");
        url.push(state.join("log"));
    }
    url
}

/// takes `operation` on `file` with `flock`, and takes it again when a signal interrupts the wait.
fn flock(file: &File, operation: libc::c_int) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    loop {
        // SAFETY: the descriptor is open for as long as `file` lives, and closing it releases the
        // lock.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

static OWNED: AtomicUsize = AtomicUsize::new(0);

/// a name unique to this process and call, `<kind>-<pid>-<n>`, which `owner` reads the process back
/// out of.
fn owned(kind: &str) -> String {
    let count = OWNED.fetch_add(1, Ordering::Relaxed);
    format!("{kind}-{}-{count}", std::process::id())
}

/// the process a name made by `owned`, or by `state`'s `<kind>-<pid>-<random>`, belongs to: its
/// next to last `-` field. a store path's name, `<hash>-<name>`, has no process there.
fn owner(name: &str) -> Option<libc::pid_t> {
    let mut fields = name.rsplitn(3, '-');
    let _last = fields.next()?;
    let pid = fields.next()?;
    fields.next()?;
    pid.parse().ok()
}

/// removes every entry of `directory` named for a process that's gone.
fn sweep(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        if name.to_str().and_then(owner).is_some_and(|pid| !alive(pid)) {
            let _ = remove_tree(&entry.path());
        }
    }
}

/// whether a process is running: signal 0 checks it exists without sending anything, and a
/// process of another user's still exists, though it refuses the signal.
fn alive(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 sends nothing; `kill` only checks the process.
    let sent = unsafe { libc::kill(pid, 0) };
    sent == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// a directory of a run's own, removed with everything in it when it's dropped.
#[derive(Debug)]
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = remove_tree(&self.0);
    }
}

/// a hold that keeps collections from starting, released when it's dropped.
#[derive(Debug)]
pub(crate) struct HeldOff {
    _lock: File,
}

/// a run's root in the private store, removed when it's dropped.
#[derive(Debug)]
pub(crate) struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// removes a tree Shikumi made, the read-only directories a store copy has included, or a file.
fn remove_tree(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
        for entry in std::fs::read_dir(path)? {
            remove_tree(&entry?.path())?;
        }
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn create(directory: &Path) -> Result<(), CacheError> {
    std::fs::create_dir_all(directory).map_err(io_at("making", directory))
}

/// an error that says what was being done, and to which path.
pub(crate) fn io_at(action: &'static str, path: &Path) -> impl FnOnce(io::Error) -> CacheError {
    let path = path.to_owned();
    move |error| CacheError::Io {
        action,
        path,
        error,
    }
}

/// removes the least recently used `.json` files of `directory`, each judged by the file `used`
/// names for it, while `over` says what's left is too much, given its count and bytes.
fn evict(directory: &Path, over: impl Fn(usize, u64) -> bool, used: impl Fn(&Path) -> PathBuf) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let at = std::fs::metadata(used(&entry.path()))
                .and_then(|used| used.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            metadata
                .is_file()
                .then(|| (at, metadata.len(), entry.path()))
        })
        .collect();
    let mut count = files.len();
    let mut bytes: u64 = files.iter().map(|(_, size, _)| size).sum();
    files.sort_unstable();
    for (_, size, path) in files {
        if !over(count, bytes) {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            let marker = used(&path);
            if marker != path {
                let _ = std::fs::remove_file(marker);
            }
            count -= 1;
            bytes -= size;
        }
    }
}

/// writes `bytes` to `path` whole: to a file of this process's beside it, then renamed over it, so a
/// reader in another process sees the old file or the new one and never part of either.
pub(crate) fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_file_name(owned(".tmp"));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

#[derive(Debug)]
pub enum CacheError {
    NoDirectory,
    Io {
        action: &'static str,
        path: PathBuf,
        error: io::Error,
    },
    /// the private store refused what Shikumi asked of it.
    Store {
        action: &'static str,
        stderr: String,
    },
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDirectory => f.write_str(
                "neither XDG_CACHE_HOME nor HOME is set, so there's nowhere to keep the store Shikumi evaluates from",
            ),
            Self::Io {
                action,
                path,
                error,
            } => write!(f, "{action} {}: {error}", path.display()),
            Self::Store { action, stderr } => write!(
                f,
                "{action} Shikumi's private store failed, which is a Shikumi bug: {stderr}"
            ),
        }
    }
}

impl std::error::Error for CacheError {}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;
    use crate::produce::SETTINGS;

    // what one key is built from, each part changeable on its own.
    struct Parts {
        evaluator: Identity,
        settings: Vec<Setting>,
        layer: Sha256,
        nixpkgs: NarHash,
        files: Vec<(String, Seen)>,
        focus: Option<&'static str>,
        system: &'static str,
    }

    fn parts() -> Parts {
        Parts {
            evaluator: Identity {
                path: "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-lix-2.95.4/bin/nix-instantiate"
                    .to_owned(),
                size: 1000,
                mtime: 1,
                inode: 7,
            },
            settings: SETTINGS.to_vec(),
            layer: Sha256::digest(b"layer"),
            nixpkgs: NarHash::new(Sha256::digest(b"nixpkgs")),
            files: vec![
                (
                    "shikumi.nix".to_owned(),
                    Seen::File {
                        digest: Sha256::digest(b"manifest"),
                        executable: false,
                    },
                ),
                ("apps/web/project.nix".to_owned(), Seen::Absent),
            ],
            focus: None,
            system: "x86_64-linux",
        }
    }

    fn key(parts: &Parts) -> Sha256 {
        Key {
            evaluator: &parts.evaluator,
            settings: &parts.settings,
            layer: &parts.layer,
            nixpkgs: &parts.nixpkgs,
            files: &parts.files,
            part: Part::Catalog {
                file: "catalog/catalog.nix",
                system: parts.system,
            },
        }
        .digest()
    }

    // each part of the key, changed alone, has to change the key, or a cache hit would hand back an
    // evaluation made under the old one.
    fn misses(change: impl FnOnce(&mut Parts)) {
        let base = parts();
        let mut changed = parts();
        change(&mut changed);
        assert_ne!(key(&base), key(&changed), "the key didn't change");
    }

    #[test]
    fn another_evaluator_misses() {
        misses(|parts| {
            parts.evaluator.path =
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-nix-2.35.3/bin/nix-instantiate"
                    .to_owned();
        });
    }

    // a program built outside the store keeps its path across a rebuild; its stat doesn't.
    #[test]
    fn a_rebuilt_evaluator_at_the_same_path_misses() {
        misses(|parts| parts.evaluator.size += 1);
        misses(|parts| parts.evaluator.mtime += 1);
        misses(|parts| parts.evaluator.inode += 1);
    }

    #[test]
    fn every_pinned_setting_is_in_the_key() {
        for index in 0..parts().settings.len() {
            misses(|parts| parts.settings[index].value = "changed");
            misses(|parts| parts.settings[index].copying = Some("changed"));
        }
    }

    #[test]
    fn another_layer_misses() {
        misses(|parts| parts.layer = Sha256::digest(b"another layer"));
    }

    #[test]
    fn another_nixpkgs_misses() {
        misses(|parts| parts.nixpkgs = NarHash::new(Sha256::digest(b"another nixpkgs")));
    }

    #[test]
    fn an_edited_file_misses() {
        misses(|parts| {
            parts.files[0].1 = Seen::File {
                digest: Sha256::digest(b"edited"),
                executable: false,
            };
        });
    }

    #[test]
    fn a_file_made_executable_misses() {
        misses(|parts| {
            parts.files[0].1 = Seen::File {
                digest: Sha256::digest(b"manifest"),
                executable: true,
            };
        });
    }

    #[test]
    fn a_file_that_appears_or_moves_misses() {
        misses(|parts| {
            parts.files[1].1 = Seen::File {
                digest: Sha256::digest(b"new"),
                executable: false,
            };
        });
        misses(|parts| parts.files[0].0 = "elsewhere.nix".to_owned());
    }

    #[test]
    fn another_part_misses() {
        misses(|parts| parts.system = "aarch64-darwin");
        let base = parts();
        let with = |part| {
            Key {
                evaluator: &base.evaluator,
                settings: &base.settings,
                layer: &base.layer,
                nixpkgs: &base.nixpkgs,
                files: &base.files,
                part,
            }
            .digest()
        };
        let all = [
            with(Part::Plan),
            with(Part::Projects { focus: base.focus }),
            with(Part::Projects { focus: Some("web") }),
            with(Part::Catalog {
                file: "catalog/catalog.nix",
                system: "x86_64-linux",
            }),
            with(Part::Context {
                file: "catalog/catalog.nix",
                system: "x86_64-linux",
            }),
        ];
        for (index, one) in all.iter().enumerate() {
            assert!(!all[index + 1..].contains(one), "part {index} shares a key");
        }
    }

    #[test]
    fn the_same_parts_hit() {
        assert_eq!(key(&parts()), key(&parts()));
    }

    // the document is the checker's work over its parts, so a part's key, the focus or another
    // `shikumi` each miss.
    #[test]
    fn the_documents_key_holds_its_parts_its_focus_and_its_checker() {
        let base = parts();
        let parts_keys = [Sha256::digest(b"plan"), Sha256::digest(b"projects")];
        let document = |checker: &Identity, focus, parts: &[Sha256]| {
            DocumentKey {
                checker,
                focus,
                parts,
            }
            .digest()
        };
        let first = document(&base.evaluator, None, &parts_keys);
        let mut other = base.evaluator.clone();
        other.inode += 1;
        assert_ne!(first, document(&other, None, &parts_keys));
        assert_ne!(first, document(&base.evaluator, Some("web"), &parts_keys));
        assert_ne!(
            first,
            document(
                &base.evaluator,
                None,
                &[parts_keys[0], Sha256::digest(b"edited")]
            )
        );
        assert_eq!(first, document(&base.evaluator, None, &parts_keys));
    }

    fn cache(base: &Path) -> Cache {
        let identity = Identity::of(Path::new("/proc/self/exe")).unwrap();
        Cache::open_in(base, base, &identity).unwrap()
    }

    // the lock a collection of the private store holds alone, taken as one would take it.
    fn collecting(opened: &Cache) -> File {
        let held = opened.gc_lock().unwrap();
        flock(&held, libc::LOCK_EX).unwrap();
        held
    }

    // a hit reads entries and nothing else, so it can't wait on a collection in another run, however
    // long that holds what a collection holds.
    #[test]
    fn a_hit_doesnt_wait_on_a_collection() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        opened.put(&Sha256::digest(b"key"), &"kept");
        let held = collecting(&opened);
        let (done, finished) = std::sync::mpsc::channel();
        let path = base.path().to_owned();
        let reading = std::thread::spawn(move || {
            let read: Option<String> = cache(&path).get(&Sha256::digest(b"key"));
            let _ = done.send(read);
        });
        let read = finished.recv_timeout(Duration::from_secs(5));
        assert_eq!(read, Ok(Some("kept".to_owned())), "the hit waited");
        drop(held);
        reading.join().unwrap();
    }

    // a collection reads the roots once, when it starts, so a root made while it runs is one it never
    // sees: a copy the run then finds already valid would be deleted under its evaluation. rooting
    // waits for a running collection, and is made at once with none running.
    #[test]
    fn a_root_isnt_made_while_a_collection_runs() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        let runs = opened.state().join(RUN_ROOTS);
        drop(opened.root("/nix/store/x").unwrap());
        let held = collecting(&opened);
        let (done, finished) = std::sync::mpsc::channel();
        let path = base.path().to_owned();
        let rooting = std::thread::spawn(move || {
            let root = cache(&path).root("/nix/store/y").unwrap();
            let _ = done.send(());
            root
        });
        assert!(
            finished.recv_timeout(Duration::from_millis(500)).is_err(),
            "the root was made while a collection held the store"
        );
        assert_eq!(std::fs::read_dir(&runs).unwrap().count(), 0);
        drop(held);
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        let root = rooting.join().unwrap();
        assert_eq!(std::fs::read_dir(&runs).unwrap().count(), 1);
        drop(root);
    }

    // a fake `nix-store` that writes down that it was asked to collect.
    fn recorder(base: &Path) -> PathBuf {
        let program = base.join("nix-store");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho \"$@\" >> {}\n",
                base.join("collected").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(
            &program,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        program
    }

    // enough keyed copies in `opened`'s store that maintenance collects.
    fn gathered(opened: &Cache) {
        for index in 0..=COPIES {
            std::fs::create_dir_all(
                store_directory(opened.store()).join(format!("{index:032}-{}", STORE_NAMES.keyed)),
            )
            .unwrap();
        }
    }

    // collection doesn't wait for every other run to finish, which with many worktrees or agents is
    // never, so copies can't gather without bound while others run: a run that only holds roots, its
    // copies landed, doesn't keep it from starting.
    #[test]
    fn collection_runs_while_other_runs_hold_roots() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        gathered(&opened);
        let _rooted = cache(base.path()).root("/nix/store/x").unwrap();
        opened.maintain(&recorder(base.path()));
        let collected = std::fs::read_to_string(base.path().join("collected")).unwrap_or_default();
        assert!(collected.contains("--gc"), "no collection: {collected:?}");
    }

    // a run holding collections off, its copies still landing, isn't queued behind: maintenance leaves
    // the copies to a later run rather than start a collection that would wait for those copies.
    #[test]
    fn no_collection_starts_while_a_run_holds_collections_off() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        gathered(&opened);
        let held = cache(base.path()).between_collections().unwrap();
        opened.maintain(&recorder(base.path()));
        let collected = std::fs::read_to_string(base.path().join("collected")).unwrap_or_default();
        assert!(
            !collected.contains("--gc"),
            "a collection started: {collected:?}"
        );
        drop(held);
        opened.maintain(&recorder(base.path()));
        let collected = std::fs::read_to_string(base.path().join("collected")).unwrap_or_default();
        assert!(
            collected.contains("--gc"),
            "no collection once the hold ended"
        );
    }

    // two runs adding one path take turns: the second's copy starts once the first has landed, so it
    // finds the path valid rather than writing it again under the first's readers. runs adding
    // different paths don't wait on each other.
    #[test]
    fn copies_of_one_path_take_turns() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        let path = format!(
            "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-{}",
            STORE_NAMES.keyed
        );
        let held = opened.copying(&[&path]).unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        let at = base.path().to_owned();
        let same = path.clone();
        let waiting = std::thread::spawn(move || {
            let locks = cache(&at).copying(&[&same]).unwrap();
            let _ = done.send(());
            locks
        });
        assert!(
            finished.recv_timeout(Duration::from_millis(500)).is_err(),
            "a second copy of the path started while the first held it"
        );
        let other = (0..)
            .map(|n| format!("/nix/store/{n:032}-{}", STORE_NAMES.keyed))
            .find(|other| {
                Sha256::digest(other.as_bytes()).to_hex()[..2]
                    != Sha256::digest(path.as_bytes()).to_hex()[..2]
            })
            .unwrap();
        drop(opened.copying(&[&other]).unwrap());
        drop(held);
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(waiting.join().unwrap());
    }

    // another evaluator's store that a run has just made, before anything marks it used, is that run's
    // to fill, not one that's gone unused.
    #[test]
    fn a_store_just_made_isnt_taken_as_unused() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        let other = base
            .path()
            .join(NAME)
            .join(LAYOUT)
            .join(STORES)
            .join("0123456789abcdef");
        std::fs::create_dir_all(store_directory(&other)).unwrap();
        opened.maintain(&recorder(base.path()));
        assert!(other.exists());
    }

    // a run that died leaves its staging, its stores' databases, its roots and whatever it was writing
    // to rename into place, each named for its process; the next maintenance takes them, and leaves a
    // live run's.
    #[test]
    fn what_a_dead_run_left_is_swept() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        let dir = base.path().join(NAME).join(LAYOUT);
        let tmp = dir.join(TEMPORARY);
        let runs = opened.state().join(RUN_ROOTS);
        let dead = i32::MAX;
        std::fs::create_dir_all(tmp.join(format!("stage-{dead}-0/sub"))).unwrap();
        std::os::unix::fs::symlink("/nix/store/x", runs.join(format!("root-{dead}-0"))).unwrap();
        let half_written = [
            dir.join(ENTRIES).join(format!(".tmp-{dead}-0")),
            dir.join(STATS).join(format!(".tmp-{dead}-1")),
            opened.state().join(PINS).join(format!(".pin-{dead}-2")),
            opened.store().join(format!(".state-dir-{dead}-3")),
        ];
        for file in &half_written {
            std::fs::write(file, "half").unwrap();
        }
        let live = opened.scratch("stage").unwrap();
        let rooted = opened.root("/nix/store/y").unwrap();
        opened.put(&Sha256::digest(b"key"), &"kept");
        opened.maintain(&recorder(base.path()));
        assert!(!tmp.join(format!("stage-{dead}-0")).exists());
        assert!(std::fs::symlink_metadata(runs.join(format!("root-{dead}-0"))).is_err());
        for file in &half_written {
            assert!(!file.exists(), "{} was left", file.display());
        }
        assert!(live.path().exists());
        assert_eq!(
            opened.get::<String>(&Sha256::digest(b"key")).as_deref(),
            Some("kept")
        );
        assert_eq!(std::fs::read_dir(&runs).unwrap().count(), 1);
        drop(rooted);
        assert_eq!(std::fs::read_dir(&runs).unwrap().count(), 0);
    }

    // a name of a process's own reads back as that process, and a store path's name, or a pin's,
    // reads as no process's, so no sweep takes it.
    #[test]
    fn only_a_name_made_for_a_process_reads_as_one() {
        assert_eq!(
            owner(&owned(".tmp")),
            Some(std::process::id() as libc::pid_t)
        );
        assert_eq!(
            owner(&owned(".state-dir")),
            Some(std::process::id() as libc::pid_t)
        );
        assert_eq!(owner("shikumi-1234-aB3dEf"), Some(1234));
        for name in [
            "0123456789abcdfghijklmnpqrsvwxyz-source",
            &format!("0123456789abcdfghijklmnpqrsvwxyz-{}", STORE_NAMES.keyed),
            "0a1b2c.json",
            USED,
        ] {
            assert_eq!(owner(name), None, "{name}");
        }
    }

    // a private store of another evaluator that no run has used for long is removed whole, its
    // read-only copies included; a used one stays.
    #[test]
    fn an_unused_store_is_removed() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        let stores = base.path().join(NAME).join(LAYOUT).join(STORES);
        for (name, age) in [("old", UNUSED * 2), ("recent", Duration::ZERO)] {
            let copy = store_directory(&stores.join(name)).join("x-source");
            std::fs::create_dir_all(&copy).unwrap();
            std::fs::write(stores.join(name).join(USED), []).unwrap();
            File::options()
                .write(true)
                .open(stores.join(name).join(USED))
                .unwrap()
                .set_modified(SystemTime::now() - age)
                .unwrap();
            std::fs::set_permissions(&copy, std::os::unix::fs::PermissionsExt::from_mode(0o555))
                .unwrap();
        }
        opened.maintain(&recorder(base.path()));
        assert!(!stores.join("old").exists());
        assert!(stores.join("recent").exists());
        assert!(opened.store().exists());
    }

    // a stat cache whose checkout is read on every run, and never changes, is the one to keep; one
    // read long ago goes first, though its file is newer.
    #[test]
    fn eviction_keeps_the_stat_cache_read_last() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        let stat = base.path().join(NAME).join(LAYOUT).join(STATS);
        let mut paths = Vec::new();
        for index in 0..=STAT_CACHES {
            let path = stat.join(format!("{index}.json"));
            std::fs::write(&path, "{}").unwrap();
            std::fs::write(stat::used(&path), []).unwrap();
            File::options()
                .write(true)
                .open(stat::used(&path))
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(60))
                .unwrap();
            paths.push(path);
        }
        // the first is old on disk but read just now, as an unchanged checkout's is on every run.
        File::options()
            .write(true)
            .open(&paths[0])
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        std::fs::write(stat::used(&paths[0]), []).unwrap();
        opened.maintain(&recorder(base.path()));
        assert!(paths[0].exists(), "the cache read last was evicted");
        let kept = std::fs::read_dir(&stat)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "json")
            })
            .count();
        assert_eq!(kept, STAT_CACHES);
    }

    // every run pins the nixpkgs it reads, and runs side by side pin the same one at once; none of
    // them may fail for another having just pinned it.
    #[test]
    fn runs_pinning_one_nixpkgs_at_once_all_succeed() {
        let base = tempfile::tempdir().unwrap();
        let opened = cache(base.path());
        std::thread::scope(|threads| {
            for _ in 0..16 {
                threads.spawn(|| {
                    for _ in 0..200 {
                        opened
                            .pin("/nix/store/0123456789abcdfghijklmnpqrsvwxyz-source")
                            .unwrap();
                    }
                });
            }
        });
        let pins: Vec<_> = std::fs::read_dir(opened.state().join(PINS))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(pins, ["0123456789abcdfghijklmnpqrsvwxyz-source"]);
    }
}
