//! a seeded generator for Cargo workspaces of tunable shape. the generated crates use only the
//! standard library, so they build offline and inside a build sandbox.
//!
//! crate `c0000` is the hub at layer 0. the other crates fill layers 1 to `depth` in contiguous
//! blocks, and each one depends on a crate in the layer just below plus a few random lower crates,
//! so every crate reaches the hub and the longest dependency chain is exactly `depth`. the optional
//! large crate sits on top and nothing depends on it.

use std::borrow::Cow;
use std::fmt::Write;
use std::num::NonZeroU32;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use super::tree::Tree;
use crate::contract::ContractError;
use crate::rng::SplitMix64;

/// the shape of a generated workspace. the types hold each count above zero; [`Self::validate`]
/// holds the rules between counts, and a record checks them when it's read.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = crate::contract::require_all_properties)]
pub struct RustWorkspaceParams {
    /// crates besides the large one, the hub included. must exceed `depth`.
    pub crates: NonZeroU32,
    /// the longest dependency chain, in edges from a top-layer crate down to the hub.
    pub depth: NonZeroU32,
    /// the most direct dependencies a crate takes: one from the layer below, the rest from any
    /// lower layer.
    pub dependencies_per_crate: NonZeroU32,
    pub modules_per_crate: NonZeroU32,
    pub functions_per_module: NonZeroU32,
    /// the share of functions that are generic, so their code is generated in the calling crate.
    pub generic_percent: Percent,
    /// one crate much larger than the rest, for work below the crate; null for none.
    #[serde(deserialize_with = "Option::deserialize")]
    pub large_crate: Option<LargeCrateParams>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LargeCrateParams {
    pub modules: NonZeroU32,
    pub functions_per_module: NonZeroU32,
    /// how many of the other crates it depends on directly; at most `crates`.
    pub dependencies: NonZeroU32,
}

/// a whole percentage, 0 to 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Percent(u8);

impl Percent {
    pub fn new(value: u8) -> Result<Self, ContractError> {
        if value <= 100 {
            Ok(Self(value))
        } else {
            Err(ContractError(format!("{value} is not a percentage")))
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Percent {
    type Error = ContractError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Percent> for u8 {
    fn from(value: Percent) -> Self {
        value.0
    }
}

impl JsonSchema for Percent {
    fn schema_name() -> Cow<'static, str> {
        "Percent".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "integer", "minimum": 0, "maximum": 100 })
    }
}

impl RustWorkspaceParams {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.crates <= self.depth {
            return Err(ContractError(format!(
                "{} crates can't fill {} layers above the hub; use at least depth + 1",
                self.crates, self.depth
            )));
        }
        if let Some(large) = &self.large_crate
            && large.dependencies > self.crates
        {
            return Err(ContractError(format!(
                "the large crate's dependencies must be at most the {} other crates",
                self.crates
            )));
        }
        Ok(())
    }

    /// the units a build of this workspace compiles: every crate, the large one included.
    pub fn unit_count(&self) -> u32 {
        self.crates.get() + u32::from(self.large_crate.is_some())
    }
}

/// a scenario's change to the generated source. each edit rewrites exactly one file, and the same
/// edit always produces the same bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// changes the body of a private function in a leaf: the large crate when there is one,
    /// otherwise the last crate of the top layer.
    OneFunction,
    /// makes the hub's public `api` generic over `Into<u64>`. every caller still compiles, but the
    /// hub's interface changes, so everything that depends on it has to be checked again.
    PublicSignature,
    /// adds a notes file to the hub's crate directory, which rustc never reads. a unit whose input
    /// is the crate directory rebuilds and produces the same bytes, so early cutoff has to stop
    /// the rebuild there. it's the only edit that can: rustc 1.99 hashes the raw bytes of every
    /// source file it reads into the crate's metadata, so changing a read file, even a comment of
    /// the same length, changes the rlib.
    IdenticalOutput,
}

/// what makes a unit rebuild, which decides how many units an edit costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracking {
    /// Cargo's rule: a crate rebuilds when a file rustc read for it changed, or when a crate it
    /// depends on rebuilt. a file rustc never read isn't tracked.
    ReadFiles,
    /// a unit rebuilds when any file in its crate directory changed, or when the bytes of an
    /// input's output changed. a dependent's bytes change whenever its dependency's metadata does,
    /// since rustc records the dependency's crate hash in it, so only an unchanged output stops the
    /// rebuild.
    UnitOutputs,
}

/// the edits applied to a workspace so far. a set, so applying one twice or in another order can't
/// produce a different tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edits {
    one_function: bool,
    public_signature: bool,
    identical_output: bool,
}

impl Edits {
    pub fn with(mut self, edit: Edit) -> Self {
        match edit {
            Edit::OneFunction => self.one_function = true,
            Edit::PublicSignature => self.public_signature = true,
            Edit::IdenticalOutput => self.identical_output = true,
        }
        self
    }

    pub fn contains(self, edit: Edit) -> bool {
        match edit {
            Edit::OneFunction => self.one_function,
            Edit::PublicSignature => self.public_signature,
            Edit::IdenticalOutput => self.identical_output,
        }
    }
}

/// one file's new contents, relative to the workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub path: String,
    pub contents: String,
}

#[derive(Debug, Clone)]
pub struct RustWorkspace {
    seed: u64,
    params: RustWorkspaceParams,
    crates: Vec<CrateModel>,
}

#[derive(Debug, Clone)]
struct CrateModel {
    name: String,
    deps: Vec<usize>,
    modules: Vec<Vec<Function>>,
    kernel_multiplier: u64,
}

#[derive(Debug, Clone)]
struct Function {
    generic: bool,
    multiplier: u64,
    rotation: u32,
    call: Call,
}

#[derive(Debug, Clone, Copy)]
enum Call {
    None,
    SameModule {
        function: usize,
    },
    EarlierModule {
        module: usize,
        function: usize,
    },
    Dependency {
        krate: usize,
        module: usize,
        function: usize,
    },
}

const HUB: usize = 0;

impl RustWorkspace {
    pub fn generate(seed: u64, params: RustWorkspaceParams) -> Result<Self, ContractError> {
        params.validate()?;
        let mut rng = SplitMix64::new(seed);
        let count = params.crates.get() as usize;
        let depth = params.depth.get() as usize;
        let layer_start = |layer: usize| -> usize {
            // crate i >= 1 sits in layer 1 + (i - 1) * depth / (count - 1); invert that to find the
            // first crate of each layer.
            if layer == 0 {
                0
            } else {
                1 + ((layer - 1) * (count - 1)).div_ceil(depth)
            }
        };

        let mut crates: Vec<CrateModel> = Vec::with_capacity(count + 1);
        for index in 0..count {
            let deps = if index == HUB {
                Vec::new()
            } else {
                let layer = 1 + (index - 1) * depth / (count - 1);
                let below = layer_start(layer - 1)..layer_start(layer);
                let mut deps = vec![below.start + rng.below(below.len())];
                for _ in 1..params.dependencies_per_crate.get() {
                    deps.push(rng.below(below.end));
                }
                deps.sort_unstable();
                deps.dedup();
                deps
            };
            let model = build_crate(
                &mut rng,
                &crates,
                format!("c{index:04}"),
                deps,
                params.modules_per_crate.get(),
                params.functions_per_module.get(),
                params.generic_percent,
            );
            crates.push(model);
        }
        if let Some(large) = &params.large_crate {
            let mut deps: Vec<usize> = (0..large.dependencies.get())
                .map(|_| rng.below(count))
                .collect();
            deps.sort_unstable();
            deps.dedup();
            let model = build_crate(
                &mut rng,
                &crates,
                "large".to_owned(),
                deps,
                large.modules.get(),
                large.functions_per_module.get(),
                params.generic_percent,
            );
            crates.push(model);
        }
        Ok(Self {
            seed,
            params,
            crates,
        })
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn params(&self) -> &RustWorkspaceParams {
        &self.params
    }

    /// the workspace with no edits applied.
    pub fn render(&self) -> Tree {
        let mut tree = Tree::default();
        tree.insert(
            "Cargo.toml".to_owned(),
            "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n".to_owned(),
        );
        for krate in &self.crates {
            let dir = format!("crates/{}", krate.name);
            tree.insert(format!("{dir}/Cargo.toml"), self.manifest(krate));
            tree.insert(format!("{dir}/src/lib.rs"), self.lib(krate, false, false));
            for (module, functions) in krate.modules.iter().enumerate() {
                tree.insert(format!("{dir}/src/m{module:04}.rs"), self.module(functions));
            }
        }
        tree
    }

    /// the one file `edit` changes, rendered with `applied` edits in it too, so edits stack in
    /// whatever order they're applied.
    pub fn patch(&self, edit: Edit, applied: Edits) -> Patch {
        let edits = applied.with(edit);
        let target = self.target(edit);
        let krate = &self.crates[target];
        if edit == Edit::IdenticalOutput {
            return Patch {
                path: format!("crates/{}/NOTES.md", krate.name),
                contents: format!("Notes on {}, which the build never reads.\n", krate.name),
            };
        }
        let contents = self.lib(
            krate,
            target == self.target(Edit::OneFunction) && edits.contains(Edit::OneFunction),
            target == self.target(Edit::PublicSignature) && edits.contains(Edit::PublicSignature),
        );
        Patch {
            path: format!("crates/{}/src/lib.rs", krate.name),
            contents,
        }
    }

    /// how many units a build after `edit` must compile under `tracking`: the edited crate if the
    /// edit reaches what it tracks, and every crate that reaches one whose output changed.
    pub fn units_rebuilt_by(&self, edit: Edit, tracking: Tracking) -> u32 {
        let target = self.target(edit);
        match (edit, tracking) {
            // Cargo never tracks a file rustc didn't read.
            (Edit::IdenticalOutput, Tracking::ReadFiles) => return 0,
            // the edited unit rebuilds, and its unchanged output stops the rebuild there.
            (Edit::IdenticalOutput, Tracking::UnitOutputs) => return 1,
            // the output changes, so every crate that reaches it rebuilds.
            (Edit::OneFunction | Edit::PublicSignature, _) => {}
        }
        // dependencies always have lower indices, so one forward pass sees every crate's
        // dependencies before the crate itself.
        let mut affected = vec![false; self.crates.len()];
        for (index, krate) in self.crates.iter().enumerate() {
            affected[index] = index == target || krate.deps.iter().any(|&d| affected[d]);
        }
        affected.iter().filter(|&&a| a).count() as u32
    }

    /// the longest dependency chain in edges, the large crate's included.
    pub fn graph_depth(&self) -> u32 {
        let mut chain = vec![0u32; self.crates.len()];
        for (index, krate) in self.crates.iter().enumerate() {
            chain[index] = krate.deps.iter().map(|&d| chain[d] + 1).max().unwrap_or(0);
        }
        chain.into_iter().max().unwrap_or(0)
    }

    /// each crate's name with the indices of the crates it depends on directly, in dependency
    /// order: every crate comes after its dependencies.
    pub fn crate_graph(&self) -> Vec<(&str, &[usize])> {
        self.crates
            .iter()
            .map(|krate| (krate.name.as_str(), krate.deps.as_slice()))
            .collect()
    }

    fn target(&self, edit: Edit) -> usize {
        match edit {
            Edit::OneFunction => self.crates.len() - 1,
            Edit::PublicSignature | Edit::IdenticalOutput => HUB,
        }
    }

    fn manifest(&self, krate: &CrateModel) -> String {
        let mut out = format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n",
            krate.name
        );
        if !krate.deps.is_empty() {
            out.push_str("\n[dependencies]\n");
            for &dep in &krate.deps {
                let name = &self.crates[dep].name;
                writeln!(out, "{name} = {{ path = \"../{name}\" }}").unwrap();
            }
        }
        out
    }

    fn lib(&self, krate: &CrateModel, edit_kernel: bool, generic_api: bool) -> String {
        let mut out = format!(
            "// generated by shikumi-bench from seed {}; regenerate rather than edit.\n\n",
            self.seed
        );
        for module in 0..krate.modules.len() {
            writeln!(out, "pub mod m{module:04};").unwrap();
        }
        // the edited kernel differs only in its constant: the body changes, nothing else does.
        let kernel = krate.kernel_multiplier ^ (u64::from(edit_kernel) << 1);
        writeln!(
            out,
            "\nfn kernel(x: u64) -> u64 {{\n    x.wrapping_mul({kernel:#x}).rotate_left(7)\n}}\n"
        )
        .unwrap();
        let last_module = krate.modules.len() - 1;
        let last_function = krate.modules[last_module].len() - 1;
        if generic_api {
            out.push_str(
                "pub fn api<T: Into<u64>>(x: T) -> u64 {\n    let v = kernel(x.into());\n",
            );
        } else {
            out.push_str("pub fn api(x: u64) -> u64 {\n    let v = kernel(x);\n");
        }
        writeln!(
            out,
            "    let v = v ^ m{last_module:04}::f{last_function:04}(v);"
        )
        .unwrap();
        out.push_str("    v");
        for &dep in &krate.deps {
            write!(out, " ^ {}::api(v)", self.crates[dep].name).unwrap();
        }
        out.push_str("\n}\n");
        out
    }

    fn module(&self, functions: &[Function]) -> String {
        let mut out = String::with_capacity(functions.len() * 160);
        for (index, function) in functions.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let Function {
                generic,
                multiplier,
                rotation,
                call,
            } = *function;
            if generic {
                writeln!(
                    out,
                    "pub fn f{index:04}<T: Into<u64>>(x: T) -> u64 {{\n    let x: u64 = x.into();"
                )
                .unwrap();
            } else {
                writeln!(out, "pub fn f{index:04}(x: u64) -> u64 {{").unwrap();
            }
            writeln!(
                out,
                "    let v = x.wrapping_mul({multiplier:#x}).rotate_left({rotation});"
            )
            .unwrap();
            match call {
                Call::None => out.push_str("    v\n"),
                Call::SameModule { function } => {
                    writeln!(out, "    v ^ f{function:04}(v)").unwrap();
                }
                Call::EarlierModule { module, function } => {
                    writeln!(out, "    v ^ crate::m{module:04}::f{function:04}(v)").unwrap();
                }
                Call::Dependency {
                    krate,
                    module,
                    function,
                } => {
                    let name = &self.crates[krate].name;
                    writeln!(out, "    v ^ {name}::m{module:04}::f{function:04}(v)").unwrap();
                }
            }
            out.push_str("}\n");
        }
        out
    }
}

fn build_crate(
    rng: &mut SplitMix64,
    existing: &[CrateModel],
    name: String,
    deps: Vec<usize>,
    modules: u32,
    functions_per_module: u32,
    generic_percent: Percent,
) -> CrateModel {
    let mut module_list = Vec::with_capacity(modules as usize);
    for module in 0..modules as usize {
        let mut functions = Vec::with_capacity(functions_per_module as usize);
        for function in 0..functions_per_module as usize {
            // calls only point at earlier functions, earlier modules or lower crates, so the call
            // graph is acyclic and every function is reachable from a later one.
            let call = match rng.below(4) {
                0 | 1 if function > 0 => Call::SameModule {
                    function: rng.below(function),
                },
                2 if module > 0 => {
                    let target = rng.below(module);
                    Call::EarlierModule {
                        module: target,
                        function: rng.below(functions_per_module as usize),
                    }
                }
                3 if !deps.is_empty() => {
                    let krate = deps[rng.below(deps.len())];
                    let target = &existing[krate].modules;
                    let module = rng.below(target.len());
                    Call::Dependency {
                        krate,
                        module,
                        function: rng.below(target[module].len()),
                    }
                }
                _ => Call::None,
            };
            functions.push(Function {
                generic: rng.percent(generic_percent.get()),
                multiplier: rng.next_u64() | 1,
                rotation: 1 + rng.below(63) as u32,
                call,
            });
        }
        module_list.push(functions);
    }
    CrateModel {
        name,
        deps,
        modules: module_list,
        kernel_multiplier: rng.next_u64() | 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Sha256;

    fn nz(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    fn small() -> RustWorkspaceParams {
        RustWorkspaceParams {
            crates: nz(6),
            depth: nz(3),
            dependencies_per_crate: nz(2),
            modules_per_crate: nz(3),
            functions_per_module: nz(5),
            generic_percent: Percent::new(30).unwrap(),
            large_crate: Some(LargeCrateParams {
                modules: nz(8),
                functions_per_module: nz(6),
                dependencies: nz(3),
            }),
        }
    }

    fn render(seed: u64) -> Tree {
        RustWorkspace::generate(seed, small()).unwrap().render()
    }

    // a seed in a record must regenerate the same fixture after the generator changes, so the
    // bytes are pinned here. a deliberate change to the output updates these digests and is a
    // change to every recorded fixture.
    #[test]
    fn seed_regenerates_the_pinned_fixture() {
        let ws = RustWorkspace::generate(42, small()).unwrap();
        let digest = |s: &str| Sha256::of(s.as_bytes()).to_string();
        assert_eq!(
            ws.render().digest().to_string(),
            "8a30fc225ca296280317d13b99ac0fc50e575f5002caacf82d192980eb474b2a",
            "the base tree for seed 42 changed"
        );
        let one = ws.patch(Edit::OneFunction, Edits::default());
        let both = ws.patch(
            Edit::PublicSignature,
            Edits::default().with(Edit::OneFunction),
        );
        assert_eq!(
            digest(&one.contents),
            "def7c0b6a97d5e3241ea712652a8bc0fcfcbba7e59804b4542a83391d001bfd5",
            "the one-function edit changed"
        );
        assert_eq!(
            digest(&both.contents),
            "86e8f87493e5a9a1a5a56d0399cd320040b0fa1e5d484c2649a501a21ede5644",
            "the signature edit changed"
        );
    }

    #[test]
    fn different_seed_gives_different_bytes() {
        let a = render(42);
        let b = render(43);
        assert_ne!(a, b);
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn written_tree_matches_the_tree_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let tree = render(9);
        tree.write_new(dir.path()).unwrap();
        for (path, contents) in tree.files() {
            assert_eq!(
                std::fs::read_to_string(dir.path().join(path)).unwrap(),
                contents
            );
        }
        assert!(
            tree.write_new(dir.path()).is_err(),
            "wrote over a non-empty directory"
        );
    }

    #[test]
    fn layers_give_the_requested_depth_and_reach_the_hub() {
        for (crates, depth) in [(2, 1), (6, 3), (10, 4), (31, 30), (100, 7)] {
            let params = RustWorkspaceParams {
                crates: nz(crates),
                depth: nz(depth),
                large_crate: None,
                ..small()
            };
            let ws = RustWorkspace::generate(5, params).unwrap();
            // longest chain to the hub, computed over the dependency edges the generator wrote.
            let mut chain = vec![0u32; ws.crates.len()];
            for (index, krate) in ws.crates.iter().enumerate().skip(1) {
                assert!(!krate.deps.is_empty(), "crate {index} reaches nothing");
                assert!(krate.deps.iter().all(|&d| d < index), "edge points upward");
                chain[index] = 1 + krate.deps.iter().map(|&d| chain[d]).max().unwrap();
            }
            assert_eq!(
                chain.iter().max().copied(),
                Some(depth),
                "{crates} crates, depth {depth}"
            );
        }
    }

    #[test]
    fn each_edit_rewrites_its_one_target_file() {
        let ws = RustWorkspace::generate(3, small()).unwrap();
        let base = ws.render();
        let one = ws.patch(Edit::OneFunction, Edits::default());
        let sig = ws.patch(Edit::PublicSignature, Edits::default());
        assert_eq!(one.path, "crates/large/src/lib.rs");
        assert_eq!(sig.path, "crates/c0000/src/lib.rs");
        for patch in [&one, &sig] {
            assert_ne!(base.get(&patch.path), Some(patch.contents.as_str()));
        }
        assert!(sig.contents.contains("pub fn api<T: Into<u64>>(x: T)"));
        // stacking: the signature patch with the other edit applied doesn't undo it, because the
        // two edits never share a file.
        let stacked = ws.patch(
            Edit::PublicSignature,
            Edits::default().with(Edit::OneFunction),
        );
        assert_eq!(stacked, sig);
        assert_eq!(
            Edits::default()
                .with(Edit::OneFunction)
                .with(Edit::OneFunction),
            Edits::default().with(Edit::OneFunction)
        );
    }

    #[test]
    fn rebuild_counts_follow_the_dependency_model() {
        let ws = RustWorkspace::generate(3, small()).unwrap();
        // the large crate is a leaf, and every crate reaches the hub.
        for tracking in [Tracking::ReadFiles, Tracking::UnitOutputs] {
            assert_eq!(ws.units_rebuilt_by(Edit::OneFunction, tracking), 1);
            assert_eq!(ws.units_rebuilt_by(Edit::PublicSignature, tracking), 7);
        }
        // the unread file: Cargo never sees it, a unit sees it and cuts off at the hub.
        assert_eq!(
            ws.units_rebuilt_by(Edit::IdenticalOutput, Tracking::ReadFiles),
            0
        );
        assert_eq!(
            ws.units_rebuilt_by(Edit::IdenticalOutput, Tracking::UnitOutputs),
            1
        );
        let no_large = RustWorkspace::generate(
            3,
            RustWorkspaceParams {
                large_crate: None,
                ..small()
            },
        )
        .unwrap();
        assert_eq!(
            no_large.units_rebuilt_by(Edit::OneFunction, Tracking::UnitOutputs),
            1
        );
        assert_eq!(
            no_large.units_rebuilt_by(Edit::PublicSignature, Tracking::UnitOutputs),
            6
        );
    }

    #[test]
    fn graph_depth_counts_the_large_crate_on_top() {
        let ws = RustWorkspace::generate(3, small()).unwrap();
        let without = RustWorkspace::generate(
            3,
            RustWorkspaceParams {
                large_crate: None,
                ..small()
            },
        )
        .unwrap();
        assert_eq!(without.graph_depth(), 3);
        // the large crate depends on some of the others, so it sits one edge above one of them.
        assert!((1..=4).contains(&ws.graph_depth()), "{}", ws.graph_depth());
        let graph = ws.crate_graph();
        assert_eq!(graph.len(), 7);
        assert!(
            graph
                .iter()
                .enumerate()
                .all(|(i, (_, deps))| deps.iter().all(|&d| d < i))
        );
    }

    #[test]
    fn the_identical_output_edit_adds_one_unread_file_to_the_hub() {
        let ws = RustWorkspace::generate(3, small()).unwrap();
        let patch = ws.patch(Edit::IdenticalOutput, Edits::default());
        assert_eq!(patch.path, "crates/c0000/NOTES.md");
        assert!(ws.render().get(&patch.path).is_none());
        assert_eq!(
            patch,
            ws.patch(
                Edit::IdenticalOutput,
                Edits::default().with(Edit::PublicSignature)
            )
        );
    }

    #[test]
    fn rejects_shapes_it_cannot_build() {
        let bad = [
            RustWorkspaceParams {
                crates: nz(3),
                depth: nz(3),
                ..small()
            },
            RustWorkspaceParams {
                large_crate: Some(LargeCrateParams {
                    modules: nz(1),
                    functions_per_module: nz(1),
                    dependencies: nz(7),
                }),
                ..small()
            },
        ];
        for params in bad {
            assert!(RustWorkspace::generate(1, params).is_err());
        }
        assert!(Percent::new(101).is_err());
    }
}
