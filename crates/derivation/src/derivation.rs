//! the derivation model, its ATerm serialization and the paths that follow from it: the hash
//! modulo, the output paths and the `.drv` file's own text-addressed path.
//!
//! the rules follow `Derivation::unparse`, `hashDerivationModulo` and `writeDerivation` in CppNix
//! 2.35.3 `src/libstore/derivations.cc`, which Lix 2.95.4 and 2.96.0 match in
//! `lix/libstore/derivations.cc`. only input-addressed and fixed-output derivations exist here; the
//! dynamic, floating content-addressed and impure kinds aren't shared by every implementation.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::address::{ContentAddress, FixedHash, output_path_name};
use crate::{PathError, Sha256, StoreDir, StoreName, StorePath};

/// a derivation output's name, such as `out` or `dev`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutputName(String);

impl OutputName {
    pub fn new(name: impl Into<String>) -> Result<Self, DerivationError> {
        let name = name.into();
        // an output name becomes part of a path name, and `drv` is refused by the `derivation`
        // builtin of every implementation because `<name>-drv` reads as a `.drv` file.
        if let Err(source) = StoreName::new(name.as_str()) {
            return Err(DerivationError::OutputName { name, source });
        }
        if name == "drv" {
            return Err(DerivationError::DrvOutput);
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// ordering and equality are the string's, so a map keyed by output name is looked up by `&str`.
impl Borrow<str> for OutputName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OutputName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outputs {
    /// outputs whose paths follow from the derivation's inputs.
    InputAddressed(BTreeSet<OutputName>),
    /// one output, `out`, whose path follows from its declared content hash.
    Fixed(FixedHash),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InputDerivation {
    outputs: BTreeSet<OutputName>,
    /// what the input contributes to this derivation's hashes in place of its path. for a fixed
    /// output that's a hash of its content and path alone, so how it's fetched never reaches
    /// dependents; its output set is always `out`, the only output it has.
    modulo: Sha256,
}

/// a derivation as Shikumi writes it, before its paths are known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derivation {
    name: StoreName,
    outputs: Outputs,
    system: String,
    builder: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    input_drvs: BTreeMap<StorePath, InputDerivation>,
    input_srcs: BTreeSet<StorePath>,
}

impl Derivation {
    pub fn new(
        name: StoreName,
        outputs: Outputs,
        system: impl Into<String>,
        builder: impl Into<String>,
    ) -> Self {
        Self {
            name,
            outputs,
            system: system.into(),
            builder: builder.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            input_drvs: BTreeMap::new(),
            input_srcs: BTreeSet::new(),
        }
    }

    pub fn arg(&mut self, arg: impl Into<String>) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    /// sets a variable. the output variables are Shikumi's to fill, so naming one is an error at
    /// synthesis.
    pub fn env(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn input_source(&mut self, path: StorePath) -> &mut Self {
        self.input_srcs.insert(path);
        self
    }

    /// depends on some of another synthesized derivation's outputs. naming the same input twice
    /// adds to its outputs.
    pub fn input_derivation<'a>(
        &mut self,
        input: &StoreDerivation,
        outputs: impl IntoIterator<Item = &'a str>,
    ) -> Result<&mut Self, DerivationError> {
        let mut wanted = BTreeSet::new();
        for output in outputs {
            let (name, _) = input.outputs.get_key_value(output).ok_or_else(|| {
                DerivationError::UnknownInputOutput {
                    input: input.path.clone(),
                    output: output.to_owned(),
                }
            })?;
            wanted.insert(name.clone());
        }
        if wanted.is_empty() {
            return Err(DerivationError::NoInputOutputs(input.path.clone()));
        }
        self.input_drvs
            .entry(input.path.clone())
            .or_insert_with(|| InputDerivation {
                outputs: BTreeSet::new(),
                modulo: input.modulo,
            })
            .outputs
            .extend(wanted);
        Ok(self)
    }

    fn output_names(&self) -> impl Iterator<Item = &str> {
        let (names, fixed) = match &self.outputs {
            Outputs::InputAddressed(names) => (Some(names), None),
            Outputs::Fixed(_) => (None, Some("out")),
        };
        names
            .into_iter()
            .flatten()
            .map(OutputName::as_str)
            .chain(fixed)
    }

    fn check(&self) -> Result<(), DerivationError> {
        StoreName::new(format!("{}.drv", self.name)).map_err(DerivationError::Name)?;
        if self.output_names().next().is_none() {
            return Err(DerivationError::NoOutputs);
        }
        for output in self.output_names() {
            StoreName::new(output_path_name(self.name.as_str(), output))
                .map_err(DerivationError::Name)?;
        }
        // the system is written into the `.drv` unescaped by every implementation, so a quote or a
        // backslash would produce a file no parser reads back as written.
        if self.system.contains(['"', '\\']) {
            return Err(DerivationError::System(self.system.clone()));
        }
        for key in self.env.keys() {
            if self.output_names().any(|output| output == key) {
                return Err(DerivationError::OutputVariable(key.clone()));
            }
            // CppNix 2.35.3 reserves `__json` for structured attributes and refuses it in a plain
            // environment (`StructuredAttrs::checkKeyNotInUse`); Lix accepts it, so it's refused
            // to keep one meaning everywhere.
            if key == "__json" {
                return Err(DerivationError::StructuredAttrs);
            }
        }
        // a NUL hashes the same everywhere, but Lix 2.95.4 and 2.96.0 refuse to build a derivation
        // whose environment holds one, while CppNix 2.35.3 cuts the value short when it starts the
        // builder. the conformance run showed both, so a derivation that holds one builds on only
        // some implementations.
        let strings = std::iter::once(("builder", self.builder.as_str()))
            .chain(std::iter::once(("system", self.system.as_str())))
            .chain(self.args.iter().map(|arg| ("argument", arg.as_str())))
            .chain(
                self.env
                    .iter()
                    .flat_map(|(k, v)| [("environment", k.as_str()), ("environment", v.as_str())]),
            );
        for (field, text) in strings {
            if text.contains('\0') {
                return Err(DerivationError::Nul(field));
            }
        }
        Ok(())
    }
}

/// a derivation with its paths: what's written to the store, and what dependents refer to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDerivation {
    derivation: Derivation,
    path: StorePath,
    outputs: BTreeMap<OutputName, StorePath>,
    aterm: String,
    /// the sha256 of `aterm`, which both the `.drv`'s path and its content address are made from.
    aterm_hash: Sha256,
    modulo: Sha256,
}

impl StoreDerivation {
    /// the `.drv` file's path.
    pub fn path(&self) -> &StorePath {
        &self.path
    }

    /// the `.drv` file's exact contents.
    pub fn aterm(&self) -> &str {
        &self.aterm
    }

    pub fn outputs(&self) -> &BTreeMap<OutputName, StorePath> {
        &self.outputs
    }

    pub fn output(&self, name: &str) -> Option<&StorePath> {
        self.outputs.get(name)
    }

    pub fn name(&self) -> &StoreName {
        &self.derivation.name
    }

    pub fn fixed(&self) -> Option<&FixedHash> {
        match &self.derivation.outputs {
            Outputs::Fixed(hash) => Some(hash),
            Outputs::InputAddressed(_) => None,
        }
    }

    pub fn system(&self) -> &str {
        &self.derivation.system
    }

    pub fn builder(&self) -> &str {
        &self.derivation.builder
    }

    pub fn args(&self) -> &[String] {
        &self.derivation.args
    }

    /// the environment as written, output variables included.
    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.derivation.env
    }

    pub fn input_sources(&self) -> &BTreeSet<StorePath> {
        &self.derivation.input_srcs
    }

    pub fn input_derivations(&self) -> impl Iterator<Item = (&StorePath, &BTreeSet<OutputName>)> {
        self.derivation
            .input_drvs
            .iter()
            .map(|(path, input)| (path, &input.outputs))
    }

    /// what the daemon is told the `.drv` file's content is, which it recomputes the path from.
    pub fn content_address(&self) -> ContentAddress {
        ContentAddress::Text(self.aterm_hash)
    }

    /// the `.drv` file's references, which its text-addressed path is hashed with: its input
    /// sources and input derivations together.
    pub fn references(&self) -> BTreeSet<StorePath> {
        self.derivation
            .input_srcs
            .iter()
            .chain(self.derivation.input_drvs.keys())
            .cloned()
            .collect()
    }
}

impl StoreDir {
    pub fn synthesize(&self, mut drv: Derivation) -> Result<StoreDerivation, DerivationError> {
        drv.check()?;
        let name = drv.name.as_str().to_owned();
        // the three writes below share one buffer; the last one's text is the `.drv` itself.
        let mut buf = String::with_capacity(1024);
        let (outputs, modulo) = match &drv.outputs {
            Outputs::Fixed(hash) => {
                let path = self.fixed_output_path(&name, hash);
                let modulo = self.fixed_modulo(hash, &path);
                drv.env.insert("out".to_owned(), self.display(&path));
                let out = OutputName("out".to_owned());
                (BTreeMap::from([(out, path)]), modulo)
            }
            Outputs::InputAddressed(names) => {
                // the masked form an output path is hashed from has every output variable empty.
                // this is the only place output variables are blanked.
                for output in names {
                    drv.env.insert(output.0.clone(), String::new());
                }
                let inputs = modulo_inputs(&drv.input_drvs);
                write_aterm(&mut buf, self, &drv, None, Inputs::Modulo(&inputs));
                let masked = Sha256::digest(buf.as_bytes());
                let outputs: BTreeMap<OutputName, StorePath> = names
                    .iter()
                    .map(|output| (output.clone(), self.output_path(&output.0, &masked, &name)))
                    .collect();
                for (output, path) in &outputs {
                    let value = drv
                        .env
                        .get_mut(output.as_str())
                        .expect("every output variable was inserted above");
                    self.write_path(value, path);
                }
                // dependents see this derivation through its unmasked hash, with its real output
                // paths, while its own inputs still stand in as their hashes.
                buf.clear();
                write_aterm(
                    &mut buf,
                    self,
                    &drv,
                    Some(&outputs),
                    Inputs::Modulo(&inputs),
                );
                (outputs, Sha256::digest(buf.as_bytes()))
            }
        };
        buf.clear();
        write_aterm(&mut buf, self, &drv, Some(&outputs), Inputs::Paths);
        let aterm_hash = Sha256::digest(buf.as_bytes());
        let path = self.drv_path(&name, &aterm_hash, &drv.input_srcs, drv.input_drvs.keys());
        Ok(StoreDerivation {
            derivation: drv,
            path,
            outputs,
            aterm: buf,
            aterm_hash,
            modulo,
        })
    }
}

/// the input derivations keyed by their hash modulo, as `hashDerivationModulo` rewrites them.
/// two inputs with the same hash, such as two fixed outputs with one content and name, merge into
/// one entry with the union of their outputs. a digest orders as its lowercase hex does, so the
/// entries come out in the order the `.drv` text needs.
fn modulo_inputs(
    input_drvs: &BTreeMap<StorePath, InputDerivation>,
) -> BTreeMap<Sha256, BTreeSet<&str>> {
    let mut inputs: BTreeMap<Sha256, BTreeSet<&str>> = BTreeMap::new();
    for input in input_drvs.values() {
        inputs
            .entry(input.modulo)
            .or_default()
            .extend(input.outputs.iter().map(OutputName::as_str));
    }
    inputs
}

enum Inputs<'a> {
    Paths,
    Modulo(&'a BTreeMap<Sha256, BTreeSet<&'a str>>),
}

/// appends the `.drv` text to `s`. with `outputs` absent, output paths are written empty; together
/// with the blank output variables `synthesize` sets, that's the masked form.
fn write_aterm(
    s: &mut String,
    dir: &StoreDir,
    drv: &Derivation,
    outputs: Option<&BTreeMap<OutputName, StorePath>>,
    inputs: Inputs<'_>,
) {
    let mut hex = [0u8; 64];
    s.push_str("Derive([");
    let fixed = match &drv.outputs {
        Outputs::Fixed(hash) => Some(hash),
        Outputs::InputAddressed(_) => None,
    };
    for (i, name) in drv.output_names().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('(');
        unquoted(s, name);
        s.push_str(",\"");
        if let Some(path) = outputs.and_then(|outputs| outputs.get(name)) {
            dir.write_path(s, path);
        }
        s.push_str("\",");
        match fixed {
            Some(hash) => {
                unquoted(s, hash.method_algo());
                s.push(',');
                unquoted(s, hash.sha256.write_hex(&mut hex));
            }
            None => s.push_str("\"\",\"\""),
        }
        s.push(')');
    }
    s.push_str("],[");
    let mut first = true;
    let mut input = |s: &mut String,
                     key: &mut dyn FnMut(&mut String),
                     names: &mut dyn Iterator<Item = &str>| {
        if !first {
            s.push(',');
        }
        first = false;
        s.push_str("(\"");
        key(s);
        s.push_str("\",[");
        for (i, output) in names.enumerate() {
            if i > 0 {
                s.push(',');
            }
            unquoted(s, output);
        }
        s.push_str("])");
    };
    match inputs {
        Inputs::Paths => {
            for (path, input_drv) in &drv.input_drvs {
                input(
                    s,
                    &mut |s| dir.write_path(s, path),
                    &mut input_drv.outputs.iter().map(OutputName::as_str),
                );
            }
        }
        Inputs::Modulo(map) => {
            for (hash, names) in map {
                input(
                    s,
                    &mut |s| s.push_str(hash.write_hex(&mut [0u8; 64])),
                    &mut names.iter().copied(),
                );
            }
        }
    }
    s.push_str("],[");
    for (i, path) in drv.input_srcs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('"');
        dir.write_path(s, path);
        s.push('"');
    }
    s.push_str("],");
    unquoted(s, &drv.system);
    s.push(',');
    quoted(s, &drv.builder);
    s.push_str(",[");
    for (i, arg) in drv.args.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        quoted(s, arg);
    }
    s.push_str("],[");
    for (i, (key, value)) in drv.env.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('(');
        quoted(s, key);
        s.push(',');
        quoted(s, value);
        s.push(')');
    }
    s.push_str("])");
}

fn unquoted(s: &mut String, text: &str) {
    s.push('"');
    s.push_str(text);
    s.push('"');
}

/// escapes exactly what `printString` escapes: quote, backslash, newline, carriage return and tab.
/// every other byte, NUL and non-ASCII included, is written as it is.
fn quoted(s: &mut String, text: &str) {
    s.push('"');
    for c in text.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c => s.push(c),
        }
    }
    s.push('"');
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DerivationError {
    Name(PathError),
    OutputName { name: String, source: PathError },
    DrvOutput,
    NoOutputs,
    System(String),
    OutputVariable(String),
    StructuredAttrs,
    Nul(&'static str),
    UnknownInputOutput { input: StorePath, output: String },
    NoInputOutputs(StorePath),
}

impl fmt::Display for DerivationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(source) => write!(f, "derivation name: {source}"),
            Self::OutputName { name, source } => write!(f, "output name '{name}': {source}"),
            Self::DrvOutput => f.write_str("an output can't be named 'drv'"),
            Self::NoOutputs => f.write_str("a derivation needs at least one output"),
            Self::System(system) => write!(
                f,
                "system {system:?} holds a quote or a backslash, which a .drv file can't carry"
            ),
            Self::OutputVariable(key) => write!(
                f,
                "environment variable '{key}' names an output; output variables are filled in from the computed paths"
            ),
            Self::Nul(field) => write!(
                f,
                "a derivation's {field} holds a NUL byte, which Lix refuses to build"
            ),
            Self::StructuredAttrs => f.write_str(
                "environment variable '__json' is reserved for structured attributes, which aren't supported",
            ),
            Self::UnknownInputOutput { input, output } => {
                write!(f, "input derivation '{input}' has no output '{output}'")
            }
            Self::NoInputOutputs(input) => {
                write!(f, "input derivation '{input}' is named with no outputs")
            }
        }
    }
}

impl std::error::Error for DerivationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Name(source) | Self::OutputName { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::Ingestion;

    fn outputs(names: &[&str]) -> Outputs {
        Outputs::InputAddressed(names.iter().map(|n| OutputName::new(*n).unwrap()).collect())
    }

    fn hello() -> Derivation {
        let mut drv = Derivation::new(
            StoreName::new("hello").unwrap(),
            outputs(&["out"]),
            "x86_64-linux",
            "/bin/sh",
        );
        drv.arg("-c").arg("echo hello > $out");
        drv
    }

    // `nix derivation add --dry-run` of the same derivation on Lix 2.95.4. a change to either path
    // means the ATerm, the hash modulo or the text-path rule moved, and every `.drv` and output
    // Shikumi writes would land somewhere a daemon doesn't expect.
    #[test]
    fn a_derivation_without_inputs_matches_nix() {
        let drv = StoreDir::default().synthesize(hello()).unwrap();
        assert_eq!(
            drv.path().base_name(),
            "iqaqr00z8983xg1l8iabj0nlaxl42q66-hello.drv"
        );
        assert_eq!(
            drv.output("out").unwrap().base_name(),
            "y12sycb4c09v7r70dkjjq51ic7mymv9i-hello"
        );
        assert_eq!(
            drv.aterm(),
            r#"Derive([("out","/nix/store/y12sycb4c09v7r70dkjjq51ic7mymv9i-hello","","")],[],[],"x86_64-linux","/bin/sh",["-c","echo hello > $out"],[("out","/nix/store/y12sycb4c09v7r70dkjjq51ic7mymv9i-hello")])"#
        );
    }

    // `fetched` is pinned from `nix derivation add --dry-run` on Lix 2.95.4. `top`, with two outputs
    // and inputs, and `above`, whose input has inputs of its own, can't be dry-run without their
    // inputs in the store; their values are this code's, and the conformance cases of the same
    // shapes check the mechanism against every implementation. a change means input rewriting or
    // fixed-output hashing moved.
    #[test]
    fn derivations_with_inputs_keep_their_paths() {
        let dir = StoreDir::default();
        let hello = dir.synthesize(hello()).unwrap();
        let mut fetch = Derivation::new(
            StoreName::new("fetched").unwrap(),
            Outputs::Fixed(FixedHash {
                ingestion: Ingestion::Flat,
                sha256: Sha256::digest(b"hello\n"),
            }),
            "x86_64-linux",
            "/bin/sh",
        );
        fetch.arg("-c").arg("echo hello > $out");
        let fetch = dir.synthesize(fetch).unwrap();
        let mut top = Derivation::new(
            StoreName::new("top").unwrap(),
            outputs(&["out", "dev"]),
            "x86_64-linux",
            "/bin/sh",
        );
        top.input_derivation(&hello, ["out"])
            .unwrap()
            .input_derivation(&fetch, ["out"])
            .unwrap()
            .env("quoted", "a \"b\" \\ c\nd\te\r");
        let top = dir.synthesize(top).unwrap();
        assert_eq!(
            fetch.path().base_name(),
            "vccifl13rsnsyiqs4zgfg2mglmgxwf2g-fetched.drv"
        );
        assert_eq!(
            fetch.output("out").unwrap().base_name(),
            "pi0hrnj8zhpb7hbxf7iyp184v2hc9zra-fetched"
        );
        assert_eq!(
            top.path().base_name(),
            "z4b28q590pgdmm2kxfh4wz2zjinvp6qw-top.drv"
        );
        assert_eq!(
            top.output("out").unwrap().base_name(),
            "1kqqh5yp8b2nflgm0wh1cagmnwygcc8p-top"
        );
        assert_eq!(
            top.output("dev").unwrap().base_name(),
            "2xh3fm2rbrp1lsd7f5f31jpj7py42qik-top-dev"
        );
        let mut above = Derivation::new(
            StoreName::new("above").unwrap(),
            outputs(&["out"]),
            "x86_64-linux",
            "/bin/sh",
        );
        above.input_derivation(&top, ["dev"]).unwrap();
        let above = dir.synthesize(above).unwrap();
        assert_eq!(
            above.output("out").unwrap().base_name(),
            "xppgx1f8azdxhwbcj3rfr740cv0c2rjw-above"
        );
    }

    #[test]
    fn output_variables_are_filled_from_the_computed_paths() {
        let dir = StoreDir::default();
        let drv = dir.synthesize(hello()).unwrap();
        assert_eq!(
            drv.env().get("out"),
            Some(&dir.display(drv.output("out").unwrap()))
        );
    }

    #[test]
    fn setting_an_output_variable_is_refused() {
        let mut drv = hello();
        drv.env("out", "/somewhere");
        assert_eq!(
            StoreDir::default().synthesize(drv),
            Err(DerivationError::OutputVariable("out".to_owned()))
        );
    }

    #[test]
    fn nul_bytes_are_refused_wherever_a_builder_would_see_them() {
        let mut drv = hello();
        drv.env("nul", "before\0after");
        assert_eq!(
            StoreDir::default().synthesize(drv),
            Err(DerivationError::Nul("environment"))
        );
        let mut drv = hello();
        drv.arg("a\0b");
        assert_eq!(
            StoreDir::default().synthesize(drv),
            Err(DerivationError::Nul("argument"))
        );
    }

    #[test]
    fn structured_attrs_and_unparseable_systems_are_refused() {
        let mut drv = hello();
        drv.env("__json", "{}");
        assert_eq!(
            StoreDir::default().synthesize(drv),
            Err(DerivationError::StructuredAttrs)
        );
        let drv = Derivation::new(
            StoreName::new("x").unwrap(),
            outputs(&["out"]),
            "x\"y",
            "/bin/sh",
        );
        assert!(matches!(
            StoreDir::default().synthesize(drv),
            Err(DerivationError::System(_))
        ));
    }

    #[test]
    fn an_output_named_drv_is_refused() {
        assert_eq!(OutputName::new("drv"), Err(DerivationError::DrvOutput));
    }

    #[test]
    fn an_output_name_that_pushes_the_path_name_past_the_limit_is_refused() {
        let drv = Derivation::new(
            StoreName::new("a".repeat(200)).unwrap(),
            outputs(&["out", "documentation"]),
            "x86_64-linux",
            "/bin/sh",
        );
        assert!(matches!(
            StoreDir::default().synthesize(drv),
            Err(DerivationError::Name(PathError::NameTooLong(_)))
        ));
    }

    #[test]
    fn an_input_output_the_input_lacks_is_refused() {
        let dir = StoreDir::default();
        let hello = dir.synthesize(hello()).unwrap();
        let mut drv = Derivation::new(
            StoreName::new("x").unwrap(),
            outputs(&["out"]),
            "x86_64-linux",
            "/bin/sh",
        );
        assert!(matches!(
            drv.input_derivation(&hello, ["dev"]),
            Err(DerivationError::UnknownInputOutput { .. })
        ));
    }

    // two fixed outputs with one name and content differ only in how they're built, so they share
    // a hash modulo and a dependent sees one input. a builder change to a fetcher must not
    // rebuild what depends on it.
    #[test]
    fn fixed_outputs_with_one_content_are_one_input_to_dependents() {
        let dir = StoreDir::default();
        let fetch = |builder: &str| {
            let drv = Derivation::new(
                StoreName::new("fetched").unwrap(),
                Outputs::Fixed(FixedHash {
                    ingestion: Ingestion::Recursive,
                    sha256: Sha256::digest(b"nar"),
                }),
                "x86_64-linux",
                builder,
            );
            dir.synthesize(drv).unwrap()
        };
        let (a, b) = (fetch("/bin/sh"), fetch("/bin/bash"));
        assert_ne!(a.path(), b.path());
        assert_eq!(a.output("out"), b.output("out"));
        let dependent = |inputs: &[&StoreDerivation]| {
            let mut drv = Derivation::new(
                StoreName::new("user").unwrap(),
                outputs(&["out"]),
                "x86_64-linux",
                "/bin/sh",
            );
            for input in inputs {
                drv.input_derivation(input, ["out"]).unwrap();
            }
            dir.synthesize(drv).unwrap()
        };
        let via_a = dependent(&[&a]);
        assert_eq!(via_a.output("out"), dependent(&[&b]).output("out"));
        assert_eq!(via_a.output("out"), dependent(&[&a, &b]).output("out"));
        assert_ne!(via_a.path(), dependent(&[&b]).path());
    }
}
