//! the store transport over the Nix CLI. it's the conformance oracle and the fallback that works
//! wherever a Nix CLI does, so it uses only commands all four supported implementations share and
//! runs as whatever user starts it.
//!
//! - `.drv` files go through `nix derivation add`, in the implementation's JSON dialect, one
//!   process per derivation since the command reads one. independent derivations are added at
//!   once.
//! - sources go through a local binary cache and `nix copy --no-recursive --from`.
//! - builds, validity and path info use `nix-store`, whose output is the same on every
//!   implementation, with many paths per invocation.
//! - a NAR is read from `nix-store --dump`, and a root is made by `nix-store --realise
//!   --add-root`, which makes the link and registers it with the daemon as an indirect root on
//!   every implementation (`IndirectRootStore::addPermRoot` in CppNix 2.35.3
//!   `src/libstore/indirect-root-store.cc`, and in Lix 2.95.4 and 2.96.0 `lix/libstore/gc.cc`).

mod cache;
mod derivation_json;
mod implementation;
mod process;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use shikumi_derivation::{OutputName, Sha256, StoreDerivation, StoreDir, StorePath};

use self::implementation::Dialect;
use self::process::{batches, describe, run, stream};
use crate::jobs::{run_all, run_graph};
use crate::nar::{Lazy, Verified};
use crate::{
    BuildTarget, GcRoot, NarReader, PathInfo, SourceNar, StoreTransport, TransportError, Trust,
};

pub use self::implementation::{Family, Implementation};

/// every `nix` invocation turns the command on: CppNix and Lix keep it experimental, and naming an
/// enabled feature again is harmless on Determinate Nix.
const NIX_COMMAND: [&str; 2] = ["--extra-experimental-features", "nix-command"];

#[derive(Debug)]
pub struct CliStore {
    store_dir: StoreDir,
    implementation: Implementation,
    dialect: Dialect,
    /// where throwaway binary caches are made. it must be readable by this user only.
    work_dir: PathBuf,
    parallelism: usize,
}

impl CliStore {
    /// detects the implementation on the PATH. `store_dir` must be the store its CLI is
    /// configured for, since every path is computed against it.
    pub async fn connect(store_dir: StoreDir, work_dir: &Path) -> Result<Self, TransportError> {
        let stdout = run("nix", &["--version".to_owned()], None).await?;
        let text = String::from_utf8_lossy(&stdout);
        let implementation = Implementation::parse(&text).ok_or_else(|| {
            TransportError::Unsupported(format!(
                "'nix --version' printed {:?}, which isn't a CppNix, Determinate Nix or Lix version this transport can read",
                text.trim()
            ))
        })?;
        let dialect = implementation.dialect().ok_or_else(|| {
            TransportError::Unsupported(format!(
                "{} {} isn't supported: the transport needs CppNix 2.33 or later, Determinate Nix 3.24 or later, or Lix",
                implementation.family, implementation.version
            ))
        })?;
        let work_dir = std::path::absolute(work_dir).map_err(|source| TransportError::Io {
            path: work_dir.to_owned(),
            source,
        })?;
        Ok(Self {
            store_dir,
            implementation,
            dialect,
            work_dir,
            parallelism: std::thread::available_parallelism().map_or(1, NonZeroUsize::get),
        })
    }

    pub fn implementation(&self) -> &Implementation {
        &self.implementation
    }

    pub fn store_dir(&self) -> &StoreDir {
        &self.store_dir
    }

    fn nix_args<'a>(&self, args: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        NIX_COMMAND
            .into_iter()
            .chain(args)
            .map(str::to_owned)
            .collect()
    }

    fn parse_path(&self, command: &str, text: &str) -> Result<StorePath, TransportError> {
        self.store_dir
            .parse(text)
            .map_err(|source| TransportError::output(command, source))
    }

    /// runs `nix-store <flags> <paths>` in batches, several at once, and returns each batch's
    /// stdout in order.
    async fn nix_store_batches(
        &self,
        flags: &[&str],
        paths: &[StorePath],
    ) -> Result<Vec<String>, TransportError> {
        let batches = batches(paths.iter().collect(), |path: &&StorePath| {
            self.store_dir.as_str().len() + 1 + path.base_name().len()
        });
        let jobs = batches
            .iter()
            .map(|batch| {
                let args: Vec<String> = flags
                    .iter()
                    .map(|&flag| flag.to_owned())
                    .chain(batch.iter().map(|path| self.store_dir.display(path)))
                    .collect();
                async move { run("nix-store", &args, None).await }
            })
            .collect();
        Ok(run_all(self.parallelism, jobs)
            .await?
            .into_iter()
            .map(|stdout| String::from_utf8_lossy(&stdout).into_owned())
            .collect())
    }
}

impl StoreTransport for CliStore {
    async fn trust(&self) -> Result<Trust, TransportError> {
        let args = self.nix_args(["store", "ping", "--json"]);
        let command = describe("nix", &args);
        let stdout = run("nix", &args, None).await?;
        let value: serde_json::Value = serde_json::from_slice(&stdout)
            .map_err(|error| TransportError::output(&command, error))?;
        // CppNix writes a boolean, and older protocol versions leave the field out.
        match value.get("trusted") {
            None | Some(serde_json::Value::Null) => Ok(Trust::Unknown),
            Some(serde_json::Value::Bool(true)) => Ok(Trust::Trusted),
            Some(serde_json::Value::Bool(false)) => Ok(Trust::Untrusted),
            Some(serde_json::Value::Number(n)) if n.as_u64() == Some(1) => Ok(Trust::Trusted),
            Some(serde_json::Value::Number(n)) if n.as_u64() == Some(0) => Ok(Trust::Untrusted),
            Some(other) => Err(TransportError::output(
                &command,
                format!("'trusted' is {other}"),
            )),
        }
    }

    async fn add_derivations(
        &self,
        derivations: &[&StoreDerivation],
    ) -> Result<Vec<StorePath>, TransportError> {
        // `nix derivation add` needs a derivation's inputs registered first, so each waits for its
        // inputs in the batch and nothing else. a derivation can only name inputs synthesized
        // before it, so the batch holds no cycle.
        let index: HashMap<&StorePath, usize> = derivations
            .iter()
            .enumerate()
            .map(|(i, drv)| (drv.path(), i))
            .collect();
        let waits_for: Vec<Vec<usize>> = derivations
            .iter()
            .map(|drv| {
                drv.input_derivations()
                    .filter_map(|(path, _)| index.get(path).copied())
                    .collect()
            })
            .collect();
        let args = self.nix_args(["derivation", "add"]);
        let outputs = run_graph(self.parallelism, &waits_for, |i| {
            let json = derivation_json::render(self.dialect, &self.store_dir, derivations[i]);
            let args = args.clone();
            async move { run("nix", &args, Some(json)).await }
        })
        .await?;
        let command = describe("nix", &args);
        outputs
            .iter()
            .map(|stdout| self.parse_path(&command, String::from_utf8_lossy(stdout).trim()))
            .collect()
    }

    async fn add_sources(&self, sources: Vec<SourceNar>) -> Result<(), TransportError> {
        if sources.is_empty() {
            return Ok(());
        }
        let root = tempfile::Builder::new()
            .prefix("shikumi-cache-")
            .tempdir_in(&self.work_dir)
            .map_err(|source| TransportError::Io {
                path: self.work_dir.clone(),
                source,
            })?;
        let paths = cache::write(root.path(), &self.store_dir, sources, self.parallelism).await?;
        let from = format!("file://{}", root.path().display());
        // sources that refer to each other have to arrive in order, so the batches run one after
        // another; `nix copy` orders the paths inside one batch by their references itself.
        for batch in batches(paths, String::len) {
            let mut args = self.nix_args(["copy", "--no-recursive", "--from", &from]);
            args.extend(batch);
            run("nix", &args, None).await?;
        }
        let path = root.path().to_owned();
        tokio::task::spawn_blocking(move || root.close())
            .await
            .expect("removing a directory doesn't panic")
            .map_err(|source| TransportError::Io { path, source })
    }

    async fn build(
        &self,
        targets: &[BuildTarget],
    ) -> Result<Vec<BTreeMap<OutputName, StorePath>>, TransportError> {
        // `drv!output` makes `nix-store --realise` print exactly one line per argument, in
        // argument order, on every implementation, which is what lets the lines be matched back.
        let wanted: Vec<(usize, &OutputName, String)> = targets
            .iter()
            .enumerate()
            .flat_map(|(index, target)| {
                target.outputs.iter().map(move |output| {
                    let arg = format!("{}!{output}", self.store_dir.display(&target.derivation));
                    (index, output, arg)
                })
            })
            .collect();
        let batches = batches(wanted, |(_, _, arg)| arg.len());
        let jobs = batches
            .iter()
            .map(|batch| {
                let args: Vec<String> = std::iter::once("--realise".to_owned())
                    .chain(batch.iter().map(|(_, _, arg)| arg.clone()))
                    .collect();
                async move { run("nix-store", &args, None).await }
            })
            .collect();
        let outputs = run_all(self.parallelism, jobs).await?;
        let mut built: Vec<BTreeMap<OutputName, StorePath>> = vec![BTreeMap::new(); targets.len()];
        for (batch, stdout) in batches.into_iter().zip(outputs) {
            let command = "nix-store --realise";
            let text = String::from_utf8_lossy(&stdout);
            let lines: Vec<&str> = text.lines().collect();
            if lines.len() != batch.len() {
                return Err(TransportError::output(
                    command,
                    format!("{} paths for {} outputs", lines.len(), batch.len()),
                ));
            }
            for ((index, output, _), line) in batch.into_iter().zip(lines) {
                built[index].insert(output.clone(), self.parse_path(command, line)?);
            }
        }
        Ok(built)
    }

    async fn query_valid(
        &self,
        paths: &[StorePath],
    ) -> Result<BTreeSet<StorePath>, TransportError> {
        let command = "nix-store --check-validity --print-invalid";
        let mut valid: BTreeSet<StorePath> = paths.iter().cloned().collect();
        for stdout in self
            .nix_store_batches(&["--check-validity", "--print-invalid"], paths)
            .await?
        {
            for line in stdout.lines() {
                valid.remove(&self.parse_path(command, line)?);
            }
        }
        Ok(valid)
    }

    async fn query_path_info(&self, paths: &[StorePath]) -> Result<Vec<PathInfo>, TransportError> {
        let mut found: HashMap<StorePath, PathInfo> = HashMap::with_capacity(paths.len());
        for stdout in self.nix_store_batches(&["--dump-db"], paths).await? {
            for info in self.parse_registrations(&stdout)? {
                found.insert(info.path.clone(), info);
            }
        }
        paths
            .iter()
            .map(|path| {
                found.get(path).cloned().ok_or_else(|| {
                    TransportError::output("nix-store --dump-db", format!("no entry for {path}"))
                })
            })
            .collect()
    }

    fn read_nar(&self, info: &PathInfo) -> NarReader {
        let path = self.store_dir.display(&info.path);
        let args = vec!["--dump".to_owned(), path.clone()];
        let opening = async move {
            stream("nix-store", &args).map(|streamed| Box::new(streamed) as NarReader)
        };
        Box::new(Verified::new(Lazy::new(Box::pin(opening)), path, info))
    }

    async fn add_roots(&self, roots: &[GcRoot]) -> Result<(), TransportError> {
        // one root per invocation: given several paths, `--add-root` numbers the links itself.
        let jobs = roots
            .iter()
            .map(|root| {
                let args = vec![
                    "--realise".to_owned(),
                    self.store_dir.display(&root.path),
                    "--add-root".to_owned(),
                    root.link.to_string_lossy().into_owned(),
                ];
                async move { run("nix-store", &args, None).await.map(drop) }
            })
            .collect();
        run_all(self.parallelism, jobs).await.map(drop)
    }
}

impl CliStore {
    /// reads `nix-store --dump-db` output: per path, its absolute path, the NAR hash in hex, the
    /// NAR size, the deriver or an empty line, the reference count and the references. the format
    /// is `makeValidityRegistration` in CppNix 2.35.3 `src/libstore/store-api.cc`, unchanged in
    /// Lix 2.95.4.
    fn parse_registrations(&self, text: &str) -> Result<Vec<PathInfo>, TransportError> {
        let command = "nix-store --dump-db";
        let truncated = || TransportError::output(command, "a registration ends early");
        let mut lines = text.lines();
        let mut out = Vec::new();
        while let Some(path) = lines.next() {
            let path = self.parse_path(command, path)?;
            let nar_hash = Sha256::parse_hex(lines.next().ok_or_else(truncated)?)
                .map_err(|source| TransportError::output(command, source))?;
            let nar_size = lines
                .next()
                .ok_or_else(truncated)?
                .parse()
                .map_err(|error| TransportError::output(command, error))?;
            let deriver = match lines.next().ok_or_else(truncated)? {
                "" => None,
                deriver => Some(self.parse_path(command, deriver)?),
            };
            let count: usize = lines
                .next()
                .ok_or_else(truncated)?
                .parse()
                .map_err(|error| TransportError::output(command, error))?;
            let references = (0..count)
                .map(|_| self.parse_path(command, lines.next().ok_or_else(truncated)?))
                .collect::<Result<_, _>>()?;
            out.push(PathInfo {
                path,
                nar_hash,
                nar_size,
                references,
                deriver,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> CliStore {
        CliStore {
            store_dir: StoreDir::default(),
            implementation: Implementation::parse("nix (Nix) 2.35.3").unwrap(),
            dialect: Dialect::V4,
            work_dir: PathBuf::from("/tmp"),
            parallelism: 1,
        }
    }

    #[test]
    fn dump_db_registrations_parse_with_and_without_derivers() {
        let a = "/nix/store/0mdqa9w1p6cmli6976v4wi0sw9r4p5pr-a";
        let b = "/nix/store/1mdqa9w1p6cmli6976v4wi0sw9r4p5pr-b";
        let drv = "/nix/store/2mdqa9w1p6cmli6976v4wi0sw9r4p5pr-b.drv";
        let hash = Sha256::digest(b"").to_hex();
        let text = format!("{a}\n{hash}\n120\n\n0\n{b}\n{hash}\n8\n{drv}\n2\n{a}\n{b}\n");
        let infos = store().parse_registrations(&text).unwrap();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].nar_size, 120);
        assert_eq!(infos[0].deriver, None);
        assert!(infos[0].references.is_empty());
        assert_eq!(infos[1].deriver.as_ref().unwrap().name(), "b.drv");
        assert_eq!(infos[1].references.len(), 2);
    }

    #[test]
    fn a_truncated_registration_is_an_error() {
        let a = "/nix/store/0mdqa9w1p6cmli6976v4wi0sw9r4p5pr-a";
        let text = format!("{a}\n{}\n120\n\n2\n{a}\n", Sha256::digest(b"").to_hex());
        assert!(matches!(
            store().parse_registrations(&text),
            Err(TransportError::Output { .. })
        ));
    }
}
