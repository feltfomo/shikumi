//! `shikumi lock`, which writes pins into the lock. reading and checking the lock is every other
//! command's first step, so a broken one is refused wherever it's read.

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Args, Subcommand};

use crate::{Entry, LockError, NarHash, ToolchainPin, ToolchainSource, Url};

#[derive(Debug, Args)]
pub struct LockArgs {
    #[command(subcommand)]
    command: LockCommand,
}

#[derive(Debug, Subcommand)]
enum LockCommand {
    #[command(
        about = "Pin a toolchain source to the tarball at a URL, by the hash of its unpacked tree"
    )]
    Pin {
        #[arg(value_parser = ToolchainSource::named, help = "The toolchain source to pin")]
        source: ToolchainSource,
        #[arg(
            value_parser = |text: &str| Url::new(text),
            help = "Where the tarball is fetched from, such as a GitHub archive of one revision"
        )]
        url: Url,
        #[arg(
            long,
            value_parser = NarHash::parse,
            help = "The unpacked tree's NAR hash, `sha256-<base64>`; without it, the tarball is fetched with `nix-prefetch-url` and hashed"
        )]
        hash: Option<NarHash>,
        #[arg(long, default_value = ".", help = "The repository's root")]
        root: PathBuf,
    },
    #[command(
        about = "Print the published JSON schema of the lock's header or of one line of a shard"
    )]
    Schema {
        #[arg(value_enum, default_value = "entry", help = "Which file's schema")]
        part: Part,
    },
}

/// the lock's two kinds of file.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Part {
    #[value(help = "`lock.json`, which names the contract and its version")]
    Header,
    #[value(help = "One line of a shard")]
    Entry,
}

pub fn run(args: &LockArgs, out: &mut impl Write) -> Result<(), CliError> {
    match &args.command {
        LockCommand::Pin {
            source,
            url,
            hash,
            root,
        } => {
            let nar_hash = match hash {
                Some(hash) => *hash,
                None => prefetch(url)?,
            };
            let entry = Entry::Toolchain(ToolchainPin {
                name: *source,
                url: url.clone(),
                nar_hash,
            });
            for file in crate::pin(root, &entry)? {
                writeln!(out, "wrote {file}").map_err(CliError::Output)?;
            }
            writeln!(
                out,
                "the lock is read as Git tracks it, so add {}/ to Git",
                crate::DIRECTORY
            )
            .map_err(CliError::Output)
        }
        LockCommand::Schema { part } => {
            let schema = match part {
                Part::Header => crate::header_schema(),
                Part::Entry => crate::entry_schema(),
            };
            out.write_all(crate::render_json(&schema).as_bytes())
                .map_err(CliError::Output)
        }
    }
}

/// fetches and unpacks the tarball into the store with the host's `nix-prefetch-url`, which
/// prints its tree's NAR hash in nix base-32, and has `nix-hash` write that as SRI. the fetch
/// leaves the source in the store, so the first evaluation after it fetches nothing.
fn prefetch(url: &Url) -> Result<NarHash, CliError> {
    let printed = output(
        Command::new("nix-prefetch-url")
            .args(["--unpack", "--type", "sha256"])
            .arg(url.as_str()),
    )?;
    let sri = output(
        Command::new("nix-hash")
            .args(["--type", "sha256", "--to-sri"])
            .arg(printed.trim()),
    )?;
    NarHash::parse(sri.trim()).map_err(CliError::Invalid)
}

fn output(command: &mut Command) -> Result<String, CliError> {
    let program = Path::new(command.get_program()).to_owned();
    let output = command.output().map_err(|error| CliError::Spawn {
        program: program.clone(),
        error,
    })?;
    if !output.status.success() {
        return Err(CliError::Failed {
            program,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    String::from_utf8(output.stdout).map_err(|_| CliError::Failed {
        program,
        stderr: "its output isn't UTF-8".to_owned(),
    })
}

#[derive(Debug)]
pub enum CliError {
    Invalid(String),
    Lock(LockError),
    Spawn { program: PathBuf, error: io::Error },
    Failed { program: PathBuf, stderr: String },
    Output(io::Error),
}

impl From<LockError> for CliError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |program: &PathBuf| program.file_name().unwrap_or(OsStr::new("")).to_owned();
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Lock(error) => error.fmt(f),
            Self::Spawn { program, error } => {
                write!(f, "running {}: {error}", name(program).to_string_lossy())
            }
            Self::Failed { program, stderr } => write!(
                f,
                "{} failed: {}",
                name(program).to_string_lossy(),
                stderr.trim()
            ),
            Self::Output(error) => write!(f, "writing the output: {error}"),
        }
    }
}

impl std::error::Error for CliError {}
