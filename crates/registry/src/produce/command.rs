//! every evaluation's command line, built in one place, so every one gets the same refusals and the
//! copying evaluation differs from them only where the settings table says. what each refusal keeps
//! a declaration from is in docs/explanation/architecture.md, under the declaration layer and the
//! context cache.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use serde::Serialize;

use crate::context::Cache;
use crate::context::tree::Nar;
use crate::evaluation::Request;
use crate::layer::STORE_NAMES;

/// a setting every evaluation runs with, set on the command line, where no configuration file or
/// `NIX_CONFIG` can set it otherwise, since the evaluator reads neither.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Setting {
    pub(crate) name: &'static str,
    pub(crate) value: &'static str,
    /// what the copying evaluation runs with instead, where it can't keep the value.
    pub(crate) copying: Option<&'static str>,
}

/// every setting, which the cache's key holds too, so a change to one is a miss.
pub(crate) const SETTINGS: &[Setting] = &[
    Setting {
        name: "allow-import-from-derivation",
        value: "false",
        copying: None,
    },
    Setting {
        name: "allow-unsafe-native-code-during-evaluation",
        value: "false",
        copying: None,
    },
    Setting {
        name: "plugin-files",
        value: "",
        copying: None,
    },
    Setting {
        name: "experimental-features",
        value: "",
        copying: None,
    },
    Setting {
        name: "nix-path",
        value: "",
        copying: None,
    },
    Setting {
        name: "restrict-eval",
        value: "true",
        copying: None,
    },
    Setting {
        name: "substituters",
        value: "",
        copying: None,
    },
    // the copy reads the directories it copies from, which pure mode can't.
    Setting {
        name: "pure-eval",
        value: "true",
        copying: Some("false"),
    },
];

/// a store copy: its path and its NAR.
#[derive(Debug, Clone)]
pub(crate) struct Copied {
    pub(crate) path: String,
    pub(crate) nar: Nar,
}

/// what one evaluation reads: its store, which registers exactly these three copies, and each copy.
#[derive(Debug, Clone)]
pub(crate) struct Inputs {
    pub(crate) store: OsString,
    pub(crate) layer: Copied,
    pub(crate) nixpkgs: Copied,
    pub(crate) root: Copied,
}

/// one evaluation to start: a copy into the private store, or a request of the layer from copies
/// already there.
pub(crate) enum Run<'a> {
    Copy {
        copies: &'a Path,
        sources: &'a [&'a Path],
    },
    Evaluate {
        inputs: &'a Inputs,
        request: &'a Request<'a>,
    },
    /// the root copy's location as the evaluator reads it, which is what a declaration's `./.`
    /// gives: the store path, or, for an evaluator that reads a store at its real location, where
    /// the copy lies on disk.
    Locate { inputs: &'a Inputs },
}

/// what the evaluator is entered through: each store copy it reads, by its path and NAR hash, which
/// `builtins.path` turns into a path pure mode lets it read without reading anything else. the
/// root is handed over as the path its marker file evaluates to, which is the copy's root wherever
/// the evaluator really reads it from: Lix reads a store under another root at its real location,
/// and a declaration's `./.` says so (lix 2.95.4 libexpr/primops.cc:172-174 and eval.cc:566-572,
/// 2.96.0 primops.cc:179-182 and eval.cc:563-569).
fn entry_expression() -> String {
    format!(
        r#"{{ layer, layerHash, nixpkgs, nixpkgsHash, root, rootHash, request }}:
let
  enter = path: name: sha256: builtins.path {{ path = /. + path; inherit name sha256; }};
in
import (enter layer "{layer}" layerHash + "/produce.nix") {{
  nixpkgs = enter nixpkgs "{source}" nixpkgsHash;
  root = import (enter root "{keyed}" rootHash + "/{marker}");
  inherit request;
}}"#,
        layer = STORE_NAMES.layer,
        source = shikumi_lock::SOURCE_NAME,
        keyed = STORE_NAMES.keyed,
        marker = STORE_NAMES.root_marker,
    )
}

/// what the locating evaluation runs: the root copy entered as every evaluation enters it, and its
/// marker's value as text.
fn locate_expression() -> String {
    format!(
        r#"{{ root, rootHash }}:
toString (import (builtins.path {{ path = /. + root; name = "{keyed}"; sha256 = rootHash; }} + "/{marker}"))"#,
        keyed = STORE_NAMES.keyed,
        marker = STORE_NAMES.root_marker,
    )
}

/// what the copying evaluation runs: each copy in `copies`, a JSON list of its source, name and NAR
/// hash, added to the store whole, and checked against the hash.
const COPY: &str = r#"{ copies }:
map (
  copy:
  builtins.path {
    path = /. + copy.source;
    inherit (copy) name;
    sha256 = copy.hash;
  }
) (builtins.fromJSON (builtins.readFile copies))"#;

/// the command line of `run`, with the evaluator at `nix_instantiate`.
pub(crate) fn command(nix_instantiate: &Path, cache: &Cache, run: &Run<'_>) -> Command {
    let mut command = cache.command(nix_instantiate);
    command.args(["--eval", "--strict", "--json"]);
    let copying = matches!(run, Run::Copy { .. });
    let store = match run {
        Run::Copy { .. } => cache.store_url(None),
        Run::Evaluate { inputs, .. } | Run::Locate { inputs } => inputs.store.clone(),
    };
    command
        .arg("--eval-store")
        .arg(store)
        .args(["--store", "dummy://"]);
    for setting in SETTINGS {
        let value = match (copying, setting.copying) {
            (true, Some(value)) => value,
            _ => setting.value,
        };
        command.args(["--option", setting.name, value]);
    }
    // every frame, in the plain format the failure is read from, whatever the host asks for.
    command.args(["--show-trace", "--log-format", "raw"]);
    match run {
        Run::Copy { copies, sources } => {
            command.arg("--read-write-mode");
            for (index, source) in sources.iter().chain([copies]).enumerate() {
                let mut entry = OsString::from(format!("shikumi-copy-{index}="));
                entry.push(source);
                command.arg("-I").arg(entry);
            }
            command
                .args(["--expr", COPY, "--argstr", "copies"])
                .arg(copies);
        }
        Run::Evaluate { inputs, request } => {
            command.arg("--expr").arg(entry_expression());
            for (name, value) in [
                ("layer", inputs.layer.path.clone()),
                ("layerHash", inputs.layer.nar.hash.to_sri()),
                ("nixpkgs", inputs.nixpkgs.path.clone()),
                ("nixpkgsHash", inputs.nixpkgs.nar.hash.to_sri()),
                ("root", inputs.root.path.clone()),
                ("rootHash", inputs.root.nar.hash.to_sri()),
            ] {
                command.args(["--argstr", name]).arg(value);
            }
            command.args(["--argstr", "request"]).arg(request.to_json());
        }
        Run::Locate { inputs } => {
            command.arg("--expr").arg(locate_expression());
            command.args(["--argstr", "root"]).arg(&inputs.root.path);
            command
                .args(["--argstr", "rootHash"])
                .arg(inputs.root.nar.hash.to_sri());
        }
    }
    command
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use shikumi_derivation::Sha256;

    use super::*;
    use crate::context::Identity;
    use crate::contract::RelativePath;
    use crate::evaluation::Project;

    fn cache(base: &Path) -> Cache {
        let identity = Identity::of(Path::new("/proc/self/exe")).unwrap();
        Cache::open_in(base, base, &identity).unwrap()
    }

    fn inputs(cache: &Cache) -> Inputs {
        let copied = |path: &str| Copied {
            path: path.to_owned(),
            nar: Nar {
                hash: Sha256::digest(path.as_bytes()),
                size: 8,
            },
        };
        Inputs {
            store: cache.store_url(Some(Path::new("/state"))),
            layer: copied("/nix/store/a-shikumi-layer"),
            nixpkgs: copied("/nix/store/b-source"),
            root: copied("/nix/store/c-shikumi-keyed"),
        }
    }

    fn arguments(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|a| a.to_str().unwrap().to_owned())
            .collect()
    }

    fn evaluation(cache: &Cache, request: &Request<'_>) -> Command {
        let inputs = inputs(cache);
        command(
            Path::new("/bin/nix-instantiate"),
            cache,
            &Run::Evaluate {
                inputs: &inputs,
                request,
            },
        )
    }

    fn settings(args: &[String]) -> Vec<(String, String)> {
        args.windows(3)
            .filter(|w| w[0] == "--option")
            .map(|w| (w[1].clone(), w[2].clone()))
            .collect()
    }

    // the store evaluation reads is the private one under the evaluation's own database; builds go
    // to `dummy://`, which builds nothing; and every setting of the table is on the command line,
    // pure mode among them.
    #[test]
    fn evaluation_can_reach_no_store_and_nothing_that_builds_or_runs() {
        let base = tempfile::tempdir().unwrap();
        let cache = cache(base.path());
        let args = arguments(&evaluation(&cache, &Request::Plan {}));
        let has = |pair: [&str; 2]| args.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]);
        assert!(has(["--store", "dummy://"]), "{args:?}");
        let own = cache.store_url(Some(Path::new("/state")));
        assert!(has(["--eval-store", own.to_str().unwrap()]), "{args:?}");
        let expected: Vec<(String, String)> = SETTINGS
            .iter()
            .map(|setting| (setting.name.to_owned(), setting.value.to_owned()))
            .collect();
        assert_eq!(settings(&args), expected);
        assert!(has(["--log-format", "raw"]), "{args:?}");
        assert!(args.contains(&"--show-trace".to_owned()), "{args:?}");
        assert!(!args.contains(&"--read-write-mode".to_owned()), "{args:?}");
    }

    // the copy keeps every setting but those the table says it changes, may read only the
    // directories it copies from, and writes only to the private store's own database.
    #[test]
    fn the_copy_keeps_every_refusal_but_pure_mode() {
        let base = tempfile::tempdir().unwrap();
        let cache = cache(base.path());
        let copies = base.path().join("copies.json");
        let stage = base.path().join("stage");
        let args = arguments(&command(
            Path::new("/bin/nix-instantiate"),
            &cache,
            &Run::Copy {
                copies: &copies,
                sources: &[&stage],
            },
        ));
        let expected: Vec<(String, String)> = SETTINGS
            .iter()
            .map(|setting| {
                let value = setting.copying.unwrap_or(setting.value);
                (setting.name.to_owned(), value.to_owned())
            })
            .collect();
        assert_eq!(settings(&args), expected);
        assert!(expected.contains(&("pure-eval".to_owned(), "false".to_owned())));
        assert!(args.contains(&"--read-write-mode".to_owned()));
        let main = cache.store_url(None);
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--eval-store" && w[1] == main.to_str().unwrap())
        );
        let readable: Vec<&String> = args
            .windows(2)
            .filter(|w| w[0] == "-I")
            .map(|w| &w[1])
            .collect();
        assert_eq!(
            readable,
            [
                &format!("shikumi-copy-0={}", stage.display()),
                &format!("shikumi-copy-1={}", copies.display())
            ]
        );
    }

    #[test]
    fn every_request_runs_with_the_same_refusals() {
        let base = tempfile::tempdir().unwrap();
        let cache = cache(base.path());
        let names = [Project {
            name: "a".to_owned(),
            file: RelativePath::new("a/project.nix").unwrap(),
        }];
        let pinned = |request: &Request<'_>| {
            let args = arguments(&evaluation(&cache, request));
            let request_at = args.iter().position(|a| a == "request").unwrap();
            args[..request_at].to_vec()
        };
        let planned = pinned(&Request::Plan {});
        let catalog = RelativePath::new("catalog/catalog.nix").unwrap();
        assert_eq!(pinned(&Request::Evaluate(&names)), planned);
        assert_eq!(pinned(&Request::Locate(&names[0])), planned);
        for request in [
            Request::Catalog {
                file: &catalog,
                system: "x86_64-linux",
            },
            Request::Context {
                file: &catalog,
                system: "x86_64-linux",
            },
        ] {
            assert_eq!(pinned(&request), planned);
        }
    }

    // nothing of the caller's environment reaches the evaluator: only an empty configuration
    // directory, no user configuration files, and its own cache and temporary directories.
    #[test]
    fn the_child_gets_no_environment_and_no_configuration() {
        let base = tempfile::tempdir().unwrap();
        let cache = cache(base.path());
        let command = evaluation(&cache, &Request::Plan {});
        let passed: BTreeMap<String, String> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_str().unwrap().to_owned(),
                    value.unwrap().to_str().unwrap().to_owned(),
                )
            })
            .collect();
        let names: Vec<&str> = passed.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "NIX_CONF_DIR",
                "NIX_USER_CONF_FILES",
                "TMPDIR",
                "XDG_CACHE_HOME"
            ]
        );
        assert_eq!(passed["NIX_USER_CONF_FILES"], "");
        let configuration = PathBuf::from(&passed["NIX_CONF_DIR"]);
        assert_eq!(std::fs::read_dir(configuration).unwrap().count(), 0);
        assert_eq!(command.get_current_dir(), Some(Path::new("/")));
    }
}
