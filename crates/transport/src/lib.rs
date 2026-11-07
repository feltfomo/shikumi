//! the store transport: the one interface every daemon operation Shikumi makes goes through, and
//! its implementation over the Nix CLI of whichever implementation is on the PATH.
//!
//! every operation takes a batch, because the CLI pays a process per call and the worker protocol
//! pays a round trip; a caller never loops over single paths. operations are async so one Tokio
//! runtime per process can drive builds, queries and adds side by side. the transport never
//! computes a store path: paths come from `shikumi-derivation`, and what the daemon reports comes
//! back unchanged for the caller to compare.

mod cli;
mod error;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;

use shikumi_derivation::{OutputName, Sha256, SourceAddress, StoreDerivation, StorePath};
use tokio::io::AsyncRead;

pub use cli::{CliStore, Family, Implementation};
pub use error::TransportError;

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

/// a content-addressed source to add, with a reader of the NAR bytes its address was computed
/// from. the NAR is streamed, never required whole in memory: a re-added build output can be as
/// large as anything a build produces, and the worker protocol sends it to the daemon as it reads
/// it from the daemon.
pub struct SourceNar {
    pub address: SourceAddress,
    pub nar: Box<dyn AsyncRead + Send + Unpin>,
}

impl fmt::Debug for SourceNar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceNar")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
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
    /// the address from the content address it's sent and refuses a mismatch from an untrusted
    /// client. references must be in the batch or already valid.
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
}
