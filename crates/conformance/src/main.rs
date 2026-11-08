//! `shikumi-conformance` checks the store paths Shikumi computes, and what it does with them,
//! against a live daemon through one store transport per run: the Nix CLI on the PATH, the
//! daemon's socket, or `ssh-ng`. it's meant to run as an untrusted user, which it checks, so the
//! daemon re-derives every content address it's sent rather than taking it on trust.
//!
//! with `--collect-garbage` it ends by collecting garbage over the whole store, so it belongs on a
//! throwaway store such as a conformance VM's. it prints one line per check and exits 0 when every
//! check passes, 1 when one fails and 2 when the run itself can't finish.

mod cases;
mod cutoff;
mod probes;
mod report;
mod roots;

use std::fmt;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use shikumi_derivation::{
    DerivationError, NarError, PathError, SelfReference, StoreDir, StoreName,
};
use shikumi_outputs::ReaddressError;
use shikumi_transport::{CliStore, Endpoint, StoreTransport, TransportError, WorkerStore};

use crate::report::Report;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Transport {
    Cli,
    Socket,
    Ssh,
}

impl Transport {
    fn name(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Socket => "socket",
            Self::Ssh => "ssh",
        }
    }
}

/// a step a run can leave out, to show the checks that rest on it fail without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Step {
    Readdressing,
    Root,
}

#[derive(Debug, Parser)]
#[command(name = "shikumi-conformance")]
struct Args {
    /// a directory this user can write, for throwaway binary caches and root links.
    #[arg(long)]
    work_dir: PathBuf,
    /// the implementation the run must find, as `<family> <version>`, such as `Lix 2.95.4`.
    #[arg(long)]
    expect: Option<String>,
    /// alters every value computed for this case, to show the harness fails when a computation
    /// drifts.
    #[arg(long)]
    perturb: Option<String>,
    /// the transport every check goes through.
    #[arg(long, value_enum, default_value_t = Transport::Cli)]
    transport: Transport,
    /// the daemon's socket, for `--transport socket`.
    #[arg(long, default_value = "/nix/var/nix/daemon-socket/socket")]
    socket: PathBuf,
    /// the `ssh` destination whose `nix-daemon --stdio` to talk to, for `--transport ssh`.
    #[arg(long)]
    ssh: Option<String>,
    /// leaves a step out, to show the checks that rest on it fail.
    #[arg(long, value_enum)]
    remove: Option<Step>,
    /// ends with a garbage collection over the whole store, which the root case checks against.
    #[arg(long)]
    collect_garbage: bool,
}

/// a suffix on every name a run gives a path, so a run in a store an earlier run used, such as
/// the next transport's run in the same VM, adds and builds everything afresh instead of finding
/// it valid.
#[derive(Debug)]
pub(crate) struct Salt(String);

impl Salt {
    pub(crate) fn name(&self, base: &str) -> Result<StoreName, PathError> {
        StoreName::new(format!("{base}-{}", self.0))
    }

    /// the salt of what every run builds alike.
    pub(crate) fn shared() -> Self {
        Self("shared".to_owned())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug)]
pub(crate) enum HarnessError {
    Usage(String),
    Transport(TransportError),
    Readdress(ReaddressError),
    Derivation(DerivationError),
    Path(PathError),
    Nar(NarError),
    SelfReference(SelfReference),
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => f.write_str(message),
            Self::Transport(error) => write!(f, "store transport: {error}"),
            Self::Readdress(error) => write!(f, "re-addressing: {error}"),
            Self::Derivation(error) => write!(f, "synthesizing a case: {error}"),
            Self::Path(error) => write!(f, "naming a case: {error}"),
            Self::Nar(error) => write!(f, "building a case's tree: {error}"),
            Self::SelfReference(error) => write!(f, "addressing a case: {error}"),
        }
    }
}

impl From<TransportError> for HarnessError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

impl From<DerivationError> for HarnessError {
    fn from(error: DerivationError) -> Self {
        Self::Derivation(error)
    }
}

impl From<PathError> for HarnessError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

impl From<NarError> for HarnessError {
    fn from(error: NarError) -> Self {
        Self::Nar(error)
    }
}

impl From<SelfReference> for HarnessError {
    fn from(error: SelfReference) -> Self {
        Self::SelfReference(error)
    }
}

/// the system string Nix uses for this build's platform, so the cases build where they run.
fn system() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    format!("{}-{os}", std::env::consts::ARCH)
}

/// the checks every transport runs.
async fn common<T: StoreTransport>(
    store: &T,
    dir: &StoreDir,
    salt: &Salt,
    args: &Args,
    report: &mut Report,
) -> Result<(), HarnessError> {
    let system = system();
    cases::run(store, dir, &system, salt, report).await?;
    cutoff::shared(store, dir, &system, report).await?;
    let readdressing = args.remove != Some(Step::Readdressing);
    cutoff::run(store, dir, &system, salt, readdressing, report).await?;
    cutoff::self_reference(store, dir, &system, salt, report).await?;
    probes::cancel(store, dir, &system, salt, report).await?;
    Ok(())
}

async fn run(args: Args, report: &mut Report) -> Result<(), HarnessError> {
    let dir = StoreDir::default();
    let cli = CliStore::connect(dir.clone(), &args.work_dir).await?;
    let found = cli.implementation();
    println!("implementation\t{}", found.banner);
    if let Some(expected) = &args.expect {
        report.push(
            "implementation",
            "family and version",
            expected.clone(),
            format!("{} {}", found.family, found.version),
        );
    }
    let salt = Salt(args.transport.name().to_owned());
    let link = args.work_dir.join("roots").join(salt.as_str());
    let rooting = args.remove != Some(Step::Root);
    let system = system();
    // more than one, so every pool in the transport holds several connections even in a VM with
    // one core.
    let connections = NonZeroUsize::new(4).expect("four isn't zero");
    match args.transport {
        Transport::Cli => {
            common(&cli, &dir, &salt, &args, report).await?;
            let rooted = roots::prepare(&cli, &dir, &system, &salt, &link, rooting).await?;
            // the CLI holds no connection between invocations, so nothing of this run's is open.
            if args.collect_garbage {
                roots::collect(report).await?;
                roots::check(&cli, &rooted, report).await?;
            }
        }
        Transport::Socket | Transport::Ssh => {
            let endpoint = match (args.transport, &args.ssh) {
                (Transport::Ssh, Some(destination)) => Endpoint::Ssh(destination.clone()),
                (Transport::Ssh, None) => {
                    return Err(HarnessError::Usage(
                        "--transport ssh needs --ssh <destination>".to_owned(),
                    ));
                }
                _ => Endpoint::Socket(args.socket.clone()),
            };
            let store = WorkerStore::connect(endpoint.clone(), dir.clone(), connections).await?;
            println!(
                "endpoint\t{endpoint}\tdaemon {}\tprotocol {}",
                store.daemon_version(),
                store.protocol()
            );
            report.note("transport", "daemon version", store.daemon_version());
            report.note("transport", "protocol", store.protocol());
            common(&store, &dir, &salt, &args, report).await?;
            probes::trust(&store, &dir, &system, &salt, &args.work_dir, report).await?;
            if args.transport == Transport::Ssh {
                // a root is a link on the daemon's machine, which a client reached through ssh
                // doesn't make, so the transport has to refuse rather than leave it dangling. the
                // root is always tried here, `--remove root` or not: what's checked is the refusal.
                let answer = match roots::prepare(&store, &dir, &system, &salt, &link, true).await {
                    Err(HarnessError::Transport(TransportError::Unsupported(message))) => {
                        eprintln!("indirect-root: refused as it should be: {message}");
                        "refused".to_owned()
                    }
                    Err(other) => return Err(other),
                    Ok(_) => "registered".to_owned(),
                };
                report.push("indirect-root", "a root over ssh-ng", "refused", answer);
                store.close().await?;
                return finish(&args, &dir, report);
            }
            let rooted = roots::prepare(&store, &dir, &system, &salt, &link, rooting).await?;
            // every daemon worker of this run has to be gone before the collection, or its
            // temporary roots would keep what it added and the root would prove nothing.
            store.close().await?;
            if args.collect_garbage {
                roots::collect(report).await?;
                let fresh = WorkerStore::connect(endpoint, dir.clone(), connections).await?;
                roots::check(&fresh, &rooted, report).await?;
                fresh.close().await?;
            }
        }
    }
    finish(&args, &dir, report)
}

fn finish(args: &Args, dir: &StoreDir, report: &mut Report) -> Result<(), HarnessError> {
    // the cases name themselves as they run, so a perturbation is checked against the report
    // rather than a list kept beside it.
    if let Some(case) = &args.perturb
        && report.perturb(case, dir) == 0
    {
        return Err(HarnessError::Usage(format!(
            "--perturb names '{case}', which no check in this run belongs to"
        )));
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = Args::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("shikumi-conformance: starting the runtime: {error}");
            return ExitCode::from(2);
        }
    };
    let mut report = Report::default();
    let result = runtime.block_on(run(args, &mut report));
    print!("{}", report.render());
    match result {
        Err(error) => {
            eprintln!("shikumi-conformance: {error}");
            ExitCode::from(2)
        }
        Ok(()) if report.passed() => ExitCode::SUCCESS,
        Ok(()) => ExitCode::FAILURE,
    }
}
