//! the strings this crate's contracts are checked as, each rule written once: the predicate, the
//! refusal and the schema's pattern. the machinery that turns them into contract types, and the
//! names and versions every contract carries, is `shikumi-contract`'s.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

pub(crate) use shikumi_contract::{
    ContractError, checked_string, closed_set, contract_name, contract_version, render_json,
};

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

impl RelativePath {
    /// a path the checker has already held to the same rule, so it isn't tested twice.
    pub(crate) fn checked(value: String) -> Self {
        debug_assert!(
            is_relative_path(&value),
            "{value:?} is not {}",
            Scalar::Path.accepts()
        );
        Self(value)
    }
}

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
            Err(ContractError::new("the list must not be empty"))
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
/// `required`, and its own `required` attribute drops `null` from the field's schema too
/// (schemars_derive 1.2.3 src/schema_exprs.rs:185-188 and 729-738), so the struct says so itself:
/// every property is required, and each keeps its nullable schema.
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
