//! what the declaration layer's Nix and the registry's Rust both have to name: the files the layer
//! reads in a repository, what each is called with, the fields it reads by name, and the frames the
//! layer puts in an evaluator's trace for `failure.rs` to read. this is the one definition of each. `fixtures/layer.json` is this
//! table written out, which a test holds to it, and the layer reads the same file beside its own Nix,
//! `nix/declarations/layer.json`, which the `declarations` check holds to this one. the checker's
//! messages and the reference page read the table from here.

use serde::Serialize;

use crate::vocabulary::{ManifestField, names};

/// a file the layer reads in a repository: its name, and the arguments it's called with when it's
/// a function, which it takes by name.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LayerFile {
    pub name: &'static str,
    pub arguments: &'static [&'static str],
}

/// the manifest, at the repository root.
pub const MANIFEST: LayerFile = LayerFile {
    name: "shikumi.nix",
    arguments: &["lib", "types"],
};

/// each listed project's declaration, in its directory.
pub const DECLARATION: LayerFile = LayerFile {
    name: "project.nix",
    arguments: &["lib", "shikumi"],
};

/// the catalog, in the directory the manifest names for it.
pub const CATALOG: LayerFile = LayerFile {
    name: "catalog.nix",
    arguments: &["lib"],
};

/// the extension of every Nix file the layer reads, and so of every file in the keyed set an
/// evaluation may import: each file above ends with it, and the context cache copies only files
/// that do.
pub const NIX_EXTENSION: &str = "nix";

/// what the files an evaluation may read are, as every message about one it may not read says it.
/// a macro, so the failure kind's and the finding code's meanings, which are constants, can put it
/// in their own sentences.
#[macro_export]
macro_rules! keyed_files {
    () => {
        "a `.nix` file Git tracks inside a listed project's or the catalog's directory, reached through no symlink"
    };
}

/// the names of what the producer copies into its store for an evaluation to read, which the
/// checker and the gate's harness read too.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreNames {
    /// a copy of keyed files: one part of a composition.
    pub keyed: &'static str,
    /// the copy of the layer's own Nix.
    pub layer: &'static str,
    /// the file every keyed copy holds at its root beside the keyed files, whose value is that
    /// root as a path. it's Shikumi's, written into the copy, never a repository's.
    pub root_marker: &'static str,
}

pub const STORE_NAMES: StoreNames = StoreNames {
    keyed: "shikumi-keyed",
    layer: "shikumi-layer",
    root_marker: ".shikumi-root.nix",
};

/// a builtin the layer's scope withholds, and whether an evaluator also gives it a global name
/// without `__` in front.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Withheld {
    pub name: &'static str,
    /// the scope shadows the `__` name of every withheld builtin, and the bare one only where an
    /// evaluator gives it: a scope's name outranks an attribute `with` brings in, so a bare name
    /// no evaluator defines would take `pkgs.fetchurl` from nixpkgs' `with pkgs; fetchurl`.
    pub global: bool,
}

/// the builtins every file a declaration's evaluation reaches is evaluated without, nixpkgs
/// included, by the layer's scope: every one that reaches a fetcher, on any of the implementations
/// Shikumi runs on, whether or not that implementation or pure and restricted mode refuse the call
/// itself, since Determinate Nix's `getFlake` takes a locked reference to any flake, commit or
/// directory on the host. the scope also refuses a lookup-path entry with a scheme, which reaches
/// `fetchTree`'s fetchers through `findFile` and `<...>`. the producer reads reaching one, the
/// evaluator's missing-attribute error, as a refused read. which names are global is each
/// implementation's `RegisterPrimOp` name or builtins page, as the route table has them.
pub const WITHHELD: &[Withheld] = &[
    Withheld {
        name: "getFlake",
        global: false,
    },
    Withheld {
        name: "fetchTree",
        global: true,
    },
    Withheld {
        name: "fetchGit",
        global: true,
    },
    Withheld {
        name: "fetchTarball",
        global: true,
    },
    Withheld {
        name: "fetchurl",
        global: false,
    },
    Withheld {
        name: "fetchMercurial",
        global: true,
    },
    Withheld {
        name: "fetchClosure",
        global: false,
    },
];

/// the fields the layer's Nix reads by name to know which files to read and how: the manifest's
/// projects and catalog, the catalog entry's directory and systems, and the catalog's packages and
/// overrides, with each override's build. the names are the vocabulary's; nix-unit holds the Nix
/// that reads them to this table.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reads {
    pub projects: &'static str,
    pub catalog: &'static str,
    pub directory: &'static str,
    pub systems: &'static str,
    pub packages: &'static str,
    pub overrides: &'static str,
    pub package: &'static str,
}

pub const READS: Reads = Reads {
    projects: ManifestField::Projects.name(),
    catalog: ManifestField::Catalog.name(),
    directory: names::DIRECTORY,
    systems: names::SYSTEMS,
    packages: names::PACKAGES,
    overrides: names::OVERRIDES,
    package: names::PACKAGE,
};

/// the frames the layer puts in an evaluator's trace, each a fixed text followed, for the project
/// and the field, by a space and JSON.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frames {
    /// around the first forcing of every value a user wrote.
    pub value: &'static str,
    /// around every evaluation of one project's declaration, followed by its name.
    pub project: &'static str,
    /// around writing out one string of a located declaration, followed by its path and position.
    pub field: &'static str,
    /// the layer's file that puts the value frame on, whose own frame is the forced value's.
    pub forced_file: &'static str,
}

pub const FRAMES: Frames = Frames {
    value: "shikumi-value",
    project: "shikumi-project",
    field: "shikumi-field",
    forced_file: "forced.nix",
};

impl LayerFile {
    /// the arguments as a sentence names them: `` `lib` and `shikumi` ``.
    pub fn called_with(&self) -> String {
        let named: Vec<String> = self.arguments.iter().map(|a| format!("`{a}`")).collect();
        match named.split_last() {
            None => "nothing".to_owned(),
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        }
    }

    /// the function head a file written as a function takes: `{ lib, shikumi }`.
    pub fn head(&self) -> String {
        format!("{{ {} }}", self.arguments.join(", "))
    }
}

/// the table as the layer reads it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Written {
    manifest: LayerFile,
    declaration: LayerFile,
    catalog: LayerFile,
    reads: Reads,
    frames: Frames,
    store_names: StoreNames,
    withheld: &'static [Withheld],
}

/// the table written out, as `fixtures/layer.json` holds it.
pub fn json() -> String {
    crate::contract::render_json(&Written {
        manifest: MANIFEST,
        declaration: DECLARATION,
        catalog: CATALOG,
        reads: READS,
        frames: FRAMES,
        store_names: STORE_NAMES,
        withheld: WITHHELD,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // the layer reads its file names, arguments and frames from a copy of the tracked JSON, so this
    // and the `declarations` check hold the two halves to one table.
    #[test]
    fn the_layer_reads_this_table() {
        let tracked = include_str!("../fixtures/layer.json");
        assert!(
            tracked == json(),
            "crates/registry/fixtures/layer.json is stale; it, and nix/declarations/layer.json, have to be:\n{}",
            json()
        );
    }

    // a layer file without the extension would be left out of every store copy, and no
    // evaluation could read it.
    #[test]
    fn every_file_the_layer_reads_is_keyed() {
        for file in [MANIFEST, DECLARATION, CATALOG] {
            assert!(
                file.name.ends_with(&format!(".{NIX_EXTENSION}")),
                "{}",
                file.name
            );
        }
    }

    #[test]
    fn arguments_read_as_a_sentence_and_a_head() {
        assert_eq!(MANIFEST.called_with(), "`lib` and `types`");
        assert_eq!(DECLARATION.head(), "{ lib, shikumi }");
    }
}
