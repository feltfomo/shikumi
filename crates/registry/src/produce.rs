//! producing the registry document: evaluating the declaration layer through the host's
//! `nix-instantiate`, reading the declarations it serializes, checking them, and handing the
//! document to the shared refusal.
//!
//! the evaluation can't build or read a build's output through the evaluator. it runs against
//! `dummy://`, a store that holds nothing and accepts nothing, so every derivation output it would
//! import, read, hash or test for is missing on every host, whatever the host's store holds.
//! import-from-derivation is off as well, which is the same refusal stated as the setting the three
//! implementations share. the settings that would hand a declaration another way to run or build
//! something, `builtins.exec`, plugins and experimental builtins, are pinned off on the command
//! line, which outranks every configuration file and `NIX_CONFIG`.
//!
//! the child gets no environment at all, so `builtins.getEnv` reads nothing and a caller's `PWD`,
//! `TMPDIR` or `NIX_PATH` can't make the document depend on where it was produced. a path read
//! straight from the filesystem, with no store context, is out of reach of all of this.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::check;
use crate::declarations::Declarations;
use crate::document::{Document, Identifier, Registry, RegistryError};

/// the manifest's file name at the repository root.
pub const MANIFEST: &str = "shikumi.nix";

/// where the producer finds what it evaluates with: the host's `nix-instantiate`, Shikumi's own
/// declaration layer and the nixpkgs source tree whose `lib` the layer runs on.
#[derive(Debug, Clone)]
pub struct Producer {
    nix_instantiate: PathBuf,
    layer: PathBuf,
    nixpkgs: PathBuf,
}

impl Producer {
    pub fn new(nix_instantiate: PathBuf, layer: PathBuf, nixpkgs: PathBuf) -> Self {
        Self {
            nix_instantiate,
            layer,
            nixpkgs,
        }
    }

    /// the producer an installed `shikumi` runs with. its package sets `SHIKUMI_NIX` to the
    /// declaration layer it ships and `SHIKUMI_NIXPKGS` to the nixpkgs it was built against, and
    /// `nix-instantiate` comes from the caller's `PATH`, since the host's evaluator is the one
    /// that reads a repository's Nix.
    pub fn from_environment() -> Result<Self, ProduceError> {
        let required = |name: &'static str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .ok_or(ProduceError::Unset(name))
        };
        let layer = required("SHIKUMI_NIX")?;
        let nixpkgs = required("SHIKUMI_NIXPKGS")?;
        let path = std::env::var_os("PATH").unwrap_or_default();
        let nix_instantiate =
            find_program("nix-instantiate", &path).ok_or(ProduceError::NoEvaluator)?;
        Ok(Self::new(nix_instantiate, layer, nixpkgs))
    }

    /// composes the repository at `root`, globally or focused on one project, and refuses the
    /// document unless it's accepted.
    pub fn produce(
        &self,
        root: &Path,
        focus: Option<&Identifier>,
    ) -> Result<Registry, ProduceError> {
        Registry::try_from(self.document(root, focus)?).map_err(ProduceError::Document)
    }

    /// the document the checker built, findings included, before the refusal.
    pub fn document(
        &self,
        root: &Path,
        focus: Option<&Identifier>,
    ) -> Result<Document, ProduceError> {
        let (declarations, root) = self.declarations(root, focus)?;
        check::check(&declarations, &utf8(&root)?).map_err(ProduceError::Check)
    }

    /// the declarations as the declaration layer serialized them, and the canonical root they
    /// were evaluated from.
    pub fn declarations(
        &self,
        root: &Path,
        focus: Option<&Identifier>,
    ) -> Result<(Declarations, PathBuf), ProduceError> {
        let root = std::fs::canonicalize(root).map_err(|error| ProduceError::Root {
            root: root.to_owned(),
            error,
        })?;
        if !root.join(MANIFEST).is_file() {
            return Err(ProduceError::NoManifest(root));
        }
        let output = self
            .command(&root, focus)?
            .output()
            .map_err(|error| ProduceError::Spawn {
                program: self.nix_instantiate.clone(),
                error,
            })?;
        if !output.status.success() {
            return Err(ProduceError::Evaluation {
                status: output.status.to_string(),
                stderr: String::from_utf8_lossy(&output.stderr)
                    .trim_end()
                    .to_owned(),
            });
        }
        let declarations =
            Declarations::from_json(&output.stdout).map_err(ProduceError::Declarations)?;
        Ok((declarations, root))
    }

    // TODO: evaluate in pure mode from the context cache's store copy of exactly the keyed files,
    // once the context cache exists. until then a declaration can read a store path whose context
    // it stripped, an untracked file, `.git` or the clock, and the document can depend on them.
    fn command(&self, root: &Path, focus: Option<&Identifier>) -> Result<Command, ProduceError> {
        let mut command = Command::new(&self.nix_instantiate);
        command
            .args(["--eval", "--strict", "--json"])
            .args(["--store", "dummy://"])
            .args(["--option", "allow-import-from-derivation", "false"])
            .args(["--option", "nix-path", ""])
            .args([
                "--option",
                "allow-unsafe-native-code-during-evaluation",
                "false",
            ])
            .args(["--option", "plugin-files", ""])
            .args(["--option", "experimental-features", ""])
            .arg(self.layer.join("produce.nix"))
            .args(["--argstr", "nixpkgs"])
            .arg(utf8(&self.nixpkgs)?)
            .args(["--argstr", "root"])
            .arg(utf8(root)?);
        if let Some(focus) = focus {
            command.args(["--argstr", "focus", focus.as_str()]);
        }
        command.env_clear().current_dir("/");
        Ok(command)
    }
}

fn utf8(path: &Path) -> Result<String, ProduceError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| ProduceError::NotUtf8(path.to_owned()))
}

fn find_program(name: &str, path: &OsString) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[derive(Debug)]
pub enum ProduceError {
    Unset(&'static str),
    NoEvaluator,
    Root {
        root: PathBuf,
        error: io::Error,
    },
    NoManifest(PathBuf),
    NotUtf8(PathBuf),
    Spawn {
        program: PathBuf,
        error: io::Error,
    },
    Evaluation {
        status: String,
        stderr: String,
    },
    /// the layer printed something that isn't the serialized declarations: a Shikumi bug, since
    /// the layer writes nothing else.
    Declarations(serde_json::Error),
    Check(check::CheckError),
    Document(RegistryError),
}

impl fmt::Display for ProduceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unset(name) => write!(
                f,
                "{name} is unset; it's set by the shikumi package, so run shikumi from it"
            ),
            Self::NoEvaluator => {
                f.write_str("no nix-instantiate on PATH to evaluate declarations with")
            }
            Self::Root { root, error } => write!(f, "opening {}: {error}", root.display()),
            Self::NoManifest(root) => write!(f, "{} has no {MANIFEST} manifest", root.display()),
            Self::NotUtf8(path) => write!(f, "{} isn't valid UTF-8", path.display()),
            Self::Spawn { program, error } => write!(f, "running {}: {error}", program.display()),
            Self::Evaluation { status, stderr } => {
                write!(
                    f,
                    "evaluating the declarations failed ({status}):\n{stderr}"
                )
            }
            Self::Declarations(error) => write!(
                f,
                "the declaration layer's output isn't the serialized declarations, which is a Shikumi bug: {error}"
            ),
            Self::Check(error) => error.fmt(f),
            Self::Document(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ProduceError {}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    fn producer() -> Producer {
        Producer::new(
            PathBuf::from("/bin/nix-instantiate"),
            PathBuf::from("/layer"),
            PathBuf::from("/nixpkgs"),
        )
    }

    #[test]
    fn evaluation_can_reach_no_store_and_nothing_that_builds_or_runs() {
        let command = producer().command(Path::new("/repo"), None).unwrap();
        let args: Vec<&OsStr> = command.get_args().collect();
        let has = |pair: [&str; 2]| args.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]);
        assert!(has(["--store", "dummy://"]), "{args:?}");
        for (setting, value) in [
            ("allow-import-from-derivation", "false"),
            ("allow-unsafe-native-code-during-evaluation", "false"),
            ("plugin-files", ""),
            ("experimental-features", ""),
        ] {
            let pinned = args.windows(3).any(|w| w == ["--option", setting, value]);
            assert!(pinned, "{setting} isn't pinned to {value:?}: {args:?}");
        }
    }

    #[test]
    fn the_child_gets_no_environment() {
        let command = producer().command(Path::new("/repo"), None).unwrap();
        let passed: Vec<_> = command.get_envs().collect();
        assert!(passed.is_empty(), "{passed:?} reached the evaluator");
        assert_eq!(command.get_current_dir(), Some(Path::new("/")));
    }

    #[test]
    fn a_focus_is_passed_only_when_given() {
        let focus = Identifier::new("web").unwrap();
        let focused = producer()
            .command(Path::new("/repo"), Some(&focus))
            .unwrap();
        let global = producer().command(Path::new("/repo"), None).unwrap();
        let args = |command: &Command| -> Vec<String> {
            command
                .get_args()
                .map(|a| a.to_str().unwrap().to_owned())
                .collect()
        };
        assert!(args(&focused).ends_with(&["--argstr".into(), "focus".into(), "web".into()]));
        assert!(!args(&global).iter().any(|a| a == "focus"));
    }

    #[test]
    fn a_root_without_a_manifest_is_refused_before_evaluating() {
        let root = tempfile::tempdir().unwrap();
        match producer().produce(root.path(), None) {
            Err(ProduceError::NoManifest(_)) => {}
            other => panic!("expected the missing manifest, got {other:?}"),
        }
    }
}
