//! a throwaway local binary cache holding content-addressed sources, which `nix copy --from`
//! sends to the daemon.
//!
//! no CLI command adds a NAR with references: `nix store add` and `nix-store --add` take none. a
//! binary cache's `.narinfo` carries them along with a `CA` field, and the daemon accepts a path
//! from an untrusted client only when that content address reproduces the path
//! (`ValidPathInfo::isContentAddressed` in CppNix 2.35.3 `src/libstore/path-info.cc`, checked from
//! `wopAddToStoreNar` in `src/libstore/daemon.cc`). so the daemon re-derives every path sent this
//! way, and a wrong one is refused rather than registered.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use shikumi_derivation::{SourceAddress, StoreDir};
use tokio::io::AsyncWriteExt;

use crate::jobs::run_all;
use crate::{SourceNar, TransportError};

/// writes the cache into `root`, an empty directory, and returns the absolute paths it holds, in
/// input order. each NAR is streamed from its reader into its own file, with at most `limit`
/// copies in flight, so memory holds copy buffers rather than NARs.
pub(crate) async fn write(
    root: &Path,
    dir: &StoreDir,
    sources: Vec<SourceNar>,
    limit: usize,
) -> Result<Vec<String>, TransportError> {
    write_file(
        root.join("nix-cache-info"),
        format!("StoreDir: {}\n", dir.as_str()),
    )
    .await?;
    let nar_dir = root.join("nar");
    tokio::fs::create_dir(&nar_dir)
        .await
        .map_err(|source| TransportError::Io {
            path: nar_dir.clone(),
            source,
        })?;
    let jobs = sources
        .into_iter()
        .map(|source| {
            let root = root.to_owned();
            let dir = dir.clone();
            async move { write_source(&root, &dir, source).await }
        })
        .collect();
    run_all(limit, jobs).await
}

async fn write_source(
    root: &Path,
    dir: &StoreDir,
    source: SourceNar,
) -> Result<String, TransportError> {
    let SourceNar {
        address,
        nar_size,
        mut nar,
    } = source;
    // one file per path, named by its hash part, so sources never collide.
    let nar_name = format!("{}.nar", address.path().hash_part());
    let nar_path = root.join("nar").join(&nar_name);
    let io = |source| TransportError::Io {
        path: nar_path.clone(),
        source,
    };
    let mut file = tokio::fs::File::create(&nar_path).await.map_err(io)?;
    let size = tokio::io::copy(&mut nar, &mut file).await.map_err(io)?;
    if size != nar_size {
        return Err(TransportError::Nar {
            path: dir.display(address.path()),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("is {size} bytes, not its declared {nar_size}"),
            ),
        });
    }
    // tokio's file finishes its last write in the background; `nix copy` must see all of it.
    file.flush().await.map_err(io)?;
    let info_path = root.join(format!("{}.narinfo", address.path().hash_part()));
    write_file(info_path, narinfo(dir, &address, &nar_name, size)).await?;
    Ok(dir.display(address.path()))
}

/// the `.narinfo` for a NAR stored uncompressed, so the file's hash and size are the NAR's. a wrong
/// NAR hash isn't caught here: the daemon hashes what it receives and refuses a mismatch.
fn narinfo(dir: &StoreDir, address: &SourceAddress, nar_name: &str, size: u64) -> String {
    let base32 = address.nar_hash().to_base32();
    let mut info = String::new();
    writeln!(info, "StorePath: {}", dir.display(address.path())).unwrap();
    writeln!(info, "URL: nar/{nar_name}").unwrap();
    writeln!(info, "Compression: none").unwrap();
    writeln!(info, "FileHash: sha256:{base32}").unwrap();
    writeln!(info, "FileSize: {size}").unwrap();
    writeln!(info, "NarHash: sha256:{base32}").unwrap();
    writeln!(info, "NarSize: {size}").unwrap();
    if !address.references().is_empty() {
        info.push_str("References:");
        for reference in address.references() {
            info.push(' ');
            info.push_str(reference.base_name());
        }
        info.push('\n');
    }
    writeln!(info, "CA: {}", address.content_address()).unwrap();
    info
}

async fn write_file(path: PathBuf, text: String) -> Result<(), TransportError> {
    tokio::fs::write(&path, text)
        .await
        .map_err(|source| TransportError::Io { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shikumi_derivation::{NarNode, References, StoreName};

    #[test]
    fn a_narinfo_carries_references_the_content_address_and_the_streamed_size() {
        let dir = StoreDir::default();
        let nar = NarNode::regular(false, b"a".to_vec()).to_nar().unwrap();
        let base = dir
            .source_address(
                &StoreName::new("a").unwrap(),
                nar.hash,
                References::default(),
            )
            .unwrap();
        let referring = dir
            .source_address(
                &StoreName::new("b").unwrap(),
                nar.hash,
                References::new([base.path().clone()].into()),
            )
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let sources = [&base, &referring]
            .into_iter()
            .map(|address| SourceNar {
                address: address.clone(),
                nar_size: nar.bytes.len() as u64,
                nar: Box::new(std::io::Cursor::new(nar.bytes.clone())),
            })
            .collect();
        let paths = runtime
            .block_on(write(root.path(), &dir, sources, 2))
            .unwrap();
        assert_eq!(
            paths,
            vec![dir.display(base.path()), dir.display(referring.path())]
        );
        let info = std::fs::read_to_string(
            root.path()
                .join(format!("{}.narinfo", referring.path().hash_part())),
        )
        .unwrap();
        let base32 = nar.hash.to_base32();
        assert!(info.contains(&format!("References: {}\n", base.path().base_name())));
        assert!(info.contains(&format!("CA: fixed:r:sha256:{base32}\n")));
        assert!(info.contains(&format!("NarSize: {}\n", nar.bytes.len())));
        let stored = std::fs::read(
            root.path()
                .join("nar")
                .join(format!("{}.nar", referring.path().hash_part())),
        )
        .unwrap();
        assert_eq!(stored, nar.bytes);
    }
}
