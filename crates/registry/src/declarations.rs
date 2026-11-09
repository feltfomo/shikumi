//! the `shikumi.declarations` contract: the manifest and every composed declaration as the
//! declaration layer serialized them, which is what the checker reads. every value arrives as a
//! tagged node saying what kind of Nix value it was, and every attribute with the source position
//! the evaluator gave it, so a value of the wrong kind is something to report rather than
//! something that failed to parse. what the values mean is the checker's; this module only reads
//! the shape.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};

use crate::contract::{RelativePath, contract_name, contract_version, require_every_property};
use crate::document::{Location, Position};

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
    pub manifest: Manifest,
    /// every composed project by its manifest name; null where the manifest's entry names no
    /// declaration file.
    pub projects: BTreeMap<String, Option<Declaration>>,
}

impl Declarations {
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
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
    Null,
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
    Function,
    /// a derivation or anything else with an `outPath`, which stands for a store path.
    Derivation,
    /// infinity or NaN, which JSON can't write.
    NonFinite,
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
            Self::Null => "null",
            Self::Path { .. } => "a path",
            Self::List { .. } => "a list",
            Self::Attrs { .. } => "an attribute set",
            Self::Function => "a function",
            Self::Derivation => "a derivation or another value with an `outPath`",
            Self::NonFinite => "a number that isn't finite",
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
