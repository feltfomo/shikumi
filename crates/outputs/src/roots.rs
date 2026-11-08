//! the root manifest: one content-addressed store path whose references are every live output, and
//! one indirect root on it. the garbage collector keeps a rooted path's whole closure, so this one
//! link keeps everything the manifest names, and replacing or removing the link releases it.

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::Path;

use shikumi_derivation::{NarNode, References, SourceAddress, StoreDir, StoreName, StorePath};
use shikumi_transport::{GcRoot, SourceNar, StoreTransport, TransportError};

/// adds a manifest naming `live`, which must all be valid, and roots it at `link`.
pub async fn root_manifest<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    live: &BTreeSet<StorePath>,
    link: &Path,
) -> Result<SourceAddress, TransportError> {
    let address = add_manifest(store, dir, live).await?;
    store
        .add_roots(&[GcRoot {
            link: link.to_owned(),
            path: address.path().clone(),
        }])
        .await?;
    Ok(address)
}

/// adds a manifest naming `live` without rooting it. the manifest lists the paths one per line,
/// so it holds their names as bytes as well as references, and the same set gives the same
/// manifest wherever it's made.
pub async fn add_manifest<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    live: &BTreeSet<StorePath>,
) -> Result<SourceAddress, TransportError> {
    let mut contents = String::new();
    for path in live {
        contents.push_str(&dir.display(path));
        contents.push('\n');
    }
    let nar = NarNode::regular(false, contents.into_bytes())
        .to_nar()
        .expect("a regular file always serializes");
    let address = dir
        .source_address(
            &StoreName::new("shikumi-roots").expect("a valid store name"),
            nar.hash,
            References::new(live.clone()),
        )
        .expect("a manifest's path depends on its references, so it can't be one of them");
    store
        .add_sources(vec![SourceNar {
            address: address.clone(),
            nar_size: nar.bytes.len() as u64,
            nar: Box::new(Cursor::new(nar.bytes)),
        }])
        .await?;
    Ok(address)
}
