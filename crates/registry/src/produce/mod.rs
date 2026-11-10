//! producing the registry document: evaluating the declaration layer through the host's
//! `nix-instantiate`, reading the declarations it serializes, checking them, and handing the
//! document to the shared refusal.
//!
//! every evaluation reads a store copy of exactly the files the context cache keys, in pure mode,
//! against a store of its own that holds only what it was given; the repository's lock pins the
//! nixpkgs it runs on. `session` runs one composition through the cache, `jobs` runs its
//! evaluations, and `command` builds each one's command line.
//! how the evaluator processes end with a run, and why none is tied to its parent's death, are in
//! docs/explanation/architecture.md, under the declaration layer.

mod command;
mod jobs;
mod session;

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use shikumi_lock::LockError;

#[cfg(test)]
pub(crate) use self::command::SETTINGS;
pub(crate) use self::command::Setting;
use self::session::Session;
use crate::check;
use crate::context::CacheError;
use crate::context::keyed::TrackedError;
use crate::declarations::{Declarations, Failure};
use crate::document::{Document, Identifier, Registry, RegistryError};
use crate::layer::MANIFEST;
use crate::toolchain::ToolchainContext;

/// where the producer finds what it evaluates with: the host's `nix-instantiate`, whose siblings
/// fill and collect the private store, and Shikumi's own declaration layer.
#[derive(Debug, Clone)]
pub struct Producer {
    nix_instantiate: PathBuf,
    layer: PathBuf,
}

/// the declarations, and every location they were evaluated from, which a string can carry.
#[derive(Debug)]
pub struct Produced {
    pub declarations: Declarations,
    pub roots: Vec<String>,
}

impl Producer {
    pub fn new(nix_instantiate: PathBuf, layer: PathBuf) -> Self {
        Self {
            nix_instantiate,
            layer,
        }
    }

    /// the producer an installed `shikumi` runs with. its package sets `SHIKUMI_NIX` to the
    /// declaration layer it ships, and `nix-instantiate` comes from the caller's `PATH`, since the
    /// host's evaluator is the one that reads a repository's Nix.
    pub fn from_environment() -> Result<Self, ProduceError> {
        let layer = std::env::var_os("SHIKUMI_NIX")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(ProduceError::Unset("SHIKUMI_NIX"))?;
        let path = std::env::var_os("PATH").unwrap_or_default();
        let nix_instantiate =
            find_program("nix-instantiate", &path).ok_or(ProduceError::NoEvaluator)?;
        Ok(Self::new(nix_instantiate, layer))
    }

    pub(crate) fn nix_instantiate(&self) -> &Path {
        &self.nix_instantiate
    }

    pub(crate) fn layer(&self) -> &Path {
        &self.layer
    }

    /// composes the repository at `root`, globally or focused on one project, and refuses the
    /// document unless it's accepted.
    pub fn produce(
        &self,
        root: &Path,
        focus: Option<&Identifier>,
    ) -> Result<Registry, ProduceError> {
        Registry::try_from(self.document(root, focus)?).map_err(ProduceError::Document)
    }

    /// the document the checker built, findings included, before the refusal.
    pub fn document(
        &self,
        root: &Path,
        focus: Option<&Identifier>,
    ) -> Result<Document, ProduceError> {
        self.session(root, |session, parallelism| {
            session.document(focus.map(Identifier::as_str), parallelism)
        })
    }

    /// the declarations as the declaration layer serialized them, each failure in its place, and
    /// the locations they were evaluated from.
    pub fn declarations(
        &self,
        root: &Path,
        focus: Option<&Identifier>,
    ) -> Result<Produced, ProduceError> {
        self.session(root, |session, parallelism| {
            session.declarations(focus.map(Identifier::as_str), parallelism)
        })
    }

    /// the toolchain context: every catalog entry's package as its store path, for each system the
    /// manifest names. computing a path builds nothing and reads nothing a build would write.
    pub fn context(&self, root: &Path) -> Result<ToolchainContext, ProduceError> {
        self.session(root, |session, parallelism| session.context(parallelism))
    }

    /// runs `work` in a session over the checkout at `root`, with the evaluations the process's
    /// cores allow, and ends the session however `work` came out.
    fn session<T>(
        &self,
        root: &Path,
        work: impl FnOnce(&mut Session<'_>, usize) -> Result<T, ProduceError>,
    ) -> Result<T, ProduceError> {
        let mut session = Session::open(self, root)?;
        let parallelism = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        let result = work(&mut session, parallelism);
        session.close();
        result
    }
}

fn utf8(path: &Path) -> Result<String, ProduceError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| ProduceError::NotUtf8(path.to_owned()))
}

fn find_program(name: &str, path: &OsString) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[derive(Debug)]
pub enum ProduceError {
    Unset(&'static str),
    NoEvaluator,
    Root {
        root: PathBuf,
        error: io::Error,
    },
    NoManifest(PathBuf),
    NotUtf8(PathBuf),
    Spawn {
        program: PathBuf,
        error: io::Error,
    },
    /// the layer printed something that isn't an evaluation answer: a Shikumi bug, since the layer
    /// writes nothing else.
    Answer(serde_json::Error),
    /// the layer answered the named request with something other than what it asked for: a
    /// Shikumi bug.
    Unanswered(String),
    Check(check::CheckError),
    Document(RegistryError),
    /// the files the evaluation may read couldn't be listed.
    Tracked(TrackedError),
    /// the repository's lock is missing, broken or pins no nixpkgs.
    Lock(LockError),
    /// a step on Shikumi's cache or its private store failed.
    Cache(CacheError),
    /// a file of the checkout, or of the layer, couldn't be read.
    Read {
        path: PathBuf,
        error: io::Error,
    },
    /// copying the keyed files, the layer or nixpkgs into the private store failed.
    Copy(String),
    /// the disk holding Shikumi's cache, at `cache`, has no space left for what a run writes there.
    DiskFull {
        cache: PathBuf,
    },
    /// a keyed file changed between being hashed for the key and being copied.
    Changed(String),
    /// the locked nixpkgs isn't on this machine and couldn't be fetched.
    Fetch {
        url: String,
        stderr: String,
    },
    /// the toolchain context was asked of a repository whose manifest declares no catalog, or
    /// whose manifest or catalog file doesn't evaluate.
    NoCatalog,
    /// the toolchain context for a system failed to evaluate.
    Context {
        system: String,
        failure: Box<Failure>,
    },
}

impl From<TrackedError> for ProduceError {
    fn from(error: TrackedError) -> Self {
        Self::Tracked(error)
    }
}

impl From<LockError> for ProduceError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}

impl From<CacheError> for ProduceError {
    fn from(error: CacheError) -> Self {
        Self::Cache(error)
    }
}

impl fmt::Display for ProduceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unset(name) => write!(
                f,
                "{name} is unset; it's set by the shikumi package, so run shikumi from it"
            ),
            Self::NoEvaluator => {
                f.write_str("no nix-instantiate on PATH to evaluate declarations with")
            }
            Self::Root { root, error } => write!(f, "opening {}: {error}", root.display()),
            Self::NoManifest(root) => {
                write!(f, "{} has no {} manifest", root.display(), MANIFEST.name)
            }
            Self::NotUtf8(path) => write!(f, "{} isn't valid UTF-8", path.display()),
            Self::Spawn { program, error } => write!(f, "running {}: {error}", program.display()),
            Self::Answer(error) => write!(
                f,
                "the declaration layer's output isn't an evaluation answer, which is a Shikumi bug: {error}"
            ),
            Self::Unanswered(request) => write!(
                f,
                "the declaration layer answered a {request} request with something else, which is a Shikumi bug"
            ),
            Self::Check(error) => error.fmt(f),
            Self::Document(error) => error.fmt(f),
            Self::Tracked(error) => error.fmt(f),
            Self::Lock(error) => write!(f, "the repository's lock is refused: {error}"),
            Self::Cache(error) => write!(f, "Shikumi's cache: {error}"),
            Self::Read { path, error } => write!(f, "reading {}: {error}", path.display()),
            Self::DiskFull { cache } => write!(
                f,
                "the disk holding Shikumi's cache, {}, has no space left: free some, or set XDG_CACHE_HOME to a directory on another disk",
                cache.display()
            ),
            Self::Copy(stderr) => write!(
                f,
                "copying what the evaluation reads into Shikumi's store failed, which is a Shikumi bug:\n{}",
                stderr.trim()
            ),
            Self::Changed(file) => write!(
                f,
                "{file} changed while Shikumi read it; running again reads it as it is now"
            ),
            Self::Fetch { url, stderr } => write!(
                f,
                "the locked nixpkgs isn't on this machine, and fetching it from {url} failed: {stderr}"
            ),
            Self::NoCatalog => f.write_str(
                "the toolchain context comes from the catalog, and the manifest declares none that evaluates",
            ),
            Self::Context { system, failure } => write!(
                f,
                "the toolchain context for {system} failed to evaluate: {}",
                failure.message
            ),
        }
    }
}

impl std::error::Error for ProduceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_without_a_manifest_is_refused_before_evaluating() {
        let root = tempfile::tempdir().unwrap();
        let producer = Producer::new(
            PathBuf::from("/bin/nix-instantiate"),
            PathBuf::from("/layer"),
        );
        match producer.produce(root.path(), None) {
            Err(ProduceError::NoManifest(_)) => {}
            other => panic!("expected the missing manifest, got {other:?}"),
        }
    }
}
