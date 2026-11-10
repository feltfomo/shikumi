//! the `shikumi.declarations` contract: the manifest, the catalog as each system reads it, and every
//! composed declaration as the declaration layer serialized them, or why evaluating one failed,
//! which is what the checker reads. every value arrives as a tagged node saying what kind of Nix
//! value it was, and every attribute with the source position the evaluator gave it, so a value of
//! the wrong kind is something to report rather than something that failed to parse. what the
//! values mean is the checker's; this module only reads the shape.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};

use crate::contract::{
    RelativePath, closed_set, contract_name, contract_version, require_every_property,
};
use crate::document::{Location, Position, Segment};

contract_name!(DeclarationsName, "shikumi.declarations");

contract_version!(
    /// the serialized declarations' version.
    DeclarationsVersion,
    1,
    0
);

/// the manifest and the declarations of every project the composition holds, as the declaration
/// layer serialized them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "shikumi.declarations", transform = require_every_property)]
pub struct Declarations {
    schema: DeclarationsName,
    version: DeclarationsVersion,
    /// the project the composition was asked to focus on, as given, or null for the global one.
    #[serde(deserialize_with = "Option::deserialize")]
    pub focus: Option<String>,
    pub manifest: Evaluated<Manifest>,
    /// the catalog the manifest declares, or null where it declares none.
    #[serde(deserialize_with = "Option::deserialize")]
    pub catalog: Option<Catalog>,
    /// every composed project by its manifest name; null where the manifest's entry names no
    /// declaration file.
    pub projects: BTreeMap<String, Option<Evaluated<Declaration>>>,
}

impl Declarations {
    pub fn new(
        focus: Option<String>,
        manifest: Evaluated<Manifest>,
        catalog: Option<Catalog>,
        projects: BTreeMap<String, Option<Evaluated<Declaration>>>,
    ) -> Self {
        Self {
            schema: DeclarationsName,
            version: DeclarationsVersion,
            focus,
            manifest,
            catalog,
            projects,
        }
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// a value the layer serialized, or the failure that stopped its evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Evaluated<T> {
    Value(T),
    Failed(Failed),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Failed {
    pub failure: Failure,
}

/// why evaluating a declaration or the manifest stopped, as Shikumi read it from the evaluator's
/// error: what kind of failure it was, and which of the trace's frames lie in the repository's
/// files and which in Shikumi's own Nix. frames anywhere else, such as in nixpkgs' `lib`, are
/// neither the user's nor Shikumi's to fix, and are left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_every_property)]
pub struct Failure {
    pub kind: FailureKind,
    /// what the evaluator said, with the repository's location taken out of it. for a thrown
    /// error, it's the text that was thrown; for an undefined variable, the variable's name.
    pub message: String,
    pub origin: Origin,
}

impl Failure {
    /// the field the failure names, for one of the user's.
    pub fn field(&self) -> Option<&FieldAt> {
        match &self.origin {
            Origin::User(origin) => origin.field.as_ref(),
            Origin::Shikumi(_) => None,
        }
    }
}

/// whose the failure is to fix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Origin {
    User(UserOrigin),
    Shikumi(ShikumiOrigin),
}

/// a failure that passed through the repository's files, or one in a value the user wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_every_property)]
pub struct UserOrigin {
    /// the frames in the repository's files, innermost first: where the failure happened in the
    /// user's code, then what led there. empty where the evaluator gives no position there.
    pub frames: Vec<At>,
    /// the field holding text that isn't valid UTF-8, where the evaluator could be made to say.
    #[serde(deserialize_with = "Option::deserialize")]
    pub field: Option<FieldAt>,
}

/// a bug in Shikumi, whose Nix never fails on user input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShikumiOrigin {
    /// the frames in Shikumi's own Nix, innermost first, for the report.
    pub frames: Vec<InternalFrame>,
}

closed_set! {
    /// what stopped an evaluation, as far as Shikumi can tell from the evaluator's error.
    FailureKind {
        Syntax = "syntax": "a file that isn't valid Nix",
        UndefinedVariable = "undefined-variable": "a variable nothing defines",
        Type = "type": "an operation on a value of the wrong type, such as calling something that isn't a function",
        Thrown = "thrown": "a `throw` or `abort` evaluation reached",
        Assertion = "assertion": "an `assert` whose condition was false",
        InvalidUtf8 = "invalid-utf8": "text that isn't valid UTF-8, which the registry document can't carry",
        Unrecognized = "unrecognized": "a failure Shikumi doesn't recognize, with the evaluator's own message",
        Crashed = "crashed": "an evaluator that stopped without reporting an error, as one that was killed does",
    }
}

/// a field's path inside a declaration and the nearest source position on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_every_property)]
pub struct FieldAt {
    pub path: Vec<Segment>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub at: Option<At>,
}

/// a frame in Shikumi's own Nix: a file of the declaration layer, relative to the layer, and the
/// position in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InternalFrame {
    pub file: String,
    pub line: NonZeroU32,
    pub column: NonZeroU32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub file: RelativePath,
    pub value: Node,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    /// whether the file returned a `shikumi.project` result; when it didn't, `value` is what it
    /// returned instead.
    pub factory: bool,
    pub value: Node,
}

/// the catalog, read once for each system the manifest names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_every_property)]
pub struct Catalog {
    /// the catalog's file, or null where the directory the manifest names holds none; then no
    /// system is read.
    #[serde(deserialize_with = "Option::deserialize")]
    pub file: Option<RelativePath>,
    /// each system the manifest names as text, and the catalog read for it, or null for a system
    /// the pinned nixpkgs doesn't know.
    pub systems: BTreeMap<String, Option<Evaluated<CatalogRead>>>,
}

/// the catalog as one system's package set gives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogRead {
    /// the catalog file's value, serialized like a declaration: its packages are derivations, and
    /// its overlay's builds functions.
    pub value: Node,
    /// each entry of `packages` that is a package, by catalog name, as the package's `name`.
    pub packages: BTreeMap<String, String>,
    /// every package reachable from those through propagation, by `name`.
    pub graph: BTreeMap<String, Package>,
}

/// what evaluation can see of a package without building it: what it's called, the program it
/// runs as, and the packages it propagates into whatever depends on it, by `name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(transform = require_every_property)]
pub struct Package {
    pub pname: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub version: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub main_program: Option<String>,
    pub propagates: Vec<String>,
}

/// one Nix value, tagged with what kind of value it was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Node {
    /// a string's text, and whether it carried a store reference the text may not show.
    String {
        value: String,
        context: bool,
    },
    Int {
        value: i64,
    },
    Float {
        value: f64,
    },
    Bool {
        value: bool,
    },
    // the kinds that carry nothing are empty structs, not units: serde holds a unit variant of a
    // tagged enum to none of the enum's `deny_unknown_fields`, and would take
    // `{"kind": "null", "junk": 1}` as a null.
    Null {},
    /// a path value, relative to the repository root, or null when it lies outside it.
    Path {
        #[serde(deserialize_with = "Option::deserialize")]
        value: Option<String>,
    },
    List {
        items: Vec<Node>,
    },
    Attrs {
        fields: BTreeMap<String, Field>,
    },
    Function {},
    /// a derivation or anything else with an `outPath`, which stands for a store path.
    Derivation {},
    /// infinity or NaN, which JSON can't write.
    NonFinite {},
    /// a value with more values in it, or deeper nesting, than serialization walks, as one that
    /// refers to itself has, with the limits the serializer held it to.
    TooLarge {
        values: u64,
        depth: u32,
    },
}

impl Node {
    /// what the value was, as the phrase a finding names it with.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::String { .. } => "a string",
            Self::Int { .. } => "an integer",
            Self::Float { .. } => "a float",
            Self::Bool { .. } => "a boolean",
            Self::Null {} => "null",
            Self::Path { .. } => "a path",
            Self::List { .. } => "a list",
            Self::Attrs { .. } => "an attribute set",
            Self::Function {} => "a function",
            Self::Derivation {} => "a derivation or another value with an `outPath`",
            Self::NonFinite {} => "a number that isn't finite",
            Self::TooLarge { .. } => "a value too large to read",
        }
    }
}

/// an attribute's value and where it was defined, or null where the evaluator gives no position
/// inside the repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_every_property)]
pub struct Field {
    #[serde(deserialize_with = "Option::deserialize")]
    pub at: Option<At>,
    pub value: Node,
}

/// an attribute's source position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct At {
    pub file: RelativePath,
    pub line: NonZeroU32,
    pub column: NonZeroU32,
}

impl At {
    pub fn location(&self) -> Location {
        Location {
            file: self.file.clone(),
            position: Some(Position {
                line: self.line,
                column: self.column,
            }),
        }
    }
}

/// the published JSON schema of the serialized declarations.
pub fn schema() -> Schema {
    schemars::schema_for!(Declarations)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::contract::render_json;

    // the schema says every node's kind has exactly its own fields, which the kinds that carry
    // nothing have to hold to as well as the rest.
    #[test]
    fn a_node_with_a_field_its_kind_doesnt_have_is_refused() {
        for kind in ["null", "function", "derivation", "non-finite"] {
            let plain = format!(r#"{{"kind":"{kind}"}}"#);
            assert!(serde_json::from_str::<Node>(&plain).is_ok(), "{plain}");
            let extra = format!(r#"{{"kind":"{kind}","junk":1}}"#);
            assert!(serde_json::from_str::<Node>(&extra).is_err(), "{extra}");
        }
        let string = r#"{"kind":"string","value":"x","context":false,"junk":1}"#;
        assert!(serde_json::from_str::<Node>(string).is_err());
    }

    #[test]
    fn tracked_schema_matches_the_types() {
        let tracked = include_str!("../schema/declarations.schema.json");
        assert!(
            tracked == render_json(&schema()),
            "crates/registry/schema/declarations.schema.json is stale; regenerate it with `shikumi registry schema declarations`"
        );
    }

    #[test]
    fn the_tracked_fixtures_read_and_write_back_byte_for_byte() {
        for tracked in [
            include_str!("../fixtures/basic/declarations.json"),
            include_str!("../fixtures/broken/declarations.json"),
            include_str!("../fixtures/failures/declarations.json"),
        ] {
            let read = Declarations::from_json(tracked.as_bytes()).unwrap();
            assert_eq!(render_json(&read), tracked);
        }
    }

    #[test]
    fn an_unknown_kind_or_field_is_refused() {
        let base = |value: serde_json::Value| {
            json!({
                "schema": "shikumi.declarations",
                "version": {"major": 1, "minor": 0},
                "focus": null,
                "manifest": {"file": "shikumi.nix", "value": value},
                "catalog": null,
                "projects": {}
            })
        };
        let read = |value| Declarations::from_json(base(value).to_string().as_bytes());
        assert!(read(json!({"kind": "null"})).is_ok());
        assert!(read(json!({"kind": "set"})).is_err());
        assert!(
            read(json!({"kind": "string", "value": "x", "context": false, "extra": 1})).is_err()
        );
        assert!(
            read(json!({"kind": "attrs", "fields": {"a": {"value": {"kind": "null"}}}})).is_err()
        );
    }
}
