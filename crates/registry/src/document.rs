//! the `shikumi.registry` document: its types, its published schema, and the one refusal every
//! consumer goes through. [`Registry::from_json`] is the only way to get a [`Registry`], so a
//! document carrying an error diagnostic, an unknown field or another version never reaches a
//! consumer, whichever entry point read it.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Deref;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

/// a validated string: the predicate decides what it accepts, the message says why it refused,
/// and the schema states the same rule for other readers.
macro_rules! checked_string {
    ($(#[$doc:meta])* $name:ident, $valid:expr, $refusal:expr, $schema:tt) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
                let value = value.into();
                if $valid(value.as_str()) {
                    Ok(Self(value))
                } else {
                    Err(ContractError(format!("{value:?} {}", $refusal)))
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = ContractError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!($schema)
            }
        }
    };
}

const IDENTIFIER_PATTERN: &str = "^[a-z][a-z0-9-]*$";

fn identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn text(value: &str) -> bool {
    !value.trim().is_empty()
}

fn relative_path(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

checked_string!(
    /// a lowercase name: a letter, then letters, digits and hyphens. projects, steps, commands,
    /// services and extension namespaces are all named this way.
    Identifier,
    identifier,
    "is not an identifier matching ^[a-z][a-z0-9-]*$",
    {
        "type": "string",
        "pattern": IDENTIFIER_PATTERN,
        "description": "a lowercase name: a letter, then letters, digits and hyphens."
    }
);

checked_string!(
    /// text that says something; an empty or blank string is a sentinel, not a value.
    Text,
    text,
    "is empty or blank, and text must say something",
    {
        "type": "string",
        "pattern": "\\S",
        "description": "non-blank text."
    }
);

checked_string!(
    /// a path relative to the repository root, written the one way the declaration layer writes
    /// it: no leading or trailing slash, and no empty, `.` or `..` part. an absolute path would
    /// differ between worktrees of one commit, which the document never does.
    RelativePath,
    relative_path,
    "is not a normalized path relative to the repository root",
    {
        "type": "string",
        "pattern": "^(?!.*(^|/)\\.{1,2}(/|$))[^/]+(/[^/]+)*$",
        "description": "a normalized path relative to the repository root."
    }
);

/// a list with at least one element, so an empty one is refused where it's read rather than by a
/// check someone has to remember.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<T>", into = "Vec<T>")]
pub struct NonEmpty<T: Clone>(Vec<T>);

impl<T: Clone> TryFrom<Vec<T>> for NonEmpty<T> {
    type Error = ContractError;

    fn try_from(items: Vec<T>) -> Result<Self, Self::Error> {
        if items.is_empty() {
            Err(ContractError("the list must not be empty".to_owned()))
        } else {
            Ok(Self(items))
        }
    }
}

impl<T: Clone> From<NonEmpty<T>> for Vec<T> {
    fn from(value: NonEmpty<T>) -> Self {
        value.0
    }
}

impl<T: Clone> Deref for NonEmpty<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T: Clone + JsonSchema> JsonSchema for NonEmpty<T> {
    fn schema_name() -> Cow<'static, str> {
        format!("NonEmpty_{}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = <Vec<T>>::json_schema(generator);
        schema.insert("minItems".to_owned(), 1.into());
        schema
    }
}

/// the document's name, `shikumi.registry`. any other name is another contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SchemaName;

const SCHEMA_NAME: &str = "shikumi.registry";

impl Serialize for SchemaName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(SCHEMA_NAME)
    }
}

impl<'de> Deserialize<'de> for SchemaName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        if name == SCHEMA_NAME {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "schema {name:?} is not {SCHEMA_NAME:?}"
            )))
        }
    }
}

impl JsonSchema for SchemaName {
    fn schema_name() -> Cow<'static, str> {
        "SchemaName".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "const": SCHEMA_NAME })
    }
}

/// the document's version. a reader takes exactly the version it was built for: the minor number
/// records an additive change for the schema's history, and a reader still refuses it rather than
/// skip fields it can't name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version;

impl Version {
    pub const MAJOR: u32 = 1;
    pub const MINOR: u32 = 0;
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionNumbers {
    major: u32,
    minor: u32,
}

impl Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        VersionNumbers {
            major: Self::MAJOR,
            minor: Self::MINOR,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let VersionNumbers { major, minor } = VersionNumbers::deserialize(deserializer)?;
        if (major, minor) == (Self::MAJOR, Self::MINOR) {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "version {major}.{minor} is not the supported version {}.{}",
                Self::MAJOR,
                Self::MINOR
            )))
        }
    }
}

impl JsonSchema for Version {
    fn schema_name() -> Cow<'static, str> {
        "Version".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "properties": {
                "major": { "const": Self::MAJOR },
                "minor": { "const": Self::MINOR }
            },
            "required": ["major", "minor"],
            "additionalProperties": false
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Project {
    pub directory: RelativePath,
    pub summary: Text,
    pub maintainers: NonEmpty<Text>,
    pub depends_on: Vec<Identifier>,
    pub steps: BTreeMap<Identifier, Step>,
    pub commands: BTreeMap<Identifier, Command>,
    pub services: BTreeMap<Identifier, Service>,
    /// each namespace's value exactly as the repository declared it, checked against the schema
    /// the repository supplied for it. Shikumi gives none of it a meaning.
    pub extensions: BTreeMap<Identifier, serde_json::Value>,
}

/// a step as the document carries it. its command line is declared and never copied here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub description: Text,
    /// steps of the same project this one runs after when both run.
    pub after: Vec<Identifier>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub description: Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub description: Text,
    pub restart: Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    Never,
    OnFailure,
    Always,
}

impl fmt::Display for Restart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Never => "never",
            Self::OnFailure => "on-failure",
            Self::Always => "always",
        })
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

/// what kind of rule a finding broke. the message says it for people; the code is for programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Code {
    Missing,
    Type,
    Invalid,
    Enum,
    UnknownOption,
    Conflict,
    StoreReference,
    Location,
    UnknownReference,
    Duplicate,
    NoDeclaration,
    Overlap,
}

/// one finding from the declaration layer, naming where it was defined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_every_property)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Code,
    #[serde(deserialize_with = "Option::deserialize")]
    pub project: Option<Identifier>,
    /// the option path inside the declaration or manifest, null for a finding about the whole
    /// composition.
    #[serde(deserialize_with = "Option::deserialize")]
    pub option: Option<Text>,
    /// the files that defined it, relative to the repository root. a file outside the repository
    /// isn't named, and the message says one was involved.
    pub files: Vec<RelativePath>,
    pub message: Text,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut files = self.files.iter();
        match files.next() {
            Some(first) => {
                write!(f, "{first}")?;
                files.try_for_each(|file| write!(f, ", {file}"))?;
            }
            None => f.write_str("(no file)")?,
        }
        if let Some(project) = &self.project {
            write!(f, " [{project}]")?;
        }
        if let Some(option) = &self.option {
            write!(f, ": {option}")?;
        }
        write!(f, ": {}", self.message)
    }
}

// the document as it's written. serde reads this shape; the rules that span fields are checked in
// the conversion to `Registry`, so the public type can only be built through them.
/// every project a repository composes, as Shikumi's declaration layer checked them, or every
/// finding that refused them.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "shikumi.registry", transform = require_every_property)]
struct RawRegistry {
    schema: SchemaName,
    version: Version,
    /// the project a focused composition centres on, or null for the global composition.
    #[serde(deserialize_with = "Option::deserialize")]
    focus: Option<Identifier>,
    /// every composed project, or null when the document carries a diagnostic.
    #[serde(deserialize_with = "Option::deserialize")]
    projects: Option<BTreeMap<Identifier, Project>>,
    /// the global composition's remainder; null in a focused one, and when the document carries a
    /// diagnostic.
    #[serde(deserialize_with = "Option::deserialize")]
    remainder: Option<Remainder>,
    diagnostics: Vec<Diagnostic>,
}

// what `to_json` writes: `RawRegistry`'s fields in its order, borrowed from an accepted registry, so
// printing a document copies none of it.
#[derive(Serialize)]
struct WrittenRegistry<'a> {
    schema: SchemaName,
    version: Version,
    focus: Option<&'a Identifier>,
    projects: &'a BTreeMap<Identifier, Project>,
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
}

impl Registry {
    /// reads a document and refuses it unless it's accepted. this is the refusal every entry point
    /// shares.
    pub fn from_json(bytes: &[u8]) -> Result<Self, RegistryError> {
        let raw: RawRegistry = serde_json::from_slice(bytes).map_err(RegistryError::Malformed)?;
        Self::try_from(raw)
    }

    /// the document in its one written form: pretty JSON with keys in a fixed order, then a
    /// newline. two worktrees of one commit produce these bytes alike.
    pub fn to_json(&self) -> String {
        let (focus, remainder) = match &self.composition {
            Composition::Global { remainder } => (None, Some(remainder)),
            Composition::Focused { project } => (Some(project), None),
        };
        let written = WrittenRegistry {
            schema: SchemaName,
            version: Version,
            focus,
            projects: &self.projects,
            remainder,
            diagnostics: [],
        };
        let mut json =
            serde_json::to_string_pretty(&written).expect("a registry always serializes");
        json.push('\n');
        json
    }

    pub fn composition(&self) -> &Composition {
        &self.composition
    }

    pub fn projects(&self) -> &BTreeMap<Identifier, Project> {
        &self.projects
    }
}

impl TryFrom<RawRegistry> for Registry {
    type Error = RegistryError;

    fn try_from(raw: RawRegistry) -> Result<Self, Self::Error> {
        // the name and version have done their work by deserializing at all.
        let RawRegistry {
            schema: SchemaName,
            version: Version,
            focus,
            projects,
            remainder,
            diagnostics,
        } = raw;
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
        Ok(Self {
            composition,
            projects,
        })
    }
}

/// a struct whose `Option` fields serde reads through `Option::deserialize` needs each of them
/// present, as an explicit `null` where it doesn't apply. schemars leaves an `Option` out of
/// `required`, and its own `required` attribute drops `null` from the field's schema too, so the
/// struct says so itself: every property is required, and each keeps its nullable schema.
fn require_every_property(schema: &mut Schema) {
    let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
        return;
    };
    let required = properties
        .keys()
        .cloned()
        .map(serde_json::Value::String)
        .collect();
    schema.insert("required".to_owned(), serde_json::Value::Array(required));
}

/// the published JSON schema of the document.
pub fn schema() -> Schema {
    schemars::schema_for!(RawRegistry)
}

/// how the schema is written to disk, so the tracked file and the drift test agree byte for byte.
pub fn render_schema(schema: &Schema) -> String {
    let mut json = serde_json::to_string_pretty(schema).expect("a schema always serializes");
    json.push('\n');
    json
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError(String);

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ContractError {}

#[derive(Debug)]
pub enum RegistryError {
    /// the bytes aren't a document of this schema and version.
    Malformed(serde_json::Error),
    /// the document carries diagnostics: the declarations are refused, every finding with them.
    Refused(Vec<Diagnostic>),
    /// the document parses but breaks a rule that spans its fields.
    Contract(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(error) => write!(f, "the registry document is malformed: {error}"),
            Self::Refused(diagnostics) => {
                let count = diagnostics.len();
                let noun = if count == 1 { "finding" } else { "findings" };
                write!(f, "the declarations are refused, with {count} {noun}:")?;
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

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

    // the tracked schema is generated from the types; this fails when one drifts, and
    // `shikumi registry schema > crates/registry/schema/registry.schema.json` regenerates it.
    #[test]
    fn tracked_schema_matches_the_types() {
        let tracked = include_str!("../schema/registry.schema.json");
        assert!(
            tracked == render_schema(&schema()),
            "crates/registry/schema/registry.schema.json is stale; regenerate it with `shikumi registry schema`"
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
        let finding = |option: &str| {
            json!({
                "severity": "error",
                "code": "missing",
                "project": "web",
                "option": option,
                "files": ["apps/web/project.nix"],
                "message": "is required but not defined"
            })
        };
        document["diagnostics"] = json!([finding("project.summary"), finding("steps.build.exec")]);
        match read(&document) {
            Err(RegistryError::Refused(diagnostics)) => {
                assert_eq!(diagnostics.len(), 2);
                assert_eq!(
                    diagnostics[1].to_string(),
                    "apps/web/project.nix [web]: steps.build.exec: is required but not defined"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_diagnostic_is_refused_even_when_projects_are_present() {
        let mut document = basic();
        document["diagnostics"] = json!([{
            "severity": "error",
            "code": "duplicate",
            "project": null,
            "option": "commands.serve",
            "files": ["apps/web/project.nix", "docs/project.nix"],
            "message": "is declared by more than one project"
        }]);
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
        for document in [top, project, step] {
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
        for field in ["focus", "projects", "remainder", "diagnostics"] {
            assert!(required(&schema).iter().any(|r| r == field), "{field}");
        }
        let diagnostic = &schema["$defs"]["Diagnostic"];
        for field in ["project", "option", "files"] {
            assert!(required(diagnostic).iter().any(|r| r == field), "{field}");
        }
    }
}
