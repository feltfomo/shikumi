//! the store transport: the one interface every daemon operation Shikumi makes goes through, with
//! two implementations. [`WorkerStore`] speaks the daemon's worker protocol over a unix socket or
//! `ssh-ng`, which is what the core uses. [`CliStore`] drives the Nix CLI of whichever
//! implementation is on the PATH, and is the conformance oracle the other is checked against.
//!
//! every operation but [`StoreTransport::read_nar`] takes a batch, because the CLI pays a process
//! per call and the worker protocol pays a round trip; a caller never loops over single paths. a
//! NAR is a stream, and both the CLI and the protocol send one path's at a time. operations are
//! async so one Tokio runtime per process can drive builds, queries and adds side by side. the
//! transport never computes a store path: paths come from `shikumi-derivation`, and what the
//! daemon reports comes back unchanged for the caller to compare.

mod cli;
mod error;
mod jobs;
mod nar;
mod worker;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;
use std::path::PathBuf;

use shikumi_derivation::{OutputName, Sha256, SourceAddress, StoreDerivation, StorePath};
use tokio::io::AsyncRead;

pub use cli::{CliStore, Family, Implementation};
pub use error::TransportError;
pub use worker::{Endpoint, WorkerStore};

/// whether the daemon treats this client as trusted. Shikumi is built to work untrusted, and the
/// conformance harness proves it runs that way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    Untrusted,
    /// the daemon didn't say, which older protocol versions don't.
    Unknown,
}

/// some outputs of a derivation already in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildTarget {
    pub derivation: StorePath,
    pub outputs: BTreeSet<OutputName>,
}

/// what the daemon records for a valid path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathInfo {
    pub path: StorePath,
    pub nar_hash: Sha256,
    pub nar_size: u64,
    pub references: BTreeSet<StorePath>,
    pub deriver: Option<StorePath>,
}

/// NAR bytes being read, from a caller's buffer, a file or another store.
pub type NarReader = Box<dyn AsyncRead + Send + Unpin>;

/// a content-addressed source to add, with a reader of the NAR bytes its address was computed
/// from. the NAR is streamed, never required whole in memory: a re-added build output can be as
/// large as anything a build produces, and the worker protocol sends it to the daemon as it reads
/// it. the daemon needs the size before the bytes, and the reader must yield exactly that many.
pub struct SourceNar {
    pub address: SourceAddress,
    pub nar_size: u64,
    pub nar: NarReader,
}

impl fmt::Debug for SourceNar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceNar")
            .field("address", &self.address)
            .field("nar_size", &self.nar_size)
            .finish_non_exhaustive()
    }
}

/// a garbage collector root: `link` is made a symlink to `path` and registered with the daemon as
/// an indirect root, so `path` and its closure stay alive while the link does. removing the link
/// lets the next collection take them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcRoot {
    pub link: PathBuf,
    pub path: StorePath,
}

pub trait StoreTransport {
    fn trust(&self) -> impl Future<Output = Result<Trust, TransportError>> + Send;

    /// writes each `.drv` and returns the path the daemon registered for it, in input order. a
    /// derivation's input derivations must be in the batch or already valid.
    fn add_derivations(
        &self,
        derivations: &[&StoreDerivation],
    ) -> impl Future<Output = Result<Vec<StorePath>, TransportError>> + Send;

    /// registers each source at its computed address, consuming its reader. the daemon recomputes
    /// the address from the content address it's sent and refuses a mismatch. references must be
    /// in the batch or already valid. a source that's already valid still has its bytes sent, so
    /// a caller that wants to skip those asks [`StoreTransport::query_valid`] first.
    fn add_sources(
        &self,
        sources: Vec<SourceNar>,
    ) -> impl Future<Output = Result<(), TransportError>> + Send;

    /// builds the outputs and returns each target's output paths as the daemon reports them, in
    /// input order.
    fn build(
        &self,
        targets: &[BuildTarget],
    ) -> impl Future<Output = Result<Vec<BTreeMap<OutputName, StorePath>>, TransportError>> + Send;

    /// the subset of `paths` that's valid.
    fn query_valid(
        &self,
        paths: &[StorePath],
    ) -> impl Future<Output = Result<BTreeSet<StorePath>, TransportError>> + Send;

    /// path info for valid paths, in input order. an invalid path is an error.
    fn query_path_info(
        &self,
        paths: &[StorePath],
    ) -> impl Future<Output = Result<Vec<PathInfo>, TransportError>> + Send;

    /// a reader of a valid path's NAR. nothing is opened until the first read, so a caller can
    /// hold a batch of these, such as the sources of one [`StoreTransport::add_sources`] call,
    /// while only the one being read holds a connection or a process. the reader fails unless it
    /// yields exactly `info.nar_size` bytes whose sha256 is `info.nar_hash`.
    fn read_nar(&self, info: &PathInfo) -> NarReader;

    /// makes each root's link and registers it as an indirect root. the link is on this machine,
    /// so a transport whose daemon is on another one refuses.
    fn add_roots(
        &self,
        roots: &[GcRoot],
    ) -> impl Future<Output = Result<(), TransportError>> + Send;
}
