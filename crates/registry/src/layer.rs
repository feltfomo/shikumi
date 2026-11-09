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
struct Written {
    manifest: LayerFile,
    declaration: LayerFile,
    catalog: LayerFile,
    reads: Reads,
    frames: Frames,
}

/// the table written out, as `fixtures/layer.json` holds it.
pub fn json() -> String {
    crate::contract::render_json(&Written {
        manifest: MANIFEST,
        declaration: DECLARATION,
        catalog: CATALOG,
        reads: READS,
        frames: FRAMES,
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

    #[test]
    fn arguments_read_as_a_sentence_and_a_head() {
        assert_eq!(MANIFEST.called_with(), "`lib` and `types`");
        assert_eq!(DECLARATION.head(), "{ lib, shikumi }");
    }
}
