//! the toolchain context, `shikumi.toolchain-context`: each catalog entry's package as the store
//! path of its output, for every system the manifest names, which is what a backend or a shell
//! reads of the catalog. it's produced and cached by the context cache, never part of the registry
//! document, which holds no store path.

use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};

use crate::contract::{contract_name, contract_version};
use crate::evaluation::SystemContext;

contract_name!(ToolchainContextName, "shikumi.toolchain-context");

contract_version!(
    /// the toolchain context's version.
    ToolchainContextVersion,
    1,
    0
);

/// the toolchain context of every system the manifest names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "shikumi.toolchain-context")]
pub struct ToolchainContext {
    schema: ToolchainContextName,
    version: ToolchainContextVersion,
    /// each system, and its context, or null for a system the pinned nixpkgs doesn't know.
    pub systems: BTreeMap<String, Option<SystemContext>>,
}

impl ToolchainContext {
    pub fn new(systems: BTreeMap<String, Option<SystemContext>>) -> Self {
        Self {
            schema: ToolchainContextName,
            version: ToolchainContextVersion,
            systems,
        }
    }
}

/// the published JSON schema of the toolchain context.
pub fn schema() -> Schema {
    schemars::schema_for!(ToolchainContext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::render_json;

    #[test]
    fn tracked_schema_matches_the_types() {
        let tracked = include_str!("../schema/toolchain-context.schema.json");
        assert!(
            tracked == render_json(&schema()),
            "crates/registry/schema/toolchain-context.schema.json is stale; regenerate it with `shikumi registry schema toolchain-context`"
        );
    }

    #[test]
    fn a_path_that_is_not_a_store_path_is_refused() {
        let read = |path: &str| {
            serde_json::from_value::<ToolchainContext>(serde_json::json!({
                "schema": "shikumi.toolchain-context",
                "version": {"major": 1, "minor": 0},
                "systems": {"x86_64-linux": {"packages": {"jdk": path}}}
            }))
        };
        assert!(read("/nix/store/0123456789abcdfghijklmnpqrsvwxyz-openjdk-21").is_ok());
        for path in [
            "relative",
            "/nix/store/short-name",
            "/nix/store/0123456789abcdfghijklmnpqrsvwxyz",
        ] {
            assert!(read(path).is_err(), "{path}");
        }
    }
}
