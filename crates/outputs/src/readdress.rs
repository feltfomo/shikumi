//! re-addressing: a built output's bytes added again at the content-addressed path those bytes
//! give, so a dependent that names that path is unchanged whenever the bytes are, whatever built
//! them. that's early cutoff without content-addressed derivations, which Lix doesn't have.
//!
//! the address is computed here, from the NAR hash and references the daemon records for the
//! output, with the same code that computes every other path. bytes move only for an address
//! that isn't valid yet, which is exactly when the output changed and its dependents rebuild
//! anyway; an unchanged output costs one path-info and one validity query for the whole batch.

use std::collections::HashSet;
use std::fmt;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};

use shikumi_derivation::{References, SelfReference, SourceAddress, StoreDir, StorePath};
use shikumi_transport::{NarReader, PathInfo, SourceNar, StoreTransport, TransportError};
use tokio::io::{AsyncRead, ReadBuf};

/// one output, re-addressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readdressed {
    pub output: StorePath,
    pub address: SourceAddress,
    /// the NAR bytes read from the output and sent to the daemon: zero when the address was
    /// already valid, which is the cutoff the caller is after.
    pub sent: u64,
}

#[derive(Debug)]
pub enum ReaddressError {
    Transport(TransportError),
    /// outputs that refer to their own store path, as absolute paths. none of the batch is added.
    SelfReference(Vec<String>),
}

impl fmt::Display for ReaddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => error.fmt(f),
            Self::SelfReference(outputs) => write!(
                f,
                "can't re-address {}: a content-addressed path can't refer to itself, so a unit whose output does has to be a leaf that isn't re-addressed",
                outputs
                    .iter()
                    .map(|output| format!("'{output}', which refers to its own store path"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

impl std::error::Error for ReaddressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::SelfReference(_) => None,
        }
    }
}

impl From<TransportError> for ReaddressError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

/// the content-addressed path a built output's bytes give: its own name, the NAR hash the daemon
/// records, and its references other than itself. an output that refers to itself has none.
pub fn output_address(dir: &StoreDir, info: &PathInfo) -> Result<SourceAddress, SelfReference> {
    dir.source_address(
        &info.path.store_name(),
        info.nar_hash,
        References::of(&info.path, info.references.iter().cloned()),
    )
}

/// re-addresses each output, in input order. the outputs must be valid. an output whose
/// references include itself fails the whole batch before anything is added, naming every such
/// output.
///
/// cost is linear in the batch: one path-info query and one validity query over all of it, then
/// one add carrying only the NARs of addresses that weren't valid, each streamed from the daemon
/// back to it as it's read.
pub async fn readdress<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    outputs: &[StorePath],
) -> Result<Vec<Readdressed>, ReaddressError> {
    let infos = store.query_path_info(outputs).await?;
    let mut addresses = Vec::with_capacity(infos.len());
    let mut refusing = Vec::new();
    for info in &infos {
        match output_address(dir, info) {
            Ok(address) => addresses.push(address),
            Err(_) => refusing.push(dir.display(&info.path)),
        }
    }
    if !refusing.is_empty() {
        return Err(ReaddressError::SelfReference(refusing));
    }
    let paths: Vec<StorePath> = addresses
        .iter()
        .map(|address| address.path().clone())
        .collect();
    let valid = store.query_valid(&paths).await?;
    let counters: Vec<Arc<AtomicU64>> = infos.iter().map(|_| Arc::default()).collect();
    // two outputs with the same bytes and name share an address; its bytes go once.
    let mut sending = HashSet::new();
    let sources: Vec<SourceNar> = infos
        .iter()
        .zip(&addresses)
        .zip(&counters)
        .filter(|((_, address), _)| {
            !valid.contains(address.path()) && sending.insert(address.path().clone())
        })
        .map(|((info, address), counter)| SourceNar {
            address: address.clone(),
            nar_size: info.nar_size,
            nar: Box::new(Counted {
                inner: store.read_nar(info),
                count: counter.clone(),
            }),
        })
        .collect();
    store.add_sources(sources).await?;
    Ok(outputs
        .iter()
        .zip(addresses)
        .zip(counters)
        .map(|((output, address), counter)| Readdressed {
            output: output.clone(),
            address,
            sent: counter.load(Ordering::Relaxed),
        })
        .collect())
}

/// counts what a reader yields, so what a re-add sent is measured where it's read rather than
/// inferred from what the caller expected to happen.
struct Counted {
    inner: NarReader,
    count: Arc<AtomicU64>,
}

impl AsyncRead for Counted {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        ready!(Pin::new(&mut this.inner).poll_read(cx, buf))?;
        this.count
            .fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Memory;
    use shikumi_derivation::{NarNode, Sha256, StoreName};
    use std::collections::BTreeSet;

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn output(name: &str) -> StorePath {
        let digest = Sha256::digest(name.as_bytes()).to_base32();
        StorePath::from_base_name(&format!("{}-{name}", &digest[..32])).unwrap()
    }

    fn nar(contents: &[u8]) -> Vec<u8> {
        NarNode::regular(false, contents.to_vec())
            .to_nar()
            .unwrap()
            .bytes
    }

    #[test]
    fn a_new_output_is_added_at_the_address_its_bytes_give() {
        let dir = StoreDir::default();
        let store = Memory::default();
        let built = output("a");
        let bytes = nar(b"a");
        store.insert(built.clone(), BTreeSet::new(), bytes.clone());
        let readdressed = block_on(readdress(&store, &dir, std::slice::from_ref(&built))).unwrap();
        let expected = dir
            .source_address(
                &StoreName::new("a").unwrap(),
                Sha256::digest(&bytes),
                References::default(),
            )
            .unwrap();
        assert_eq!(readdressed[0].address, expected);
        assert_eq!(readdressed[0].sent, bytes.len() as u64);
        assert!(store.paths.lock().unwrap().contains_key(expected.path()));
    }

    // the cutoff itself: a rebuilt output with the same bytes under another input-addressed path
    // lands on the address already there, and nothing is sent for it.
    #[test]
    fn an_output_whose_bytes_are_already_addressed_sends_nothing() {
        let dir = StoreDir::default();
        let store = Memory::default();
        let (first, second) = (output("a"), output("a-rebuilt"));
        store.insert(first.clone(), BTreeSet::new(), nar(b"same"));
        let earlier = block_on(readdress(&store, &dir, &[first])).unwrap();
        // a rebuild's path differs, but its name is the unit's, so give it the same name.
        let rebuilt =
            StorePath::from_base_name(&format!("{}-a", &second.base_name()[..32])).unwrap();
        store.insert(rebuilt.clone(), BTreeSet::new(), nar(b"same"));
        let received = *store.received.lock().unwrap();
        let later = block_on(readdress(&store, &dir, &[rebuilt])).unwrap();
        assert_eq!(later[0].address, earlier[0].address);
        assert_eq!(later[0].sent, 0);
        assert_eq!(*store.received.lock().unwrap(), received);
    }

    #[test]
    fn references_to_other_paths_carry_over_into_the_address() {
        let dir = StoreDir::default();
        let store = Memory::default();
        let dependency = output("dep");
        store.insert(dependency.clone(), BTreeSet::new(), nar(b"dep"));
        let built = output("user");
        store.insert(
            built.clone(),
            BTreeSet::from([dependency.clone()]),
            nar(b"user"),
        );
        let readdressed = block_on(readdress(&store, &dir, &[built])).unwrap();
        assert_eq!(
            readdressed[0].address.references(),
            &BTreeSet::from([dependency])
        );
    }

    #[test]
    fn an_output_that_refers_to_itself_fails_the_batch_naming_it_and_adds_nothing() {
        let dir = StoreDir::default();
        let store = Memory::default();
        let (plain, selfish) = (output("plain"), output("selfish"));
        store.insert(plain.clone(), BTreeSet::new(), nar(b"plain"));
        store.insert(
            selfish.clone(),
            BTreeSet::from([selfish.clone()]),
            nar(b"selfish"),
        );
        let held = store.paths.lock().unwrap().len();
        match block_on(readdress(&store, &dir, &[plain, selfish.clone()])) {
            Err(ReaddressError::SelfReference(outputs)) => {
                assert_eq!(outputs, vec![dir.display(&selfish)]);
                let message = ReaddressError::SelfReference(outputs).to_string();
                assert!(message.contains(&dir.display(&selfish)), "{message}");
            }
            other => panic!("expected the self-reference to be refused, got {other:?}"),
        }
        assert_eq!(store.paths.lock().unwrap().len(), held);
    }

    #[test]
    fn two_outputs_with_one_address_send_their_bytes_once() {
        let dir = StoreDir::default();
        let store = Memory::default();
        let first = output("twin");
        let second =
            StorePath::from_base_name(&format!("{}-twin", &output("other").base_name()[..32]))
                .unwrap();
        store.insert(first.clone(), BTreeSet::new(), nar(b"twin"));
        store.insert(second.clone(), BTreeSet::new(), nar(b"twin"));
        let readdressed = block_on(readdress(&store, &dir, &[first, second])).unwrap();
        assert_eq!(readdressed[0].address, readdressed[1].address);
        assert_eq!(*store.received.lock().unwrap(), nar(b"twin").len() as u64);
    }
}
