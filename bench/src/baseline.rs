//! the plain-Cargo baseline every other candidate is compared against. for each iteration and each
//! incremental setting it builds a freshly generated workspace through the four scenarios in the
//! only order that makes them meaningful: cold, no-op, then the two edits stacked on each other.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::contract::{Identifier, Text};
use crate::fixture::rust_workspace::{Edits, RustWorkspace};
use crate::measure::{Measurement, run_measured};
use crate::record::{Candidate, Fixture, Machine, Record, RecordVersion, Scenario};

#[derive(Debug, Clone)]
pub struct BaselineConfig {
    pub workspace: RustWorkspace,
    pub iterations: u32,
    /// where fixtures are generated and built. each run gets its own subdirectory, removed when the
    /// run's scenarios finish.
    pub work_dir: PathBuf,
    pub cargo: PathBuf,
    pub machine: Machine,
}

#[derive(Debug)]
pub enum BaselineError {
    Io { context: String, source: io::Error },
    Build { scenario: Scenario, stderr: String },
    Unexpected { scenario: Scenario, detail: String },
}

impl fmt::Display for BaselineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::Build { scenario, stderr } => {
                write!(f, "cargo failed in the {scenario:?} scenario:\n{stderr}")
            }
            Self::Unexpected { scenario, detail } => {
                write!(
                    f,
                    "the {scenario:?} scenario didn't measure what it claims: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for BaselineError {}

fn io_error(context: impl Into<String>) -> impl FnOnce(io::Error) -> BaselineError {
    move |source| BaselineError::Io {
        context: context.into(),
        source,
    }
}

/// environment that would change what Cargo does without the record saying so: a redirected target
/// directory, extra compiler flags or a caching wrapper would each make the baseline something else.
const SCRUBBED_ENV: &[&str] = &[
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTFLAGS",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "CARGO_PROFILE_DEV_INCREMENTAL",
];

/// the scenarios in the only order that makes them meaningful: each one builds on the state the
/// previous one left, and the two edits stack.
const SEQUENCE: [Scenario; 4] = [
    Scenario::Cold,
    Scenario::NoOp,
    Scenario::OneFunctionEdit,
    Scenario::PublicSignatureEdit,
];

/// runs every scenario and hands each record to `sink` as soon as it's measured, so an interrupted
/// run keeps what it finished.
pub fn run(
    config: &BaselineConfig,
    sink: &mut dyn FnMut(&Record) -> io::Result<()>,
) -> Result<(), BaselineError> {
    let candidate = Candidate {
        name: Identifier::new("cargo").expect("a literal identifier"),
        version: cargo_version(&config.cargo)?,
    };
    let workspace = &config.workspace;
    let base = workspace.render();
    let fixture = Fixture::RustWorkspace {
        seed: workspace.seed(),
        parameters: workspace.params().clone(),
        digest: base.digest(),
    };
    let unit_count = workspace.params().unit_count();

    for iteration in 0..config.iterations {
        for incremental in [false, true] {
            let dir = config.work_dir.join(format!(
                "run-{iteration}-incremental-{}",
                if incremental { "on" } else { "off" }
            ));
            base.write_new(&dir).map_err(io_error(format!(
                "writing the fixture to {}",
                dir.display()
            )))?;

            let mut applied = Edits::default();
            for scenario in SEQUENCE {
                let expected = match scenario.edit() {
                    None if scenario == Scenario::Cold => unit_count,
                    None => 0,
                    Some(edit) => {
                        let patch = workspace.patch(edit, applied);
                        std::fs::write(dir.join(&patch.path), patch.contents)
                            .map_err(io_error(format!("applying the {scenario:?} edit")))?;
                        applied = applied.with(edit);
                        workspace.units_rebuilt_by(edit)
                    }
                };
                let (measurement, compiled) = build(&config.cargo, &dir, incremental, scenario)?;
                check_compiled(scenario, compiled, expected)?;
                let Measurement {
                    started_at_unix_ms,
                    wall_time_ns,
                    cpu_user_ns,
                    cpu_system_ns,
                } = measurement;
                let record = Record {
                    schema_version: RecordVersion::default(),
                    started_at_unix_ms,
                    machine: config.machine.clone(),
                    nix: None,
                    candidate: candidate.clone(),
                    transport: None,
                    fixture: fixture.clone(),
                    scenario,
                    incremental: Some(incremental),
                    iteration,
                    wall_time_ns,
                    cpu_user_ns,
                    cpu_system_ns,
                };
                sink(&record).map_err(io_error("writing a record"))?;
            }
            std::fs::remove_dir_all(&dir)
                .map_err(io_error(format!("removing {}", dir.display())))?;
        }
    }
    Ok(())
}

/// builds and returns the measurement with the number of crates Cargo reported compiling.
fn build(
    cargo: &Path,
    dir: &Path,
    incremental: bool,
    scenario: Scenario,
) -> Result<(Measurement, u32), BaselineError> {
    let mut command = Command::new(cargo);
    command
        .args(["build", "--offline", "--workspace"])
        .current_dir(dir)
        .env("CARGO_INCREMENTAL", if incremental { "1" } else { "0" })
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TERM_QUIET", "false")
        .env("CARGO_TERM_VERBOSE", "false");
    for name in SCRUBBED_ENV {
        command.env_remove(name);
    }
    let (measurement, output) = run_measured(&mut command).map_err(io_error("running cargo"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(BaselineError::Build {
            scenario,
            stderr: stderr.into_owned(),
        });
    }
    let compiled = stderr
        .lines()
        .filter(|line| line.trim_start().starts_with("Compiling "))
        .count();
    Ok((measurement, u32::try_from(compiled).unwrap_or(u32::MAX)))
}

/// a scenario that compiled other than the fixture's dependency model predicts measured something
/// other than its name: a no-op that rebuilt, an edit Cargo never saw, or an edit that landed in the
/// wrong crate.
fn check_compiled(scenario: Scenario, compiled: u32, expected: u32) -> Result<(), BaselineError> {
    if compiled == expected {
        Ok(())
    } else {
        Err(BaselineError::Unexpected {
            scenario,
            detail: format!(
                "cargo compiled {compiled} crates where the fixture predicts {expected}"
            ),
        })
    }
}

fn cargo_version(cargo: &Path) -> Result<Text, BaselineError> {
    let output = Command::new(cargo)
        .arg("--version")
        .output()
        .map_err(io_error(format!("running {} --version", cargo.display())))?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Text::new(version).map_err(|e| BaselineError::Io {
        context: format!("{} --version", cargo.display()),
        source: io::Error::new(io::ErrorKind::InvalidData, e.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::rust_workspace::{LargeCrateParams, Percent, RustWorkspaceParams};
    use crate::measure::machine;
    use std::num::NonZeroU32;

    // runs real Cargo builds of a tiny workspace, so it proves the scenario order, the edit
    // detection and the records end to end rather than any one piece.
    #[test]
    fn runs_every_scenario_both_ways_and_writes_valid_records() {
        let nz = |n| NonZeroU32::new(n).unwrap();
        let params = RustWorkspaceParams {
            crates: nz(3),
            depth: nz(2),
            dependencies_per_crate: nz(2),
            modules_per_crate: nz(2),
            functions_per_module: nz(3),
            generic_percent: Percent::new(50).unwrap(),
            large_crate: Some(LargeCrateParams {
                modules: nz(3),
                functions_per_module: nz(4),
                dependencies: nz(2),
            }),
        };
        let work = tempfile::tempdir().unwrap();
        let config = BaselineConfig {
            workspace: RustWorkspace::generate(11, params).unwrap(),
            iterations: 1,
            work_dir: work.path().to_owned(),
            cargo: PathBuf::from(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())),
            machine: machine(Some(Text::new("test-machine").unwrap())).unwrap(),
        };
        let mut lines = Vec::new();
        run(&config, &mut |record| {
            lines.push(record.to_json_line());
            Ok(())
        })
        .unwrap();

        let records: Vec<Record> = lines
            .iter()
            .map(|line| Record::from_json_line(line).unwrap())
            .collect();
        let seen: Vec<(Scenario, Option<bool>)> = records
            .iter()
            .map(|r| (r.scenario, r.incremental))
            .collect();
        let mut expected = Vec::new();
        for incremental in [false, true] {
            for scenario in SEQUENCE {
                expected.push((scenario, Some(incremental)));
            }
        }
        assert_eq!(seen, expected);
        assert!(
            records
                .iter()
                .all(|r| r.unit_count() == 4 && r.nix.is_none())
        );
        assert!(
            std::fs::read_dir(work.path()).unwrap().next().is_none(),
            "run directories were left behind"
        );
    }

    #[test]
    fn a_count_other_than_the_model_predicts_is_rejected() {
        // an edit that landed in the hub instead of a leaf rebuilds everything; only the exact
        // count tells that apart from a one-function edit.
        assert!(check_compiled(Scenario::OneFunctionEdit, 4, 1).is_err());
        assert!(check_compiled(Scenario::PublicSignatureEdit, 1, 4).is_err());
        assert!(check_compiled(Scenario::NoOp, 1, 0).is_err());
        assert!(check_compiled(Scenario::Cold, 3, 4).is_err());
        assert!(check_compiled(Scenario::OneFunctionEdit, 1, 1).is_ok());
    }
}
