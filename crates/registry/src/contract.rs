//! what every JSON contract this crate reads or writes is built from: validated strings, the
//! contract's name and version as types that refuse any other, and the one way a schema or a
//! document is written to disk.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
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
            pub fn new(value: impl Into<String>) -> Result<Self, $crate::contract::ContractError> {
                let value = value.into();
                if $valid(value.as_str()) {
                    Ok(Self(value))
                } else {
                    Err($crate::contract::ContractError(format!("{value:?} {}", $refusal)))
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// a value the checker has already held to the same rule, so it isn't tested twice.
            #[allow(dead_code, reason = "not every checked string is built by the checker")]
            pub(crate) fn checked(value: String) -> Self {
                debug_assert!($valid(value.as_str()), "{value:?} {}", $refusal);
                Self(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::contract::ContractError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!($schema)
            }
        }
    };
}

/// a contract's name, such as `shikumi.registry`, as a unit type that writes exactly that string
/// and refuses any other: a document under another name is another contract.
macro_rules! contract_name {
    ($name:ident, $value:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) struct $name;

        impl $name {
            pub(crate) const NAME: &'static str = $value;
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(Self::NAME)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let name = String::deserialize(deserializer)?;
                if name == Self::NAME {
                    Ok(Self)
                } else {
                    Err(serde::de::Error::custom(format!(
                        "schema {name:?} is not {:?}",
                        Self::NAME
                    )))
                }
            }
        }

        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({ "const": Self::NAME })
            }
        }
    };
}

/// a contract's version. a reader takes exactly the version it was built for: the minor number
/// records an additive change for the schema's history, and a reader still refuses it rather than
/// skip fields it can't name.
macro_rules! contract_version {
    ($(#[$doc:meta])* $name:ident, $major:expr, $minor:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name;

        impl $name {
            pub const MAJOR: u32 = $major;
            pub const MINOR: u32 = $minor;
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                $crate::contract::VersionNumbers {
                    major: Self::MAJOR,
                    minor: Self::MINOR,
                }
                .serialize(serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let $crate::contract::VersionNumbers { major, minor } =
                    $crate::contract::VersionNumbers::deserialize(deserializer)?;
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

        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({
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
    };
}

/// a closed set of names, such as the finding codes: each variant's written name and what it
/// means are given once, beside the variant, and the enum, its serde names, `ALL`, `as_str` and
/// `meaning` all come from that one list, so a variant can't be left out of any of them.
macro_rules! closed_set {
    (
        $(#[$doc:meta])*
        $name:ident {
            $($(#[$variant_doc:meta])* $variant:ident = $text:literal: $meaning:literal,)*
        }
    ) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            serde::Serialize, serde::Deserialize, schemars::JsonSchema,
        )]
        pub enum $name {
            $($(#[$variant_doc])* #[serde(rename = $text)] $variant,)*
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)*
                }
            }

            /// what the name stands for, for the reference page.
            pub fn meaning(self) -> &'static str {
                match self {
                    $(Self::$variant => $meaning,)*
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

pub(crate) use {closed_set, contract_name, contract_version};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VersionNumbers {
    pub(crate) major: u32,
    pub(crate) minor: u32,
}

const IDENTIFIER_PATTERN: &str = "^[a-z][a-z0-9-]*$";

fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_text(value: &str) -> bool {
    !value.trim().is_empty()
}

fn is_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn is_glob(value: &str) -> bool {
    is_text(value) && !value.starts_with('/') && !value.split('/').any(|part| part == "..")
}

const VARIABLE_PATTERN: &str = "^[A-Za-z_][A-Za-z0-9_]*$";

/// what an environment variable's name may be, as the variable's refusal and schema say it.
pub(crate) const VARIABLE_ACCEPTS: &str =
    "an environment variable's name: a letter or underscore, then letters, digits and underscores";

pub(crate) fn is_variable(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

// one attribute name, as a regex fragment, which both the name's pattern and the path's are built
// from, so the two can't spell it differently.
macro_rules! attribute_name {
    () => {
        "[A-Za-z_][A-Za-z0-9_'-]*"
    };
}

const ATTRIBUTE_PATTERN: &str = concat!("^", attribute_name!(), "$");

/// what a nixpkgs attribute's name may be, as an override's refusal and schema say it.
pub(crate) const ATTRIBUTE_ACCEPTS: &str = "an attribute name Nix takes unquoted: a letter or underscore, then letters, digits, underscores, apostrophes and hyphens";

pub(crate) fn is_attribute(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'\'' | b'-'))
}

const ATTRIBUTE_PATH_PATTERN: &str =
    concat!("^", attribute_name!(), "(\\.", attribute_name!(), ")*$");

/// what a nixpkgs attribute path may be, as a catalog entry's refusal and schema say it.
pub(crate) const ATTRIBUTE_PATH_ACCEPTS: &str = "a nixpkgs attribute path: attribute names, each one Nix takes unquoted, joined by `.`, such as `jdk21` or `nodePackages.prettier`";

pub(crate) fn is_attribute_path(value: &str) -> bool {
    value.split('.').all(is_attribute)
}

/// the string kinds a declaration is checked as. each one's rule is written once, here: the
/// predicate, what a value of another kind is told it must be, and the phrase that says what it
/// accepts, which the checker's refusals, the contract's own errors and schemas, and the reference
/// page all quote.
macro_rules! scalars {
    ($($variant:ident {
        tag: $tag:literal,
        expected: $expected:literal,
        accepts: $accepts:literal,
        valid: $valid:expr,
    })*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Scalar {
            $($variant,)*
        }

        impl Scalar {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];

            /// the constructor's name in `shikumi.lib.types`, which is also its schema's tag.
            pub fn tag(self) -> &'static str {
                match self {
                    $(Self::$variant => $tag,)*
                }
            }

            /// the kind of value it is, as a type finding names it.
            pub fn expected(self) -> &'static str {
                match self {
                    $(Self::$variant => $expected,)*
                }
            }

            /// what it accepts, as a noun phrase.
            pub fn accepts(self) -> &'static str {
                match self {
                    $(Self::$variant => $accepts,)*
                }
            }

            pub fn valid(self, value: &str) -> bool {
                match self {
                    $(Self::$variant => $valid(value),)*
                }
            }
        }
    };
}

scalars! {
    Text {
        tag: "text",
        expected: "text",
        accepts: "non-blank text",
        valid: is_text,
    }
    Identifier {
        tag: "identifier",
        expected: "an identifier",
        accepts: "an identifier: a lowercase letter, then lowercase letters, digits and hyphens",
        valid: is_identifier,
    }
    Path {
        tag: "path",
        expected: "a path string",
        accepts: "a normalized path relative to the repository root: no leading or trailing slash, no empty, `.` or `..` part",
        valid: is_relative_path,
    }
    Glob {
        tag: "glob",
        expected: "a pattern string",
        accepts: "a non-blank pattern relative to the repository root, with no leading slash and no `..` part",
        valid: is_glob,
    }
}

checked_string!(
    /// a lowercase name: a letter, then letters, digits and hyphens. projects, steps, commands,
    /// services and extension namespaces are all named this way.
    Identifier,
    is_identifier,
    format!("is not {}", Scalar::Identifier.accepts()),
    {
        "type": "string",
        "pattern": IDENTIFIER_PATTERN,
        "description": Scalar::Identifier.accepts()
    }
);

checked_string!(
    /// text that says something; an empty or blank string is a sentinel, not a value.
    Text,
    is_text,
    format!("is not {}", Scalar::Text.accepts()),
    {
        "type": "string",
        "pattern": "\\S",
        "description": Scalar::Text.accepts()
    }
);

checked_string!(
    /// a path relative to the repository root, written the one way the declaration layer writes
    /// it: no leading or trailing slash, and no empty, `.` or `..` part. an absolute path would
    /// differ between worktrees of one commit, which no contract here ever does.
    RelativePath,
    is_relative_path,
    format!("is not {}", Scalar::Path.accepts()),
    {
        "type": "string",
        "pattern": "^(?!.*(^|/)\\.{1,2}(/|$))[^/]+(/[^/]+)*$",
        "description": Scalar::Path.accepts()
    }
);

checked_string!(
    /// the name of an environment variable a project or toolchain defines.
    Variable,
    is_variable,
    format!("is not {VARIABLE_ACCEPTS}"),
    {
        "type": "string",
        "pattern": VARIABLE_PATTERN,
        "description": VARIABLE_ACCEPTS
    }
);

checked_string!(
    /// the name of a nixpkgs attribute the catalog's overlay replaces or adds.
    AttributeName,
    is_attribute,
    format!("is not {ATTRIBUTE_ACCEPTS}"),
    {
        "type": "string",
        "pattern": ATTRIBUTE_PATTERN,
        "description": ATTRIBUTE_ACCEPTS
    }
);

checked_string!(
    /// a package's attribute path in nixpkgs, which the catalog names a package by.
    AttributePath,
    is_attribute_path,
    format!("is not {ATTRIBUTE_PATH_ACCEPTS}"),
    {
        "type": "string",
        "pattern": ATTRIBUTE_PATH_PATTERN,
        "description": ATTRIBUTE_PATH_ACCEPTS
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

impl<T: Clone> std::ops::Deref for NonEmpty<T> {
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

/// a struct whose `Option` fields serde reads through `Option::deserialize` needs each of them
/// present, as an explicit `null` where it doesn't apply. schemars leaves an `Option` out of
/// `required`, and its own `required` attribute drops `null` from the field's schema too, so the
/// struct says so itself: every property is required, and each keeps its nullable schema.
pub(crate) fn require_every_property(schema: &mut Schema) {
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

/// a schema or document in its one written form: pretty JSON with keys in the types' order, then a
/// newline, so tracked files and what a command prints agree byte for byte.
pub fn render_json(value: &impl Serialize) -> String {
    let mut json = serde_json::to_string_pretty(value).expect("a contract value always serializes");
    json.push('\n');
    json
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError(pub(crate) String);

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ContractError {}

#[cfg(test)]
mod tests {
    use super::*;

    // the pattern a checked string's schema publishes, as other readers apply it.
    fn pattern<T: JsonSchema>() -> fancy_regex::Regex {
        let schema = T::json_schema(&mut SchemaGenerator::default());
        let pattern = schema
            .get("pattern")
            .and_then(|pattern| pattern.as_str())
            .expect("a checked string's schema has a pattern");
        fancy_regex::Regex::new(pattern).expect("the schema's pattern is a valid regex")
    }

    // each checked string's rule is written twice, as the predicate the contract checks with and
    // as the pattern its schema publishes; these samples, at each rule's edges, hold the two to one
    // answer.
    #[test]
    fn every_schema_pattern_accepts_what_its_predicate_accepts() {
        let samples = [
            "JAVA_HOME",
            "_x",
            "PATH",
            "a'",
            "x.y.z",
            "x..y",
            ".x",
            "x.",
            "nodePackages.prettier",
            "a.1b",
            "a'.b-c",
            "",
            " ",
            "a",
            "A",
            "a-b",
            "a_b",
            "a1",
            "1a",
            "-a",
            "web",
            "Web",
            "apps/web",
            "/apps",
            "apps/",
            "apps//web",
            "./a",
            "a/./b",
            "a/../b",
            "..",
            ".",
            "a.b",
            "a b",
            "\tx",
        ];
        type Rule = (&'static str, fancy_regex::Regex, fn(&str) -> bool);
        let rules: [Rule; 6] = [
            ("identifier", pattern::<Identifier>(), is_identifier),
            ("text", pattern::<Text>(), is_text),
            ("relative path", pattern::<RelativePath>(), is_relative_path),
            ("variable", pattern::<Variable>(), is_variable),
            ("attribute name", pattern::<AttributeName>(), is_attribute),
            (
                "attribute path",
                pattern::<AttributePath>(),
                is_attribute_path,
            ),
        ];
        for (name, regex, predicate) in rules {
            for sample in samples {
                assert_eq!(
                    regex.is_match(sample).unwrap(),
                    predicate(sample),
                    "{name} disagrees on {sample:?}"
                );
            }
        }
    }
}
