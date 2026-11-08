//! the plain-Cargo baseline every other candidate is compared against. for each iteration and each
//! incremental setting it builds a freshly generated workspace in place, with its own target
//! directory, through the scenarios in the only order that makes them meaningful: cold, no-op,
//! then the edits stacked on each other, each build reusing the session the previous one left.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::contract::{Identifier, Text};
use crate::fixture::rust_workspace::{Edits, RustWorkspace};
use crate::measure::{Measurement, run_measured};
use crate::record::{
    Candidate, Cpu, CpuScope, Fixture, Machine, Mechanism, Outcome, Record, RecordParts, Scenario,
    unmet,
};
use crate::steps::{Step, Timeline};

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
    Io {
        context: String,
        source: io::Error,
    },
    Build {
        scenario: Scenario,
        stderr: String,
    },
    Unexpected {
        scenario: Scenario,
        detail: String,
    },
    /// a Cargo config above the work directory sets what would change the build.
    Refused {
        config: PathBuf,
        key: String,
    },
    /// a measured build whose record breaks the record's own rules.
    Record {
        scenario: Scenario,
        detail: String,
    },
}

impl fmt::Display for BaselineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::Build { scenario, stderr } => {
                write!(f, "cargo failed in the {scenario:?} scenario:\n{stderr}")
            }
            Self::Unexpected { scenario, detail } => f.write_str(&unmet(*scenario, detail)),
            Self::Refused { config, key } => write!(
                f,
                "{} sets {key}, which would configure the baseline's Cargo",
                config.display()
            ),
            Self::Record { scenario, detail } => {
                write!(f, "the {scenario:?} scenario's record is invalid: {detail}")
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
/// directory, extra compiler flags, another rustc or a caching wrapper would each make the baseline
/// something else. every `CARGO_PROFILE_` variable goes too.
const SCRUBBED_ENV: &[&str] = &[
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_BUILD_RUSTFLAGS",
    "RUSTFLAGS",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "RUSTC",
];

/// the same for configuration files: the user's own is left out by giving Cargo a home of its
/// own, flags and the target directory are overridden, and an ancestor of the work directory
/// whose Cargo config sets what can't be overridden, such as a wrapper or a profile, is refused
/// before anything is written. a config that replaces sources or names a linker, as the Nix build
/// sandbox's does, is allowed. a wrapper isn't overridden with `--config`: any
/// `build.rustc-wrapper` given there turns off Cargo's cache of `rustc -vV`, which costs every
/// build about 80 ms of probes plain Cargo doesn't pay.
#[derive(Debug)]
struct Isolation {
    /// every `CARGO_PROFILE_` variable in this process's environment, read once per run.
    profile_vars: Vec<OsString>,
}

impl Isolation {
    fn new(work_dir: &Path) -> Result<Self, BaselineError> {
        refuse_configs(work_dir)?;
        Ok(Self {
            profile_vars: std::env::vars_os()
                .map(|(name, _)| name)
                .filter(|name| name.to_string_lossy().starts_with("CARGO_PROFILE_"))
                .collect(),
        })
    }

    fn apply(&self, command: &mut Command, dir: &Path) {
        command
            .args(["--target-dir", "target"])
            .env("CARGO_HOME", dir.join(".cargo-home"));
        for name in SCRUBBED_ENV {
            command.env_remove(name);
        }
        for name in &self.profile_vars {
            command.env_remove(name);
        }
        // Cargo takes the first of `CARGO_ENCODED_RUSTFLAGS`, `RUSTFLAGS`, a target's rustflags
        // and `build.rustflags` that's set, so an empty one here means no flags, whatever a
        // config file says: nixpkgs' cargo setup hook, for one, gives every target
        // `-Cforce-frame-pointers=yes`.
        command.env("CARGO_ENCODED_RUSTFLAGS", "");
    }
}

/// refuses a Cargo config in `dir` or any directory above it that sets what the baseline can't
/// override.
fn refuse_configs(dir: &Path) -> Result<(), BaselineError> {
    for ancestor in dir.ancestors() {
        for name in ["config", "config.toml"] {
            let config = ancestor.join(".cargo").join(name);
            let text = match std::fs::read_to_string(&config) {
                Ok(text) => text,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_error(format!("reading {}", config.display()))(e)),
            };
            if let Some(key) = configures_build(&text) {
                return Err(BaselineError::Refused { config, key });
            }
        }
    }
    Ok(())
}

/// the first setting in a Cargo config that changes how rustc builds and that the baseline can't
/// override from its environment: a profile or env table, or a `build` key for the compiler, its
/// wrappers, target or parallelism. flags, the target directory and incremental are overridden
/// instead. read line by line rather than as TOML: a dotted key or a table header names the
/// setting either way.
fn configures_build(text: &str) -> Option<String> {
    const BUILD: &[&str] = &[
        "rustc",
        "rustc-wrapper",
        "rustc-workspace-wrapper",
        "target",
        "jobs",
        "pipelining",
    ];
    let mut table = String::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let key = if let Some(header) = line.strip_prefix('[') {
            table = header
                .trim_matches(|c| c == '[' || c == ']')
                .trim()
                .to_owned();
            table.clone()
        } else {
            let key = line
                .split('=')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"');
            if table.is_empty() {
                key.to_owned()
            } else {
                format!("{table}.{key}")
            }
        };
        let mut parts = key.split('.');
        let refused = match parts.next() {
            Some("profile" | "env") => true,
            Some("build") => parts.next().is_some_and(|k| BUILD.contains(&k)),
            _ => false,
        };
        if refused {
            return Some(key);
        }
    }
    None
}

/// after a cold build, the incremental directory exists exactly when incremental is on: a bar
/// row named incremental that left none wasn't.
fn check_incremental(dir: &Path, incremental: bool) -> Result<(), BaselineError> {
    let sessions = dir.join("target/debug/incremental");
    let present = std::fs::read_dir(&sessions).is_ok_and(|mut entries| entries.next().is_some());
    if present == incremental {
        Ok(())
    } else {
        Err(BaselineError::Unexpected {
            scenario: Scenario::Cold,
            detail: format!(
                "incremental was {}, but {} {}",
                if incremental { "on" } else { "off" },
                sessions.display(),
                if present {
                    "holds sessions"
                } else {
                    "is empty or missing"
                }
            ),
        })
    }
}

/// the scenarios in the only order that makes them meaningful: each one builds on the state the
/// previous one left, and the edits stack.
pub const SEQUENCE: [Scenario; 5] = [
    Scenario::Cold,
    Scenario::NoOp,
    Scenario::OneFunctionEdit,
    Scenario::PublicSignatureEdit,
    Scenario::IdenticalOutputEdit,
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
        mechanism: Mechanism::Cargo,
    };
    let isolation = Isolation::new(&config.work_dir)?;
    let workspace = &config.workspace;
    let base = workspace.render();
    let fixture = Fixture::RustWorkspace {
        seed: workspace.seed(),
        parameters: workspace.params().clone(),
        digest: base.digest(),
    };
    let model = fixture.model().map_err(|e| BaselineError::Record {
        scenario: Scenario::Cold,
        detail: e.to_string(),
    })?;

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
                if let Some(edit) = scenario.edit() {
                    // the edit is the developer's, not the build's, so it's written before the
                    // timer starts.
                    let patch = workspace.patch(edit, applied);
                    std::fs::write(dir.join(&patch.path), patch.contents)
                        .map_err(io_error(format!("applying the {scenario:?} edit")))?;
                    applied = applied.with(edit);
                }
                let expected = scenario
                    .expected_built(Mechanism::Cargo, &model)
                    .expect("every scenario in the sequence is a build");
                let (measurement, compiled) =
                    build(&config.cargo, &dir, incremental, scenario, &isolation)?;
                check_compiled(scenario, compiled, expected)?;
                if scenario == Scenario::Cold {
                    check_incremental(&dir, incremental)?;
                }
                let measured = Measured {
                    scenario,
                    incremental,
                    iteration,
                    measurement,
                    compiled,
                    expected,
                };
                let record = measured.record(config, &candidate, &fixture, model.graph.units)?;
                sink(&record).map_err(io_error("writing a record"))?;
            }
            std::fs::remove_dir_all(&dir)
                .map_err(io_error(format!("removing {}", dir.display())))?;
        }
    }
    Ok(())
}

/// one Cargo build as it was measured and checked.
struct Measured {
    scenario: Scenario,
    incremental: bool,
    iteration: u32,
    measurement: Measurement,
    compiled: u32,
    expected: u32,
}

impl Measured {
    /// the build's record. the whole Cargo process is its one step, compiling, and the record's
    /// own rules check the count Cargo reported against the model once more.
    fn record(
        self,
        config: &BaselineConfig,
        candidate: &Candidate,
        fixture: &Fixture,
        units: u32,
    ) -> Result<Record, BaselineError> {
        let Self {
            scenario,
            incremental,
            iteration,
            measurement,
            compiled,
            expected,
        } = self;
        let Measurement {
            started_at_unix_ms,
            start_ns,
            wall_time_ns,
            cpu_user_ns,
            cpu_system_ns,
        } = measurement;
        let timeline = Timeline::starting_at(start_ns);
        timeline.record(Step::Compile, start_ns, start_ns + wall_time_ns);
        let invalid = |detail: String| BaselineError::Record { scenario, detail };
        let steps = timeline.finish().map_err(|e| invalid(e.to_string()))?;
        Record::new(RecordParts {
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
            cpu: Cpu {
                scope: CpuScope::ProcessTree,
                user_ns: cpu_user_ns,
                system_ns: cpu_system_ns,
            },
            buckets: steps.buckets,
            covered_ns: steps.covered_ns,
            outcome: Outcome::Build {
                built: compiled,
                skipped: units.saturating_sub(compiled),
                expected_built: expected,
                source_bytes_sent: 0,
                output_bytes_sent: 0,
                identity: None,
                warm: None,
            },
            store_valid_paths: None,
        })
        .map_err(|e| invalid(e.to_string()))
    }
}

/// builds and returns the measurement with the number of crates Cargo reported compiling.
fn build(
    cargo: &Path,
    dir: &Path,
    incremental: bool,
    scenario: Scenario,
    isolation: &Isolation,
) -> Result<(Measurement, u32), BaselineError> {
    let mut command = Command::new(cargo);
    command
        .args(["build", "--offline", "--workspace"])
        .current_dir(dir)
        .env("CARGO_INCREMENTAL", if incremental { "1" } else { "0" })
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TERM_QUIET", "false")
        .env("CARGO_TERM_VERBOSE", "false");
    isolation.apply(&mut command, dir);
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

/// a scenario that compiled other than the fixture's dependency model predicts measured
/// something other than its name: a no-op that rebuilt, an edit Cargo never saw, or an edit that
/// landed in the wrong crate.
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
            .map(|r| (r.scenario(), r.incremental()))
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
                .all(|r| r.graph().units == 4 && r.nix().is_none())
        );
        // Cargo never reads the notes file, so the identical-output edit compiles nothing.
        assert!(records.iter().all(|r| {
            r.scenario() != Scenario::IdenticalOutputEdit
                || matches!(r.outcome(), Outcome::Build { built: 0, .. })
        }));
        assert!(
            std::fs::read_dir(work.path()).unwrap().next().is_none(),
            "run directories were left behind"
        );
    }

    #[test]
    fn a_workspace_under_a_cargo_config_is_refused() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".cargo")).unwrap();
        std::fs::write(
            root.path().join(".cargo/config.toml"),
            "[build]\nrustc-wrapper = \"sccache\"\n",
        )
        .unwrap();
        let workspace = root.path().join("deep/ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let error = refuse_configs(&workspace).unwrap_err();
        assert!(error.to_string().contains("build.rustc-wrapper"), "{error}");
        for refused in [
            "[build]\nrustc = \"/opt/rustc\"",
            "[profile.dev]\nincremental = false",
            "build.jobs = 1",
            "[env]\nRUSTFLAGS = \"-g\"",
        ] {
            assert!(configures_build(refused).is_some(), "{refused}");
        }
        let clean = tempfile::tempdir().unwrap();
        assert!(refuse_configs(clean.path()).is_ok());
    }

    // the Nix build sandbox vendors crates through a Cargo config in `/build/.cargo`, above every
    // temporary directory there, which also names the target's linker and flags (nixpkgs' cargo
    // setup hook); the baseline overrides the flags, and the rest changes nothing it measures.
    #[test]
    fn a_vendoring_config_above_the_workspace_is_allowed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".cargo")).unwrap();
        std::fs::write(
            root.path().join(".cargo/config.toml"),
            "[target.\"x86_64-unknown-linux-gnu\"]\n\"linker\" = \"cc\"\n\"rustflags\" = [ \"-Cforce-frame-pointers=yes\" ]\n\n[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"/build/cargo-vendor-dir\"\n",
        )
        .unwrap();
        let workspace = root.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        assert!(refuse_configs(&workspace).is_ok());
    }

    // the row named incremental has to have left incremental sessions, and the row named off
    // none, or the bar was measured against something else.
    #[test]
    fn an_incremental_directory_that_contradicts_the_row_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_incremental(dir.path(), false).is_ok());
        assert!(check_incremental(dir.path(), true).is_err());
        let sessions = dir.path().join("target/debug/incremental/c0000-x");
        std::fs::create_dir_all(sessions).unwrap();
        assert!(check_incremental(dir.path(), true).is_ok());
        assert!(check_incremental(dir.path(), false).is_err());
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
