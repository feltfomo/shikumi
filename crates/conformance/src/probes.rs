//! what a daemon does with operations Shikumi doesn't rely on, recorded as notes, and what it does
//! when a client walks away from a build, checked.

use std::path::Path;
use std::time::Duration;

use shikumi_derivation::{OutputName, StoreDir};
use shikumi_transport::{BuildTarget, StoreTransport, WorkerStore};

use crate::cutoff::unit;
use crate::report::Report;
use crate::{HarnessError, Salt};

/// how long a builder gets to appear, which includes the daemon setting up its sandbox.
const START: Duration = Duration::from_secs(120);
/// how long a builder gets to stop once its client has hung up.
const STOP: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(100);

/// the daemon's answers to `BuildDerivation` of an input-addressed derivation, sent whole rather
/// than added, both before and after the `.drv` is in the store, and to `AddPermRoot`.
pub(crate) async fn trust(
    store: &WorkerStore,
    dir: &StoreDir,
    system: &str,
    salt: &Salt,
    work_dir: &Path,
    report: &mut Report,
) -> Result<(), HarnessError> {
    const CASE: &str = "trust-probe";
    let answer = |result: Result<(), shikumi_transport::TransportError>| match result {
        Ok(()) => "accepted".to_owned(),
        Err(error) => format!("refused: {}", first_line(&error.to_string())),
    };
    let drv = unit(dir, system, salt, "probe", "1", "echo probe > $out", None)?;
    report.note(
        CASE,
        "BuildDerivation of an input-addressed derivation not in the store",
        answer(store.build_derivation(&drv).await),
    );
    store.add_derivations(&[&drv]).await?;
    report.note(
        CASE,
        "BuildDerivation of an input-addressed derivation in the store",
        answer(store.build_derivation(&drv).await),
    );
    report.note(
        CASE,
        "AddPermRoot",
        answer(
            store
                .add_perm_root(
                    drv.path(),
                    &work_dir.join(format!("perm-root-{}", salt.as_str())),
                )
                .await,
        ),
    );
    Ok(())
}

fn first_line(text: &str) -> &str {
    // the error's first line ends with the daemon's own message; the rest is its log.
    text.lines().next().unwrap_or(text)
}

/// a build whose future is dropped while its builder runs. the builder has to keep running while
/// the build is live, and stop once it's dropped, which it does only if the transport hangs up on
/// the daemon.
pub(crate) async fn cancel<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    system: &str,
    salt: &Salt,
    report: &mut Report,
) -> Result<(), HarnessError> {
    const CASE: &str = "cancelled-build";
    // the sandbox has a shell and little else, so the builder spins in it; the marker is in its
    // command line, which is how its processes are found in `/proc`.
    let marker = format!("shikumi-cancel-{}", salt.as_str());
    let script = format!("while :; do :; done # {marker}");
    let drv = unit(dir, system, salt, "cancel", "1", &script, None)?;
    store.add_derivations(&[&drv]).await?;
    let target = [BuildTarget {
        derivation: drv.path().clone(),
        outputs: [OutputName::new("out")?].into(),
    }];
    let (builders, alive) = {
        let build = store.build(&target);
        tokio::pin!(build);
        let builders = tokio::select! {
            finished = &mut build => {
                report.push(CASE, "builder", "running", format!("build finished: {finished:?}"));
                return Ok(());
            }
            builders = appear(&marker) => builders?,
        };
        // a builder that dies by itself would make the drop prove nothing.
        tokio::select! {
            finished = &mut build => {
                report.push(CASE, "builder while its build is live", "running", format!("build finished: {finished:?}"));
                return Ok(());
            }
            () = tokio::time::sleep(LIVE) => {}
        }
        let alive = !builders.is_empty() && live(&marker, &builders).await? == builders;
        (builders, alive)
        // leaving this block drops the build, as cancelling a command does.
    };
    report.push(
        CASE,
        "builder",
        "running",
        if builders.is_empty() {
            "never seen"
        } else {
            "running"
        },
    );
    report.push(
        CASE,
        "builder while its build is live",
        "running",
        if alive { "running" } else { "stopped" },
    );
    let stopped = !builders.is_empty() && gone(&marker, &builders).await?;
    report.push(
        CASE,
        "builder after its build is dropped",
        "stopped",
        if stopped { "stopped" } else { "still running" },
    );
    Ok(())
}

/// how long a builder has to stay up while its build is live.
const LIVE: Duration = Duration::from_secs(2);

/// waits up to `START` for processes holding `marker` and returns them, or none.
async fn appear(marker: &str) -> Result<Vec<u32>, HarnessError> {
    let deadline = tokio::time::Instant::now() + START;
    loop {
        let found = scan(marker).await?;
        if !found.is_empty() || tokio::time::Instant::now() >= deadline {
            return Ok(found);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// waits up to `STOP` until none of `builders` still holds `marker`, seen on two scans in a row
/// so one that races a process starting or a slow `/proc` can't pass.
async fn gone(marker: &str, builders: &[u32]) -> Result<bool, HarnessError> {
    let deadline = tokio::time::Instant::now() + STOP;
    let mut misses = 0;
    loop {
        if live(marker, builders).await?.is_empty() {
            misses += 1;
            if misses == 2 {
                return Ok(true);
            }
        } else {
            misses = 0;
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// those of `builders` still running with `marker`, so a reused process id doesn't count.
async fn live(marker: &str, builders: &[u32]) -> Result<Vec<u32>, HarnessError> {
    let running = scan(marker).await?;
    Ok(builders
        .iter()
        .copied()
        .filter(|pid| running.contains(pid))
        .collect())
}

/// the processes whose command line holds `marker`. a builder runs as a build user in its own
/// namespaces, but its process is still listed in this machine's `/proc`. an unreadable `/proc`
/// is an error: it would otherwise read as every builder having stopped.
async fn scan(marker: &str) -> Result<Vec<u32>, HarnessError> {
    let marker = marker.to_owned();
    tokio::task::spawn_blocking(move || {
        let entries = std::fs::read_dir("/proc")
            .map_err(|error| HarnessError::Usage(format!("can't list /proc: {error}")))?;
        let mut found: Vec<u32> = entries
            .flatten()
            .filter_map(|entry| {
                let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
                let cmdline = std::fs::read(entry.path().join("cmdline")).ok()?;
                String::from_utf8_lossy(&cmdline)
                    .contains(&marker)
                    .then_some(pid)
            })
            .collect();
        found.sort_unstable();
        Ok(found)
    })
    .await
    .expect("scanning /proc doesn't panic")
}
