//! `shikumi-conformance` checks the store paths Shikumi computes against the daemon behind the Nix
//! CLI on the PATH. it's meant to run as an untrusted user, which it checks, so the daemon
//! re-derives every content address it's sent rather than taking it on trust.
//!
//! it prints one line per check and exits 0 when every check passes, 1 when one fails and 2 when
//! the run itself can't finish.

mod cases;
mod report;

use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use shikumi_derivation::{DerivationError, NarError, PathError, SelfReference, StoreDir};
use shikumi_transport::{CliStore, TransportError};

use crate::report::Report;

#[derive(Debug, Parser)]
#[command(name = "shikumi-conformance")]
struct Args {
    /// a directory this user can write, for throwaway binary caches.
    #[arg(long)]
    work_dir: PathBuf,
    /// the implementation the run must find, as `<family> <version>`, such as `Lix 2.95.4`.
    #[arg(long)]
    expect: Option<String>,
    /// alters every value computed for this case, to show the harness fails when a computation
    /// drifts.
    #[arg(long)]
    perturb: Option<String>,
}

#[derive(Debug)]
pub(crate) enum HarnessError {
    Usage(String),
    Transport(TransportError),
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

async fn run(args: Args, report: &mut Report) -> Result<(), HarnessError> {
    let dir = StoreDir::default();
    let store = CliStore::connect(dir.clone(), &args.work_dir).await?;
    let found = store.implementation();
    println!("implementation\t{}", found.banner);
    if let Some(expected) = &args.expect {
        report.push(
            "implementation",
            "family and version",
            expected.clone(),
            format!("{} {}", found.family, found.version),
        );
    }
    cases::run(&store, &dir, &system(), report).await?;
    // the cases name themselves as they run, so a perturbation is checked against the report
    // rather than a list kept beside it.
    if let Some(case) = &args.perturb
        && report.perturb(case, &dir) == 0
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
