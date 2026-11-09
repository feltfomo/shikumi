//! the `shikumi.registry` document: its types, its published schema, the diagnostics it carries,
//! and the one refusal every consumer goes through. [`Registry::try_from`] a [`Document`], or
//! [`Registry::from_json`], is the only way to get a [`Registry`], so a document carrying a
//! diagnostic, an unknown field or another version never reaches a consumer, whichever entry point
//! read it.

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU32;

use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};

pub use crate::contract::{AttributeName, AttributePath, Identifier, RelativePath, Text, Variable};
use crate::contract::{
    NonEmpty, closed_set, contract_name, contract_version, require_every_property,
};
use crate::graph;

contract_name!(SchemaName, "shikumi.registry");

contract_version!(
    /// the document's version.
    Version,
    1,
    0
);

/// a project as the document carries it: the serde shape of the vocabulary's
/// [`vocabulary::document_project`], whose descriptions its schema takes. each extension
/// namespace's value is exactly as the repository declared it, and Shikumi gives it no meaning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(transform = described::project)]
pub struct Project {
    pub directory: RelativePath,
    pub summary: Text,
    pub maintainers: NonEmpty<Text>,
    pub depends_on: Vec<Identifier>,
    pub imports: BTreeMap<Identifier, Identifier>,
    pub packages: Vec<Identifier>,
    pub env: BTreeMap<Variable, EnvValue>,
    pub toolchains: BTreeMap<Identifier, Toolchain>,
    pub steps: BTreeMap<Identifier, Step>,
    pub commands: BTreeMap<Identifier, Command>,
    pub services: BTreeMap<Identifier, Service>,
    pub extensions: BTreeMap<Identifier, serde_json::Value>,
}

/// a toolchain setting, such as the JDK or the Rust toolchain: what it puts on the path and in the
/// environment of every project that uses it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = described::toolchain)]
pub struct Toolchain {
    pub description: Text,
    pub packages: Vec<Identifier>,
    pub env: BTreeMap<Variable, EnvValue>,
}

/// an environment variable's value: text as written, or a catalog package's location, which the
/// toolchain context resolves to a store path the document never carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EnvValue {
    Text(String),
    Package(PackageLocation),
}

/// where in a catalog package a variable points: the package, and a path inside it, or null for
/// the package itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_every_property)]
#[schemars(transform = described::location)]
pub struct PackageLocation {
    pub package: Identifier,
    #[serde(deserialize_with = "Option::deserialize")]
    pub path: Option<RelativePath>,
}

impl EnvValue {
    /// the catalog package the value names, if it names one.
    pub fn package(&self) -> Option<&Identifier> {
        match self {
            Self::Text(_) => None,
            Self::Package(location) => Some(&location.package),
        }
    }
}

/// the repository's catalog: the systems its package set is read for, every package a shell or
/// step may use, and the one overlay's entries.
/// the serde shape of the vocabulary's [`vocabulary::document_catalog`], whose descriptions its
/// schema takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = described::catalog)]
pub struct Catalog {
    pub systems: NonEmpty<Text>,
    pub packages: BTreeMap<Identifier, AttributePath>,
    pub overrides: BTreeMap<AttributeName, Override>,
}

/// an entry of the catalog's overlay: why it exists and what lets it go. what it builds is
/// declared and never copied here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = described::overrides)]
pub struct Override {
    pub reason: Text,
    pub release: Text,
}

/// a step as the document carries it. its command line is declared and never copied here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = described::step)]
pub struct Step {
    pub description: Text,
    pub after: Vec<Identifier>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = described::command)]
pub struct Command {
    pub description: Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = described::service)]
pub struct Service {
    pub description: Text,
    pub restart: Restart,
}

/// the schema's descriptions of each record the document carries, taken from the vocabulary, so a
/// field's text is written once, for the reference page and the schema alike.
mod described {
    use std::collections::BTreeMap;

    use schemars::Schema;

    use crate::vocabulary::{self, Record, Type, names};

    fn describe(schema: &mut Schema, record: &Record) {
        let Some(properties) = schema
            .get_mut("properties")
            .and_then(|properties| properties.as_object_mut())
        else {
            return;
        };
        for (name, property) in properties {
            let description = record.get(name).and_then(|field| field.description);
            if let (Some(description), Some(property)) = (description, property.as_object_mut()) {
                property.insert("description".to_owned(), description.into());
            }
        }
    }

    fn project_record() -> Record {
        vocabulary::document_project(&BTreeMap::new())
    }

    // the record each entry of one of a record's attribute sets holds.
    fn entry(record: &Record, field: &str) -> Record {
        match record.get(field).map(|field| &field.ty) {
            Some(Type::Attrs { element, .. }) => match &**element {
                Type::Record(record) => record.clone(),
                _ => Record::default(),
            },
            _ => Record::default(),
        }
    }

    pub(super) fn project(schema: &mut Schema) {
        describe(schema, &project_record());
    }

    pub(super) fn step(schema: &mut Schema) {
        describe(schema, &entry(&project_record(), names::STEPS));
    }

    pub(super) fn command(schema: &mut Schema) {
        describe(schema, &entry(&project_record(), names::COMMANDS));
    }

    pub(super) fn toolchain(schema: &mut Schema) {
        describe(schema, &entry(&project_record(), names::TOOLCHAINS));
    }

    pub(super) fn location(schema: &mut Schema) {
        describe(schema, &vocabulary::package_location());
    }

    pub(super) fn catalog(schema: &mut Schema) {
        describe(schema, &vocabulary::document_catalog());
    }

    pub(super) fn overrides(schema: &mut Schema) {
        describe(
            schema,
            &entry(&vocabulary::document_catalog(), names::OVERRIDES),
        );
    }

    pub(super) fn service(schema: &mut Schema) {
        describe(schema, &entry(&project_record(), names::SERVICES));
    }
}

closed_set! {
    /// when the supervisor restarts a service.
    Restart {
        Never = "never": "the service stays stopped once it exits",
        OnFailure = "on-failure": "the service restarts when it exits with a failure",
        Always = "always": "the service restarts whenever it exits",
    }
}

/// the scope of every source no project owns: the repository minus these directories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Remainder {
    pub excludes: Vec<RelativePath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Error,
}

closed_set! {
    /// what kind of rule a finding broke, as a stable name programs can match on. the message says it
    /// for people and is free to change; a code isn't.
    Code {
        Missing = "missing": "a required field has no value",
        Type = "type": "a value of the wrong kind, such as a string where a list belongs",
        Invalid = "invalid": "a value of the right kind that breaks its type's rule",
        Enum = "enum": "a value outside a closed enumeration",
        UnknownOption = "unknown-option": "a field the record doesn't have",
        StoreReference = "store-reference": "text that refers to a store path",
        Location = "location": "text that contains the repository's location",
        UnknownReference = "unknown-reference": "a name that refers to no project, step, catalog package or toolchain",
        Cycle = "cycle": "projects that depend on each other, or steps that run after each other, in a loop",
        Duplicate = "duplicate": "a name that has one owner defined twice: a command, service or toolchain more than one project declares, or an environment variable a shell gets from two places",
        Conflict = "conflict": "two different packages on one shell's path, propagated inputs included, under one package name or program name",
        SecondOverlay = "second-overlay": "an overlay outside the catalog's own, which is the repository's one overlay",
        NoDeclaration = "no-declaration": "a listed directory with no declaration file, or a catalog directory with no catalog file",
        NotADeclaration = "not-a-declaration": "a declaration file that doesn't return a `shikumi.project` result",
        Overlap = "overlap": "a listed directory at or inside another project's directory",
        Syntax = "syntax": "a declaration or manifest file that isn't valid Nix",
        UndefinedVariable = "undefined-variable": "a variable a file uses that nothing defines",
        TypeError = "type-error": "evaluation stopped at an operation on a value of the wrong type, such as calling something that isn't a function",
        Thrown = "thrown": "evaluation reached a `throw` or an `abort`",
        Assertion = "assertion": "evaluation reached an `assert` whose condition is false",
        InvalidUtf8 = "invalid-utf8": "text that isn't valid UTF-8, which the registry document can't carry",
        Evaluation = "evaluation": "evaluation failed in a way Shikumi doesn't recognize, or the evaluator stopped without saying why",
        ShikumiBug = "shikumi-bug": "evaluation failed inside Shikumi's own Nix, which is a bug in Shikumi rather than in the declaration",
    }
}

/// one step of a field's path: an attribute name, or a list element's index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Segment {
    Field(String),
    Index(u32),
}

/// a field's path inside a declaration or the manifest, written as Nix would select it:
/// `steps.build.after[0]`, with a name that isn't an identifier quoted.
#[derive(Debug, Clone, Copy)]
pub struct ShowPath<'a>(pub &'a [Segment]);

impl fmt::Display for ShowPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, segment) in self.0.iter().enumerate() {
            match segment {
                Segment::Index(item) => write!(f, "[{item}]")?,
                Segment::Field(name) => {
                    if index > 0 {
                        f.write_str(".")?;
                    }
                    if crate::contract::is_attribute(name) {
                        f.write_str(name)?;
                    } else {
                        write!(f, "{name:?}")?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// a line and column, both counted from 1 as the evaluator counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub line: NonZeroU32,
    pub column: NonZeroU32,
}

/// where a finding points: a file relative to the repository root, and the position of the
/// attribute that defined the field, or of the nearest enclosing one that has a position. null
/// when nothing has one, and the finding is about the file as a whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_every_property)]
pub struct Location {
    pub file: RelativePath,
    #[serde(deserialize_with = "Option::deserialize")]
    pub position: Option<Position>,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file)?;
        if let Some(Position { line, column }) = self.position {
            write!(f, ":{line}:{column}")?;
        }
        Ok(())
    }
}

/// a second place a finding involves, such as the first declaration of a duplicated name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Related {
    pub location: Location,
    pub message: Text,
}

/// one finding, collected rather than thrown, so a document carries every one of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_every_property)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Code,
    /// the project the finding belongs to; null for one about the manifest as a whole or a
    /// manifest entry whose name isn't an identifier.
    #[serde(deserialize_with = "Option::deserialize")]
    pub project: Option<Identifier>,
    /// the field's path inside the declaration or manifest; empty for a finding about the whole
    /// file or composition.
    pub path: Vec<Segment>,
    pub location: Location,
    /// what's wrong with the field, as a phrase the path reads before.
    pub message: Text,
    /// how to fix it, where there's one fix to name.
    #[serde(deserialize_with = "Option::deserialize")]
    pub help: Option<Text>,
    pub related: Vec<Related>,
}

impl Diagnostic {
    /// the finding as one sentence: the field's path, then what's wrong with it.
    pub fn headline(&self) -> String {
        if self.path.is_empty() {
            self.message.to_string()
        } else {
            format!("`{}` {}", ShowPath(&self.path), self.message)
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: error[{}]: {}",
            self.location,
            self.code,
            self.headline()
        )
    }
}

/// every project a repository composes, as Shikumi's checker accepted them, or every finding that
/// refused them. serde reads this shape; the rules that span its fields are checked in the
/// conversion to [`Registry`], so a consumer can only act on a document through them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "shikumi.registry", transform = require_every_property)]
pub struct Document {
    pub(crate) schema: SchemaName,
    pub(crate) version: Version,
    /// the project a focused composition centres on, or null for the global composition.
    #[serde(deserialize_with = "Option::deserialize")]
    pub(crate) focus: Option<Identifier>,
    /// every composed project, or null when the document carries a diagnostic.
    #[serde(deserialize_with = "Option::deserialize")]
    pub(crate) projects: Option<BTreeMap<Identifier, Project>>,
    /// the repository's catalog, or null when it declares none, and when the document carries a
    /// diagnostic.
    #[serde(deserialize_with = "Option::deserialize")]
    pub(crate) catalog: Option<Catalog>,
    /// the global composition's remainder; null in a focused one, and when the document carries a
    /// diagnostic.
    #[serde(deserialize_with = "Option::deserialize")]
    pub(crate) remainder: Option<Remainder>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl Document {
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

// what `to_json` writes: `Document`'s fields in its order, borrowed from an accepted registry, so
// printing a document copies none of it.
#[derive(Serialize)]
struct WrittenRegistry<'a> {
    schema: SchemaName,
    version: Version,
    focus: Option<&'a Identifier>,
    projects: &'a BTreeMap<Identifier, Project>,
    catalog: Option<&'a Catalog>,
    remainder: Option<&'a Remainder>,
    diagnostics: [Diagnostic; 0],
}

/// which composition a document holds: every project with the remainder, or one project and what
/// it depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Composition {
    Global { remainder: Remainder },
    Focused { project: Identifier },
}

/// an accepted registry document: no diagnostics, and every reference resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    composition: Composition,
    projects: BTreeMap<Identifier, Project>,
    catalog: Option<Catalog>,
}

impl Registry {
    /// reads a document and refuses it unless it's accepted. this is the refusal every entry point
    /// shares.
    pub fn from_json(bytes: &[u8]) -> Result<Self, RegistryError> {
        let document: Document = serde_json::from_slice(bytes).map_err(RegistryError::Malformed)?;
        Self::try_from(document)
    }

    /// the document in its one written form. two worktrees of one commit produce these bytes alike.
    pub fn to_json(&self) -> String {
        let (focus, remainder) = match &self.composition {
            Composition::Global { remainder } => (None, Some(remainder)),
            Composition::Focused { project } => (Some(project), None),
        };
        crate::contract::render_json(&WrittenRegistry {
            schema: SchemaName,
            version: Version,
            focus,
            projects: &self.projects,
            catalog: self.catalog.as_ref(),
            remainder,
            diagnostics: [],
        })
    }

    pub fn composition(&self) -> &Composition {
        &self.composition
    }

    pub fn projects(&self) -> &BTreeMap<Identifier, Project> {
        &self.projects
    }

    pub fn catalog(&self) -> Option<&Catalog> {
        self.catalog.as_ref()
    }
}

impl TryFrom<Document> for Registry {
    type Error = RegistryError;

    fn try_from(document: Document) -> Result<Self, Self::Error> {
        // the name and version have done their work by existing at all.
        let Document {
            schema: SchemaName,
            version: Version,
            focus,
            projects,
            catalog,
            remainder,
            diagnostics,
        } = document;
        if !diagnostics.is_empty() {
            return Err(RegistryError::Refused(diagnostics));
        }
        let contract = |message: String| Err(RegistryError::Contract(message));
        let Some(projects) = projects else {
            return contract("projects is null, yet the document carries no diagnostic".to_owned());
        };
        let composition = match (focus, remainder) {
            (None, Some(remainder)) => Composition::Global { remainder },
            (Some(project), None) => Composition::Focused { project },
            (None, None) => return contract("the global composition has no remainder".to_owned()),
            (Some(focus), Some(_)) => {
                return contract(format!(
                    "the composition focused on `{focus}` carries a remainder"
                ));
            }
        };
        if let Composition::Focused { project } = &composition
            && !projects.contains_key(project)
        {
            return contract(format!(
                "the focused project `{project}` isn't in the document"
            ));
        }
        for (name, project) in &projects {
            if let Some(missing) = project
                .depends_on
                .iter()
                .find(|dependency| !projects.contains_key(*dependency))
            {
                return contract(format!(
                    "`{name}` depends on `{missing}`, which isn't in the document"
                ));
            }
            for (step_name, step) in &project.steps {
                if let Some(missing) = step
                    .after
                    .iter()
                    .find(|other| !project.steps.contains_key(*other))
                {
                    return contract(format!(
                        "step `{step_name}` of `{name}` runs after `{missing}`, which isn't one of its steps"
                    ));
                }
            }
        }
        if let Some(message) = first_loop(&projects) {
            return contract(message);
        }
        let focus = match &composition {
            Composition::Global { .. } => None,
            Composition::Focused { project } => Some(project),
        };
        if let Some(message) = unresolved_package_or_toolchain(&projects, catalog.as_ref(), focus) {
            return contract(message);
        }
        Ok(Self {
            composition,
            projects,
            catalog,
        })
    }
}

/// one place a shell takes packages and variables from: a project, or one of the toolchains it
/// defines. every rule about a shell's packages and variables reads these, in the checker and in
/// the refusal alike, so a new place a package can come from is added here once.
#[derive(Debug, Clone, Copy)]
pub struct ShellSource<'p> {
    /// the project that declares it.
    pub owner: &'p Identifier,
    /// the toolchain it is, or none for the project itself.
    pub toolchain: Option<&'p Identifier>,
    pub packages: &'p [Identifier],
    pub env: &'p BTreeMap<Variable, EnvValue>,
}

impl ShellSource<'_> {
    /// every catalog package it names: on the path, then in its variables.
    pub fn picks(&self) -> impl Iterator<Item = &Identifier> {
        self.packages
            .iter()
            .chain(self.env.values().filter_map(EnvValue::package))
    }
}

impl Project {
    /// the project's own sources: itself, then each toolchain it defines, in name order.
    pub fn sources<'p>(&'p self, owner: &'p Identifier) -> impl Iterator<Item = ShellSource<'p>> {
        let own = ShellSource {
            owner,
            toolchain: None,
            packages: &self.packages,
            env: &self.env,
        };
        std::iter::once(own).chain(self.toolchains.iter().map(move |(name, toolchain)| {
            ShellSource {
                owner,
                toolchain: Some(name),
                packages: &toolchain.packages,
                env: &toolchain.env,
            }
        }))
    }
}

/// the sources of one composition's shell. the global shell holds every project and every
/// toolchain; a focused one holds the focused project, the toolchains it defines and those it
/// imports, each from its owner. either way they come in one order: every toolchain, by its
/// project's name and then its own, and then every project, by name. a rule that keeps the first of
/// two definitions keeps the same one in a focused shell as in the global shell, and reports the
/// other at the same place; and since every project that uses a toolchain shares it, a clash
/// between a toolchain and a project is reported at the project, whatever the names.
pub fn shell<'p>(
    projects: impl IntoIterator<Item = (&'p Identifier, &'p Project)>,
    focus: Option<&Identifier>,
) -> Vec<ShellSource<'p>> {
    let projects: Vec<(&Identifier, &Project)> = projects.into_iter().collect();
    let imports = focus.and_then(|focus| {
        projects
            .iter()
            .find(|(name, _)| *name == focus)
            .map(|(_, project)| &project.imports)
    });
    let (toolchains, own): (Vec<ShellSource<'p>>, Vec<ShellSource<'p>>) = projects
        .iter()
        .flat_map(|(name, project)| project.sources(name))
        .filter(|source| {
            let focused = focus.is_none_or(|focus| source.owner == focus);
            let imported = source.toolchain.is_some_and(|toolchain| {
                imports.is_some_and(|imports| imports.get(toolchain) == Some(source.owner))
            });
            focused || imported
        })
        .partition(|source| source.toolchain.is_some());
    toolchains.into_iter().chain(own).collect()
}

/// who claims each name in one of the composition's name spaces, in name order: the commands,
/// services or toolchains `names` reads from a project. a name with more than one claimant is
/// declared twice.
pub fn claims<'p, N>(
    projects: impl IntoIterator<Item = (&'p Identifier, &'p Project)>,
    names: impl Fn(&'p Project) -> N,
) -> BTreeMap<&'p Identifier, Vec<&'p Identifier>>
where
    N: Iterator<Item = &'p Identifier>,
{
    let mut claims: BTreeMap<&Identifier, Vec<&Identifier>> = BTreeMap::new();
    for (owner, project) in projects {
        for name in names(project) {
            claims.entry(name).or_default().push(owner);
        }
    }
    claims
}

// the first rule on packages, toolchains and variables the document can decide that it breaks: a
// package outside the catalog, a toolchain two projects define, an import its project doesn't
// define, or a variable the composition's shell gets twice. a conflict between two packages needs
// the catalog's package graph, which the document doesn't carry, so the checker alone holds it.
fn unresolved_package_or_toolchain(
    projects: &BTreeMap<Identifier, Project>,
    catalog: Option<&Catalog>,
    focus: Option<&Identifier>,
) -> Option<String> {
    let cataloged =
        |package: &Identifier| catalog.is_some_and(|c| c.packages.contains_key(package));
    for (name, project) in projects {
        for source in project.sources(name) {
            if let Some(package) = source.picks().find(|package| !cataloged(package)) {
                return Some(format!(
                    "`{name}` uses `{package}`, which isn't in the catalog"
                ));
            }
        }
    }
    let owners = claims(projects, |project| project.toolchains.keys());
    if let Some((toolchain, owners)) = owners.iter().find(|(_, owners)| owners.len() > 1) {
        return Some(format!(
            "toolchain `{toolchain}` is defined by both `{}` and `{}`",
            owners[0], owners[1]
        ));
    }
    for (name, project) in projects {
        for (toolchain, owner) in &project.imports {
            if owners
                .get(toolchain)
                .is_none_or(|owners| !owners.contains(&owner))
            {
                return Some(format!(
                    "`{name}` imports toolchain `{toolchain}` from `{owner}`, which doesn't define it in the document"
                ));
            }
        }
    }
    let mut defined: BTreeMap<&Variable, &Identifier> = BTreeMap::new();
    for source in shell(projects, focus) {
        for variable in source.env.keys() {
            if let Some(first) = defined.insert(variable, source.owner) {
                return Some(format!(
                    "the shell gets `{variable}` from both `{first}` and `{}`",
                    source.owner
                ));
            }
        }
    }
    None
}

// the first loop in either ordering, which nothing could order. every reference resolves by now.
fn first_loop(projects: &BTreeMap<Identifier, Project>) -> Option<String> {
    let named = |found: &[(usize, graph::Edge)], names: &[&str]| -> Vec<String> {
        found
            .iter()
            .map(|(node, _)| names[*node].to_owned())
            .collect()
    };
    let sentence = |names: Vec<String>, relation: &str| {
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        graph::sentence(&names, relation)
    };
    let names: Vec<&str> = projects.keys().map(Identifier::as_str).collect();
    let position = graph::positions(names.iter().copied());
    let dependencies: Vec<Vec<graph::Edge>> = projects
        .values()
        .map(|project| graph::edges(project.depends_on.iter().map(Identifier::as_str), &position))
        .collect();
    if let Some(found) = graph::cycles(&dependencies).first() {
        let described = sentence(named(found, &names), "depends on");
        return Some(format!("the dependencies loop: {described}"));
    }
    for (name, project) in projects {
        let steps: Vec<&str> = project.steps.keys().map(Identifier::as_str).collect();
        let position = graph::positions(steps.iter().copied());
        let order: Vec<Vec<graph::Edge>> = project
            .steps
            .values()
            .map(|step| graph::edges(step.after.iter().map(Identifier::as_str), &position))
            .collect();
        if let Some(found) = graph::cycles(&order).first() {
            let described = sentence(named(found, &steps), "runs after");
            return Some(format!("the steps of `{name}` loop: {described}"));
        }
    }
    None
}

/// the published JSON schema of the document.
pub fn schema() -> Schema {
    schemars::schema_for!(Document)
}

#[derive(Debug)]
pub enum RegistryError {
    /// the bytes aren't a document of this schema and version.
    Malformed(serde_json::Error),
    /// the document carries diagnostics: the declarations are refused, every finding with them.
    Refused(Vec<Diagnostic>),
    /// the document parses but breaks a rule that spans its fields.
    Contract(String),
}

/// the line that ends every refusal, whoever renders the findings above it.
pub fn refusal_summary(count: usize) -> String {
    let noun = if count == 1 { "finding" } else { "findings" };
    format!("the declarations are refused, with {count} {noun}")
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(error) => write!(f, "the registry document is malformed: {error}"),
            Self::Refused(diagnostics) => {
                write!(f, "{}:", refusal_summary(diagnostics.len()))?;
                diagnostics
                    .iter()
                    .try_for_each(|diagnostic| write!(f, "\n  {diagnostic}"))
            }
            Self::Contract(message) => {
                write!(f, "the registry document breaks its contract: {message}")
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// a text the checker wrote. it always says something, so this can't fail on any message the
/// checker builds.
pub(crate) fn text(value: impl Into<String>) -> Text {
    Text::new(value).expect("a message the checker writes is never blank")
}

/// an identifier when `name` is one.
pub(crate) fn identifier(name: &str) -> Option<Identifier> {
    Identifier::new(name).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::contract::render_json;
    use crate::vocabulary;

    const BASIC: &str = include_str!("../fixtures/basic/document.json");

    fn basic() -> Value {
        serde_json::from_str(BASIC).unwrap()
    }

    fn read(document: &Value) -> Result<Registry, RegistryError> {
        Registry::from_json(document.to_string().as_bytes())
    }

    fn contract_message(document: &Value) -> String {
        match read(document) {
            Err(RegistryError::Contract(message)) => message,
            other => panic!("expected a contract error, got {other:?}"),
        }
    }

    fn finding(path: Value) -> Value {
        json!({
            "severity": "error",
            "code": "missing",
            "project": "web",
            "path": path,
            "location": {
                "file": "apps/web/project.nix",
                "position": {"line": 3, "column": 5}
            },
            "message": "is required but not defined",
            "help": null,
            "related": []
        })
    }

    // the tracked schema is generated from the types; this fails when one drifts, and
    // `shikumi registry schema > crates/registry/schema/registry.schema.json` regenerates it.
    #[test]
    fn tracked_schema_matches_the_types() {
        let tracked = include_str!("../schema/registry.schema.json");
        assert!(
            tracked == render_json(&schema()),
            "crates/registry/schema/registry.schema.json is stale; regenerate it with `shikumi registry schema`"
        );
    }

    // every field of a vocabulary type, against the matching part of the published schema: a
    // record's carried fields are its properties, no more and no fewer, and an attribute set's or
    // a list's elements follow theirs. reports each place the two disagree.
    fn disagreements(ty: &vocabulary::Type, schema: &Value, defs: &Value, at: &str) -> Vec<String> {
        use vocabulary::Type;
        let schema = match schema.get("$ref").and_then(Value::as_str) {
            Some(reference) => &defs[reference.trim_start_matches("#/$defs/")],
            None => schema,
        };
        let mut found = Vec::new();
        match ty {
            Type::Record(record) if !record.fields().is_empty() => {
                let properties = schema["properties"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                for (name, field) in record.fields() {
                    let path = format!("{at}.{name}");
                    match (field.ty.carried(), properties.get(name)) {
                        (true, Some(property)) => {
                            found.extend(disagreements(&field.ty, property, defs, &path));
                        }
                        (true, None) => found.push(format!("{path} isn't in the schema")),
                        (false, Some(_)) => found.push(format!(
                            "{path} is in the schema, though the vocabulary never carries it"
                        )),
                        (false, None) => {}
                    }
                }
                for name in properties.keys() {
                    if record.get(name).is_none() {
                        found.push(format!("{at}.{name} isn't in the vocabulary"));
                    }
                }
            }
            // a map keyed by a checked name is written with `patternProperties`, any other with
            // `additionalProperties`.
            // a variable's value is text or the location record it carries.
            Type::EnvValue(location) => {
                let branch = schema["anyOf"]
                    .as_array()
                    .and_then(|branches| {
                        branches.iter().find(|branch| branch.get("$ref").is_some())
                    })
                    .unwrap_or(&Value::Null);
                let record = Type::Record(location.clone());
                found.extend(disagreements(&record, branch, defs, at));
            }
            Type::Attrs { element, .. } => {
                let entry = schema["patternProperties"]
                    .as_object()
                    .and_then(|patterns| patterns.values().next())
                    .unwrap_or(&schema["additionalProperties"]);
                found.extend(disagreements(element, entry, defs, at));
            }
            Type::List { element, .. } => {
                found.extend(disagreements(element, &schema["items"], defs, at));
            }
            _ => {}
        }
        found
    }

    // the vocabulary is the one definition of what a project holds, and `Project` is its serde
    // shape; this holds every field of one to the other, all the way down.
    #[test]
    fn the_document_holds_exactly_the_fields_the_vocabulary_defines() {
        let schema = serde_json::to_value(schema()).unwrap();
        let project = vocabulary::Type::Record(vocabulary::document_project(&BTreeMap::new()));
        let mut found = disagreements(
            &project,
            &schema["$defs"]["Project"],
            &schema["$defs"],
            "project",
        );
        let catalog = vocabulary::Type::Record(vocabulary::document_catalog());
        found.extend(disagreements(
            &catalog,
            &schema["$defs"]["Catalog"],
            &schema["$defs"],
            "catalog",
        ));
        assert_eq!(found, Vec::<String>::new());
        assert_eq!(
            schema["$defs"]["Step"]["properties"]["after"]["description"],
            "steps of the same project this one runs after when both run"
        );
    }

    #[test]
    fn a_composed_document_is_accepted_and_written_back_byte_for_byte() {
        let registry = Registry::from_json(BASIC.as_bytes()).unwrap();
        assert_eq!(registry.to_json(), BASIC);
    }

    #[test]
    fn a_document_with_an_error_diagnostic_is_refused_with_every_finding() {
        let mut document = basic();
        document["projects"] = Value::Null;
        document["remainder"] = Value::Null;
        document["diagnostics"] = json!([
            finding(json!(["project", "summary"])),
            finding(json!(["steps", "build", "after", 0]))
        ]);
        match read(&document) {
            Err(RegistryError::Refused(diagnostics)) => {
                assert_eq!(diagnostics.len(), 2);
                assert_eq!(
                    diagnostics[1].to_string(),
                    "apps/web/project.nix:3:5: error[missing]: `steps.build.after[0]` is required but not defined"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_diagnostic_is_refused_even_when_projects_are_present() {
        let mut document = basic();
        document["diagnostics"] = json!([finding(json!([]))]);
        assert!(matches!(read(&document), Err(RegistryError::Refused(_))));
    }

    #[test]
    fn an_unknown_field_is_refused_at_every_level() {
        let mut top = basic();
        top["extra"] = json!(1);
        let mut project = basic();
        project["projects"]["web"]["exec"] = json!("serve");
        let mut step = basic();
        step["projects"]["core"]["steps"]["build"]["exec"] = json!("make");
        let mut diagnostic = basic();
        let mut extra = finding(json!([]));
        extra["files"] = json!([]);
        diagnostic["diagnostics"] = json!([extra]);
        for document in [top, project, step, diagnostic] {
            assert!(matches!(read(&document), Err(RegistryError::Malformed(_))));
        }
    }

    #[test]
    fn another_version_or_schema_name_is_refused() {
        for version in [
            json!({"major": 2, "minor": 0}),
            json!({"major": 1, "minor": 1}),
        ] {
            let mut document = basic();
            document["version"] = version;
            let error = read(&document).unwrap_err().to_string();
            assert!(
                error.contains("is not the supported version 1.0"),
                "{error}"
            );
        }
        let mut document = basic();
        document["schema"] = json!("shikumi.lock");
        assert!(matches!(read(&document), Err(RegistryError::Malformed(_))));
    }

    #[test]
    fn a_missing_nullable_field_is_refused_rather_than_read_as_null() {
        let mut document = basic();
        document.as_object_mut().unwrap().remove("focus");
        assert!(matches!(read(&document), Err(RegistryError::Malformed(_))));
        let mut diagnostic = basic();
        let mut partial = finding(json!([]));
        partial.as_object_mut().unwrap().remove("help");
        diagnostic["diagnostics"] = json!([partial]);
        assert!(matches!(
            read(&diagnostic),
            Err(RegistryError::Malformed(_))
        ));
    }

    #[test]
    fn sentinels_are_refused() {
        for (pointer, value) in [
            ("/projects/web/summary", json!(" ")),
            ("/projects/web/directory", json!("/abs/apps/web")),
            ("/projects/web/directory", json!("apps/../web")),
            ("/projects/web/dependsOn/0", json!("Core")),
            (
                "/projects/web/services/database/restart",
                json!("sometimes"),
            ),
        ] {
            let mut document = basic();
            *document.pointer_mut(pointer).unwrap() = value;
            assert!(
                matches!(read(&document), Err(RegistryError::Malformed(_))),
                "{pointer} was accepted"
            );
        }
        let mut line = basic();
        let mut zero = finding(json!([]));
        zero["location"]["position"]["line"] = json!(0);
        line["diagnostics"] = json!([zero]);
        assert!(matches!(read(&line), Err(RegistryError::Malformed(_))));
    }

    #[test]
    fn a_reference_that_resolves_nowhere_breaks_the_contract() {
        let mut dependency = basic();
        dependency["projects"]["web"]["dependsOn"] = json!(["nowhere"]);
        assert!(contract_message(&dependency).contains("`nowhere`"));

        let mut step = basic();
        step["projects"]["core"]["steps"]["test"]["after"] = json!(["package"]);
        assert!(contract_message(&step).contains("`package`"));

        let mut focus = basic();
        focus["focus"] = json!("nowhere");
        focus["remainder"] = Value::Null;
        assert!(contract_message(&focus).contains("`nowhere`"));
    }

    #[test]
    fn a_package_or_toolchain_that_resolves_nowhere_breaks_the_contract() {
        let mut package = basic();
        package["projects"]["web"]["packages"] = json!(["nowhere"]);
        assert!(contract_message(&package).contains("`nowhere`, which isn't in the catalog"));

        let mut variable = basic();
        variable["projects"]["web"]["env"]["HOME_OF"] = json!({"package": "nowhere", "path": null});
        assert!(contract_message(&variable).contains("`nowhere`, which isn't in the catalog"));

        let mut uncataloged = basic();
        uncataloged["catalog"] = Value::Null;
        assert!(contract_message(&uncataloged).contains("which isn't in the catalog"));

        let mut import = basic();
        import["projects"]["web"]["imports"]["rust"] = json!("core");
        assert!(contract_message(&import).contains("toolchain `rust` from `core`"));

        // a variable the shell gets from a toolchain and again from a project that uses it.
        let mut variable = basic();
        variable["projects"]["web"]["env"]["JAVA_HOME"] = json!("/opt/jdk");
        assert!(
            contract_message(&variable).contains("gets `JAVA_HOME` from both `core` and `web`"),
            "{}",
            contract_message(&variable)
        );

        let mut twice = basic();
        twice["projects"]["docs"]["toolchains"]["jdk"] =
            twice["projects"]["core"]["toolchains"]["jdk"].clone();
        assert!(
            contract_message(&twice).contains("defined by both `core` and `docs`"),
            "{}",
            contract_message(&twice)
        );
    }

    #[test]
    fn a_loop_in_either_ordering_breaks_the_contract() {
        let mut dependencies = basic();
        dependencies["projects"]["core"]["dependsOn"] = json!(["web"]);
        assert!(
            contract_message(&dependencies).contains("the dependencies loop"),
            "{}",
            contract_message(&dependencies)
        );

        let mut steps = basic();
        steps["projects"]["core"]["steps"]["build"]["after"] = json!(["test"]);
        assert!(
            contract_message(&steps).contains("the steps of `core` loop"),
            "{}",
            contract_message(&steps)
        );
    }

    #[test]
    fn the_remainder_belongs_to_the_global_composition_alone() {
        let mut global = basic();
        global["remainder"] = Value::Null;
        assert!(contract_message(&global).contains("no remainder"));

        let mut focused = basic();
        focused["focus"] = json!("web");
        assert!(contract_message(&focused).contains("carries a remainder"));
    }

    #[test]
    fn a_project_without_a_maintainer_is_refused_where_it_is_read() {
        let mut document = basic();
        document["projects"]["docs"]["maintainers"] = json!([]);
        let error = read(&document).unwrap_err();
        assert!(matches!(error, RegistryError::Malformed(_)), "{error}");
        assert!(error.to_string().contains("must not be empty"), "{error}");
    }

    #[test]
    fn null_projects_without_a_diagnostic_break_the_contract() {
        let mut document = basic();
        document["projects"] = Value::Null;
        assert!(contract_message(&document).contains("projects is null"));
    }

    #[test]
    fn the_schema_requires_every_nullable_field() {
        let schema = serde_json::to_value(schema()).unwrap();
        let required = |value: &Value| -> Vec<String> {
            serde_json::from_value(value["required"].clone()).unwrap()
        };
        for field in ["focus", "projects", "catalog", "remainder", "diagnostics"] {
            assert!(required(&schema).iter().any(|r| r == field), "{field}");
        }
        let diagnostic = &schema["$defs"]["Diagnostic"];
        for field in ["project", "path", "location", "help", "related"] {
            assert!(required(diagnostic).iter().any(|r| r == field), "{field}");
        }
        assert!(
            required(&schema["$defs"]["Location"])
                .iter()
                .any(|r| r == "position")
        );
    }

    #[test]
    fn a_path_reads_the_way_nix_selects_it() {
        let path = [
            Segment::Field("extensions".into()),
            Segment::Field("Not Valid".into()),
            Segment::Field("pages".into()),
            Segment::Index(2),
        ];
        assert_eq!(
            ShowPath(&path).to_string(),
            r#"extensions."Not Valid".pages[2]"#
        );
    }

    #[test]
    fn every_code_is_listed_once_with_its_name() {
        let names: Vec<String> = Code::ALL
            .iter()
            .map(|code| {
                serde_json::to_value(code)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        let shown: Vec<&str> = Code::ALL.iter().map(|code| code.as_str()).collect();
        assert_eq!(names, shown);
        let mut unique = shown.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), Code::ALL.len());
    }
}
