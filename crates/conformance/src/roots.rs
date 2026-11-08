//! the root manifest against a real collection: live paths rooted through one manifest, an
//! unrooted control added the same way in the same batch, every connection closed, then a whole
//! store collection. the control going proves the collection ran and that nothing else, such as a
//! connection's temporary roots, held what was added; the manifest's closure staying then proves
//! the root held it.

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::Path;

use shikumi_derivation::{NarNode, References, StoreDir, StorePath};
use shikumi_outputs::{add_manifest, root_manifest};
use shikumi_transport::{SourceNar, StoreTransport};

use crate::cutoff::{realise, unit};
use crate::report::Report;
use crate::{HarnessError, Salt};

const CASE: &str = "indirect-root";

/// what a run rooted, and what it left unrooted to be collected.
#[derive(Debug)]
pub(crate) struct Rooted {
    manifest: StorePath,
    live: BTreeSet<StorePath>,
    control: Vec<StorePath>,
}

/// builds and adds the live paths and the controls, then adds the manifest and roots it at
/// `link`, or with `rooting` false adds it unrooted, to show the checks fail without the root.
pub(crate) async fn prepare<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    system: &str,
    salt: &Salt,
    link: &Path,
    rooting: bool,
) -> Result<Rooted, HarnessError> {
    let built = realise(
        store,
        &unit(dir, system, salt, "rooted", "1", "echo rooted > $out", None)?,
    )
    .await?;
    let control_built = realise(
        store,
        &unit(
            dir,
            system,
            salt,
            "control",
            "1",
            "echo control > $out",
            None,
        )?,
    )
    .await?;
    let mut sources = Vec::new();
    let mut add = |name: &str, contents: &[u8]| -> Result<StorePath, HarnessError> {
        let nar = NarNode::regular(false, contents.to_vec()).to_nar()?;
        let address = dir.source_address(&salt.name(name)?, nar.hash, References::default())?;
        let path = address.path().clone();
        sources.push(SourceNar {
            address,
            nar_size: nar.bytes.len() as u64,
            nar: Box::new(Cursor::new(nar.bytes)),
        });
        Ok(path)
    };
    let source = add("rooted-source", b"rooted source\n")?;
    let control_source = add("control-source", b"control source\n")?;
    store.add_sources(sources).await?;
    let live = BTreeSet::from([built, source]);
    let manifest = if rooting {
        root_manifest(store, dir, &live, link).await?
    } else {
        add_manifest(store, dir, &live).await?
    };
    Ok(Rooted {
        manifest: manifest.path().clone(),
        live,
        control: vec![control_built, control_source],
    })
}

/// checks, after a collection, that the manifest and everything it names stayed and the controls
/// went.
pub(crate) async fn check<T: StoreTransport>(
    store: &T,
    rooted: &Rooted,
    report: &mut Report,
) -> Result<(), HarnessError> {
    let all: Vec<StorePath> = std::iter::once(&rooted.manifest)
        .chain(&rooted.live)
        .chain(&rooted.control)
        .cloned()
        .collect();
    let valid = store.query_valid(&all).await?;
    let state = |path: &StorePath| {
        if valid.contains(path) {
            "kept"
        } else {
            "collected"
        }
    };
    report.push(CASE, "manifest", "kept", state(&rooted.manifest));
    for path in &rooted.live {
        report.push(CASE, format!("live {}", path.name()), "kept", state(path));
    }
    for path in &rooted.control {
        report.push(
            CASE,
            format!("unrooted {}", path.name()),
            "collected",
            state(path),
        );
    }
    Ok(())
}

/// collects garbage over the whole store as this user, through the CLI, which is what a user who
/// isn't trusted would run. the answer to whether such a user may is recorded as a check, since
/// the root case depends on it.
pub(crate) async fn collect(report: &mut Report) -> Result<(), HarnessError> {
    let output = tokio::process::Command::new("nix-store")
        .arg("--gc")
        .output()
        .await
        .map_err(|source| HarnessError::Usage(format!("can't run 'nix-store --gc': {source}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("nix-store --gc: {}", stderr.trim_end());
    report.push(
        CASE,
        "a collection run by this untrusted user",
        "ran",
        if output.status.success() {
            "ran".to_owned()
        } else {
            format!("refused: {}", stderr.trim_end())
        },
    );
    Ok(())
}
