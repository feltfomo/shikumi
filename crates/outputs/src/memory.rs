//! a store held in memory, for testing what this crate does with a transport's answers without a
//! daemon. it keeps paths and their NARs, and counts the NAR bytes it's sent.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use shikumi_derivation::{Sha256, StoreDerivation, StorePath};
use shikumi_transport::{
    BuildTarget, GcRoot, NarReader, PathInfo, SourceNar, StoreTransport, TransportError, Trust,
};
use tokio::io::AsyncReadExt;

#[derive(Debug, Default)]
pub(crate) struct Memory {
    pub(crate) paths: Mutex<BTreeMap<StorePath, (PathInfo, Vec<u8>)>>,
    pub(crate) received: Mutex<u64>,
}

impl Memory {
    pub(crate) fn insert(&self, path: StorePath, references: BTreeSet<StorePath>, nar: Vec<u8>) {
        let info = PathInfo {
            path: path.clone(),
            nar_hash: Sha256::digest(&nar),
            nar_size: nar.len() as u64,
            references,
            deriver: None,
        };
        self.paths.lock().unwrap().insert(path, (info, nar));
    }
}

impl StoreTransport for Memory {
    async fn trust(&self) -> Result<Trust, TransportError> {
        Ok(Trust::Untrusted)
    }

    async fn add_derivations(
        &self,
        _derivations: &[&StoreDerivation],
    ) -> Result<Vec<StorePath>, TransportError> {
        unimplemented!("no test here adds derivations")
    }

    async fn add_sources(&self, sources: Vec<SourceNar>) -> Result<(), TransportError> {
        for mut source in sources {
            let mut nar = Vec::new();
            source.nar.read_to_end(&mut nar).await.unwrap();
            *self.received.lock().unwrap() += nar.len() as u64;
            assert_eq!(nar.len() as u64, source.nar_size);
            assert_eq!(&Sha256::digest(&nar), source.address.nar_hash());
            self.insert(
                source.address.path().clone(),
                source.address.references().clone(),
                nar,
            );
        }
        Ok(())
    }

    async fn build(
        &self,
        _targets: &[BuildTarget],
    ) -> Result<Vec<BTreeMap<shikumi_derivation::OutputName, StorePath>>, TransportError> {
        unimplemented!("no test here builds")
    }

    async fn query_valid(
        &self,
        paths: &[StorePath],
    ) -> Result<BTreeSet<StorePath>, TransportError> {
        let held = self.paths.lock().unwrap();
        Ok(paths
            .iter()
            .filter(|path| held.contains_key(path))
            .cloned()
            .collect())
    }

    async fn query_path_info(&self, paths: &[StorePath]) -> Result<Vec<PathInfo>, TransportError> {
        let held = self.paths.lock().unwrap();
        Ok(paths.iter().map(|path| held[path].0.clone()).collect())
    }

    fn read_nar(&self, info: &PathInfo) -> NarReader {
        let nar = self.paths.lock().unwrap()[&info.path].1.clone();
        Box::new(std::io::Cursor::new(nar))
    }

    async fn add_roots(&self, _roots: &[GcRoot]) -> Result<(), TransportError> {
        unimplemented!("no test here roots paths")
    }
}
