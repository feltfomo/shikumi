//! value types shared by the bench's JSON contracts. each one rejects the sentinels a loose `String`
//! would let through, so a field that doesn't apply has to be `null` instead of `""`.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

/// a lowercase name that is safe as a path component, a Nix attribute and a derivation name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Identifier(String);

const IDENTIFIER_PATTERN: &str = "^[a-z][a-z0-9-]*$";

impl Identifier {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        let mut bytes = value.bytes();
        let valid = bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(ContractError(format!(
                "{value:?} is not an identifier matching {IDENTIFIER_PATTERN}"
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Identifier {
    type Error = ContractError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Identifier> for String {
    fn from(value: Identifier) -> Self {
        value.0
    }
}

impl fmt::Display for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl JsonSchema for Identifier {
    fn schema_name() -> Cow<'static, str> {
        "Identifier".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": IDENTIFIER_PATTERN,
            "description": "a lowercase name: a letter, then letters, digits and hyphens."
        })
    }
}

/// free text that must say something; an empty string is a sentinel, not a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Text(String);

impl Text {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        if value.trim().is_empty() {
            Err(ContractError("text must not be empty or blank".to_owned()))
        } else {
            Ok(Self(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Text {
    type Error = ContractError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Text> for String {
    fn from(value: Text) -> Self {
        value.0
    }
}

impl JsonSchema for Text {
    fn schema_name() -> Cow<'static, str> {
        "Text".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "\\S",
            "description": "non-blank text."
        })
    }
}

/// a SHA-256 digest as 64 lowercase hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256([u8; 32]);

impl Sha256 {
    pub fn of(bytes: &[u8]) -> Self {
        use sha2::Digest;
        Self(sha2::Sha256::digest(bytes).into())
    }

    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl TryFrom<String> for Sha256 {
    type Error = ContractError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let invalid = || ContractError(format!("{value:?} is not 64 lowercase hex digits"));
        let hex = value.as_bytes();
        if hex.len() != 64 {
            return Err(invalid());
        }
        let digit = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        };
        let mut out = [0u8; 32];
        for (byte, &[high, low]) in out.iter_mut().zip(hex.as_chunks::<2>().0) {
            *byte = (digit(high).ok_or_else(invalid)? << 4) | digit(low).ok_or_else(invalid)?;
        }
        Ok(Self(out))
    }
}

impl From<Sha256> for String {
    fn from(value: Sha256) -> Self {
        value.to_string()
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl JsonSchema for Sha256 {
    fn schema_name() -> Cow<'static, str> {
        "Sha256".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^[0-9a-f]{64}$",
            "description": "a SHA-256 digest in lowercase hex."
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError(pub(crate) String);

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ContractError {}

/// a contract's format version, written as the number `N`. only `N` deserializes, so a reader that
/// meets a newer document fails instead of misreading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Version<const N: u32>;

impl<const N: u32> Serialize for Version<N> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(N)
    }
}

impl<'de, const N: u32> Deserialize<'de> for Version<N> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let version = u32::deserialize(deserializer)?;
        if version == N {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "schema_version {version} is not the supported version {N}"
            )))
        }
    }
}

impl<const N: u32> JsonSchema for Version<N> {
    fn schema_name() -> Cow<'static, str> {
        format!("Version{N}").into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "const": N })
    }
}

/// marks every property of an object schema required. schemars leaves `Option` fields out of
/// `required`, but the contracts write a field that doesn't apply as an explicit `null`, and serde
/// rejects its absence through `deserialize_with = "Option::deserialize"`; the schema has to agree.
pub(crate) fn require_all_properties(schema: &mut Schema) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    let Some(properties) = object.get("properties").and_then(|p| p.as_object()) else {
        return;
    };
    let required = properties
        .keys()
        .cloned()
        .map(serde_json::Value::String)
        .collect();
    object.insert("required".to_owned(), serde_json::Value::Array(required));
}

/// [`require_all_properties`] for each variant of an internally tagged enum, whose schema is a
/// `oneOf` of object schemas.
pub(crate) fn require_all_variant_properties(schema: &mut Schema) {
    let Some(variants) = schema
        .as_object_mut()
        .and_then(|object| object.get_mut("oneOf"))
        .and_then(|variants| variants.as_array_mut())
    else {
        return;
    };
    for variant in variants {
        if let Ok(variant) = <&mut Schema>::try_from(variant) {
            require_all_properties(variant);
        }
    }
}
