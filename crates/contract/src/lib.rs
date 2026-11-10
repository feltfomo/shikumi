//! what every JSON contract Shikumi reads or writes is built from: validated strings, closed sets
//! of names, the contract's name and version as types that refuse any other, and the one written
//! form of a schema or document. the registry and the lock both build their contracts from these.

use std::fmt;

use serde::{Deserialize, Serialize};

/// a validated string: the predicate decides what it accepts, the message says why it refused,
/// and the schema states the same rule for other readers.
#[macro_export]
macro_rules! checked_string {
    ($(#[$doc:meta])* $name:ident, $valid:expr, $refusal:expr, $schema:tt) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, $crate::ContractError> {
                let value = value.into();
                if $valid(value.as_str()) {
                    Ok(Self(value))
                } else {
                    Err($crate::ContractError::new(format!("{value:?} {}", $refusal)))
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::ContractError;

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
#[macro_export]
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
#[macro_export]
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
                $crate::VersionNumbers {
                    major: Self::MAJOR,
                    minor: Self::MINOR,
                }
                .serialize(serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let $crate::VersionNumbers { major, minor } =
                    $crate::VersionNumbers::deserialize(deserializer)?;
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
#[macro_export]
macro_rules! closed_set {
    (
        $(#[$doc:meta])*
        $name:ident {
            $($(#[$variant_doc:meta])* $variant:ident = $text:literal: $meaning:expr,)*
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

/// a contract's version as JSON writes it, for `contract_version!`.
#[doc(hidden)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionNumbers {
    pub major: u32,
    pub minor: u32,
}

/// a schema or document in its one written form: pretty JSON with keys in the types' order, then a
/// newline, so tracked files and what a command prints agree byte for byte.
pub fn render_json(value: &impl Serialize) -> String {
    let mut json = serde_json::to_string_pretty(value).expect("a contract value always serializes");
    json.push('\n');
    json
}

/// why a value isn't one a contract accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError(String);

impl ContractError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ContractError {}
