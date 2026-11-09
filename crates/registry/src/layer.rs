//! what the declaration layer's Nix and the registry's Rust both have to name: the files the layer
//! reads in a repository, what each is called with, and the frames the layer puts in an evaluator's
//! trace for `failure.rs` to read. this is the one definition of each. `fixtures/layer.json` is this
//! table written out, which a test holds to it, and the layer reads the same file beside its own Nix,
//! `nix/declarations/layer.json`, which the `declarations` check holds to this one. the checker's
//! messages and the reference page read the table from here.

use serde::Serialize;

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
    frames: Frames,
}

/// the table written out, as `fixtures/layer.json` holds it.
pub fn json() -> String {
    crate::contract::render_json(&Written {
        manifest: MANIFEST,
        declaration: DECLARATION,
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
