//! requests Shikumi never sends, kept so the conformance harness can record each daemon's answer
//! to them. they're behind the `trust-probes` feature, which only the harness turns on, so they're
//! no part of the transport anyone builds on.

use std::path::Path;

use shikumi_derivation::{StoreDerivation, StorePath};

use super::{WorkerStore, absolute, result_line};
use crate::TransportError;

const BUILD_DERIVATION: u64 = 36;
const ADD_PERM_ROOT: u64 = 47;

impl WorkerStore {
    /// asks the daemon to build a derivation it's sent rather than one in its store, which is how
    /// a remote builder is driven. Shikumi adds the `.drv` and builds it instead, because a daemon
    /// refuses this for an input-addressed derivation from an untrusted client (`BuildDerivation`
    /// in CppNix 2.35.3 `src/libstore/daemon.cc`).
    pub async fn build_derivation(&self, drv: &StoreDerivation) -> Result<(), TransportError> {
        let dir = self.dir();
        let mut connection = self.operations.acquire().await?;
        let message = &mut connection.writer.message;
        message.u64(BUILD_DERIVATION).path(dir, drv.path());
        message.u64(drv.outputs().len() as u64);
        for (name, path) in drv.outputs() {
            message.string(name.as_str()).path(dir, path);
            match drv.fixed() {
                Some(fixed) => message
                    .string(fixed.method_algo())
                    .string(&fixed.sha256.to_hex()),
                None => message.string("").string(""),
            };
        }
        message
            .paths(dir, drv.input_sources().iter())
            .string(drv.system())
            .string(drv.builder())
            .strings(drv.args().iter().map(String::as_str))
            .u64(drv.env().len() as u64);
        for (key, value) in drv.env() {
            message.string(key).string(value);
        }
        message.u64(0);
        connection.writer.flush().await?;
        connection
            .reader
            .finish_work("build a derivation it was sent")
            .await?;
        let minor = connection.minor;
        let result = connection.reader.decoder.build_result(dir, minor).await?;
        connection.release();
        if result.status.succeeded() {
            Ok(())
        } else {
            Err(self.failed(
                "build a derivation it was sent",
                vec![result_line(drv.path().base_name(), &result)],
            ))
        }
    }

    /// asks the daemon to make `link` a root of `path` itself. Shikumi makes the link and
    /// registers it as an indirect root instead, because CppNix refuses this from an untrusted
    /// client and Lix has no such operation.
    pub async fn add_perm_root(&self, path: &StorePath, link: &Path) -> Result<(), TransportError> {
        let link = absolute(link)?;
        let mut connection = self.operations.acquire().await?;
        connection
            .writer
            .message
            .u64(ADD_PERM_ROOT)
            .path(self.dir(), path)
            .string(&link.to_string_lossy());
        connection.writer.flush().await?;
        connection
            .reader
            .finish_work("add a permanent root")
            .await?;
        connection.reader.decoder.text().await?;
        connection.release();
        Ok(())
    }
}
