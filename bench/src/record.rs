//! the result record: one measured build, written as one JSON line. every executor's measurements
//! use this type, so comparing a candidate with the Cargo baseline is a join on these fields.

use std::num::NonZeroU32;

use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};

use crate::contract::{ContractError, Identifier, Sha256, Text, Version};
use crate::fixture::overhead::Shape;
use crate::fixture::rust_workspace::{Edit, RustWorkspaceParams};

pub type RecordVersion = Version<1>;

/// one measured build, with `unit_count` agreeing with the fixture it names.
// the unit count isn't a field here: it's derived from the fixture, so a record built in code can't
// claim a different number of units than the fixture it names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "RawRecord", into = "RawRecord")]
pub struct Record {
    pub schema_version: RecordVersion,
    pub started_at_unix_ms: u64,
    pub machine: Machine,
    pub nix: Option<NixImplementation>,
    pub candidate: Candidate,
    pub transport: Option<Identifier>,
    pub fixture: Fixture,
    pub scenario: Scenario,
    pub incremental: Option<bool>,
    pub iteration: u32,
    pub wall_time_ns: u64,
    pub cpu_user_ns: u64,
    pub cpu_system_ns: u64,
}

/// the record as written. it owns the published schema and its field descriptions; [`Record`]
/// adds the rules that span fields.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    title = "Shikumi benchmark record",
    transform = crate::contract::require_all_properties
)]
struct RawRecord {
    schema_version: RecordVersion,
    /// when the measured build started, in milliseconds since the Unix epoch.
    started_at_unix_ms: u64,
    machine: Machine,
    /// the Nix implementation the candidate ran against; null when no Nix is in play, as for the
    /// Cargo baseline.
    #[serde(deserialize_with = "Option::deserialize")]
    nix: Option<NixImplementation>,
    candidate: Candidate,
    /// how work and outputs move between the candidate and the store, such as a daemon socket;
    /// null when the candidate has no store transport.
    #[serde(deserialize_with = "Option::deserialize")]
    transport: Option<Identifier>,
    fixture: Fixture,
    scenario: Scenario,
    /// whether rustc's incremental compilation was on; null for fixtures that run no compiler.
    #[serde(deserialize_with = "Option::deserialize")]
    incremental: Option<bool>,
    /// the units the fixture holds: crates for a Rust workspace, units for an overhead graph. it
    /// must match the fixture.
    unit_count: u32,
    /// which repetition of the scenario this is, from 0.
    iteration: u32,
    /// elapsed wall-clock time of the measured build, in nanoseconds.
    wall_time_ns: u64,
    /// user CPU time of the measured build and every process it waited for, in nanoseconds.
    cpu_user_ns: u64,
    /// system CPU time of the measured build and every process it waited for, in nanoseconds.
    cpu_system_ns: u64,
}

impl TryFrom<RawRecord> for Record {
    type Error = ContractError;

    fn try_from(raw: RawRecord) -> Result<Self, Self::Error> {
        raw.fixture.validate()?;
        let units = raw.fixture.unit_count();
        if raw.unit_count != units {
            return Err(ContractError(format!(
                "unit_count {} doesn't match the fixture's {units} units",
                raw.unit_count
            )));
        }
        let RawRecord {
            schema_version,
            started_at_unix_ms,
            machine,
            nix,
            candidate,
            transport,
            fixture,
            scenario,
            incremental,
            unit_count: _,
            iteration,
            wall_time_ns,
            cpu_user_ns,
            cpu_system_ns,
        } = raw;
        Ok(Self {
            schema_version,
            started_at_unix_ms,
            machine,
            nix,
            candidate,
            transport,
            fixture,
            scenario,
            incremental,
            iteration,
            wall_time_ns,
            cpu_user_ns,
            cpu_system_ns,
        })
    }
}

impl From<Record> for RawRecord {
    fn from(record: Record) -> Self {
        let unit_count = record.unit_count();
        let Record {
            schema_version,
            started_at_unix_ms,
            machine,
            nix,
            candidate,
            transport,
            fixture,
            scenario,
            incremental,
            iteration,
            wall_time_ns,
            cpu_user_ns,
            cpu_system_ns,
        } = record;
        Self {
            schema_version,
            started_at_unix_ms,
            machine,
            nix,
            candidate,
            transport,
            fixture,
            scenario,
            incremental,
            unit_count,
            iteration,
            wall_time_ns,
            cpu_user_ns,
            cpu_system_ns,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = crate::contract::require_all_properties)]
pub struct Machine {
    /// the host's name, or the name given on the command line.
    pub name: Text,
    /// the CPU model string; null where the platform doesn't report one.
    #[serde(deserialize_with = "Option::deserialize")]
    pub cpu_model: Option<Text>,
    pub logical_cpus: u32,
    /// Rust's name for the operating system, such as `linux` or `macos`.
    pub os: Text,
    /// Rust's name for the CPU architecture, such as `x86_64` or `aarch64`.
    pub arch: Text,
}

/// the Nix a candidate ran against, with what distinguishes two runs of the same version: where it
/// ran and the settings that change what a build costs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NixImplementation {
    pub implementation: NixFlavor,
    /// the version the implementation reports, such as `2.95.4`.
    pub version: Text,
    pub environment: NixEnvironment,
    pub settings: NixSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum NixFlavor {
    Cppnix,
    Determinate,
    Lix,
}

/// where the Nix implementation ran, relative to the machine the record names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum NixEnvironment {
    /// directly on the machine's own operating system.
    Host,
    /// inside a virtual machine on that machine.
    VirtualMachine,
    /// inside a container on that machine.
    Container,
}

/// the settings that change what a build costs, as the implementation reported them for the run.
/// each is read rather than assumed: a run whose settings weren't read can't be told apart from
/// another, so the contract has no "unknown" value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NixSettings {
    pub sandbox: Setting<SandboxMode>,
    /// whether builds run in their own cgroups.
    pub use_cgroups: Setting<bool>,
    pub max_jobs: Setting<u32>,
    /// cores offered to each build; 0 is Nix's own value for all of them.
    pub cores: Setting<u32>,
}

/// a setting's value for a run, or the fact that the implementation has no such setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", content = "value", rename_all = "kebab-case")]
#[schemars(rename = "Setting_{T}")]
pub enum Setting<T> {
    Set(T),
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    Enabled,
    Disabled,
    Relaxed,
}

/// the executor being measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub name: Identifier,
    /// the version line the candidate's tool reports.
    pub version: Text,
}

/// what was built, with enough to regenerate it and a digest to prove the regeneration matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Fixture {
    RustWorkspace {
        seed: u64,
        parameters: RustWorkspaceParams,
        /// the digest of the generated tree before any scenario edit.
        digest: Sha256,
    },
    Overhead {
        shape: Shape,
        units: NonZeroU32,
        /// the digest of the unit graph's canonical JSON.
        digest: Sha256,
    },
}

impl Fixture {
    pub fn unit_count(&self) -> u32 {
        match self {
            Self::RustWorkspace { parameters, .. } => parameters.unit_count(),
            Self::Overhead { units, .. } => units.get(),
        }
    }

    /// the rules that span fields of the parameters, which their types alone can't hold.
    pub fn validate(&self) -> Result<(), ContractError> {
        match self {
            Self::RustWorkspace { parameters, .. } => parameters.validate(),
            Self::Overhead { .. } => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Scenario {
    /// a build from an empty target directory.
    Cold,
    /// a build straight after a successful one, with nothing changed.
    NoOp,
    /// a build after one private function's body changed.
    OneFunctionEdit,
    /// a build after a public signature changed in a crate many others depend on.
    PublicSignatureEdit,
}

impl Scenario {
    /// the source change the scenario applies before it builds, if any.
    pub fn edit(self) -> Option<Edit> {
        match self {
            Self::Cold | Self::NoOp => None,
            Self::OneFunctionEdit => Some(Edit::OneFunction),
            Self::PublicSignatureEdit => Some(Edit::PublicSignature),
        }
    }
}

impl Record {
    pub fn unit_count(&self) -> u32 {
        self.fixture.unit_count()
    }

    pub fn to_json_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("a record always serializes");
        line.push('\n');
        line
    }

    pub fn from_json_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

pub fn record_schema() -> Schema {
    schemars::schema_for!(Record)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn valid() -> Value {
        json!({
            "schema_version": 1,
            "started_at_unix_ms": 1_790_000_000_000u64,
            "machine": {
                "name": "bench-host",
                "cpu_model": null,
                "logical_cpus": 16,
                "os": "linux",
                "arch": "x86_64"
            },
            "nix": null,
            "candidate": { "name": "cargo", "version": "cargo 1.99.0" },
            "transport": null,
            "fixture": {
                "kind": "rust-workspace",
                "seed": 7,
                "parameters": {
                    "crates": 4,
                    "depth": 2,
                    "dependencies_per_crate": 2,
                    "modules_per_crate": 2,
                    "functions_per_module": 3,
                    "generic_percent": 25,
                    "large_crate": null
                },
                "digest": "0".repeat(64)
            },
            "scenario": "one-function-edit",
            "incremental": true,
            "unit_count": 4,
            "iteration": 0,
            "wall_time_ns": 1_500_000_000u64,
            "cpu_user_ns": 2_000_000_000u64,
            "cpu_system_ns": 300_000_000u64
        })
    }

    fn with_nix() -> Value {
        let mut value = valid();
        value["nix"] = json!({
            "implementation": "determinate",
            "version": "3.11.2",
            "environment": "virtual-machine",
            "settings": {
                "sandbox": { "state": "set", "value": "enabled" },
                "use_cgroups": { "state": "set", "value": false },
                "max_jobs": { "state": "set", "value": 16 },
                "cores": { "state": "set", "value": 0 }
            }
        });
        value["transport"] = json!("daemon-socket");
        value["fixture"] = json!({
            "kind": "overhead",
            "shape": "chain",
            "units": 12,
            "digest": "0".repeat(64)
        });
        value["unit_count"] = json!(12);
        value["incremental"] = json!(null);
        value
    }

    fn parse(value: &Value) -> Result<Record, serde_json::Error> {
        Record::from_json_line(&value.to_string())
    }

    fn rejects_from(base: Value, mutate: impl FnOnce(&mut Value), expected: &str) {
        let mut value = base;
        mutate(&mut value);
        let error = parse(&value)
            .expect_err("malformed record was accepted")
            .to_string();
        assert!(
            error.contains(expected),
            "error {error:?} doesn't mention {expected:?}"
        );
    }

    fn rejects(mutate: impl FnOnce(&mut Value), expected: &str) {
        rejects_from(valid(), mutate, expected);
    }

    #[test]
    fn accepts_and_round_trips_valid_records() {
        for value in [valid(), with_nix()] {
            let record = parse(&value).unwrap();
            let line = record.to_json_line();
            assert_eq!(Record::from_json_line(&line).unwrap(), record);
            assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), value);
        }
    }

    #[test]
    fn keeps_runs_that_differ_only_in_where_or_how_nix_ran_apart() {
        let a = parse(&with_nix()).unwrap();
        let mut moved = with_nix();
        moved["nix"]["environment"] = json!("host");
        let mut cgroups = with_nix();
        cgroups["nix"]["settings"]["use_cgroups"] = json!({ "state": "set", "value": true });
        for other in [moved, cgroups] {
            assert_ne!(parse(&other).unwrap().nix, a.nix);
        }
        let mut unsupported = with_nix();
        unsupported["nix"]["settings"]["use_cgroups"] = json!({ "state": "unsupported" });
        assert!(parse(&unsupported).is_ok());
    }

    #[test]
    fn rejects_a_missing_required_field() {
        rejects(
            |v| drop(v.as_object_mut().unwrap().remove("wall_time_ns")),
            "wall_time_ns",
        );
        rejects_from(
            with_nix(),
            |v| {
                drop(
                    v["nix"]["settings"]
                        .as_object_mut()
                        .unwrap()
                        .remove("cores"),
                )
            },
            "cores",
        );
    }

    #[test]
    fn rejects_a_missing_nullable_field() {
        // null is the explicit "doesn't apply"; leaving the field out is a malformed record.
        rejects(
            |v| drop(v.as_object_mut().unwrap().remove("transport")),
            "transport",
        );
        rejects(|v| drop(v.as_object_mut().unwrap().remove("nix")), "nix");
    }

    #[test]
    fn rejects_a_wrong_type() {
        rejects(|v| v["wall_time_ns"] = json!("1.5s"), "invalid type");
        rejects(|v| v["incremental"] = json!("yes"), "invalid type");
    }

    #[test]
    fn rejects_sentinels() {
        rejects(|v| v["transport"] = json!(""), "identifier");
        rejects(|v| v["wall_time_ns"] = json!(-1), "invalid value");
        rejects(|v| v["candidate"]["version"] = json!(" "), "empty");
        rejects_from(
            with_nix(),
            |v| v["nix"]["settings"]["sandbox"] = json!({ "state": "unknown" }),
            "unknown variant",
        );
    }

    #[test]
    fn rejects_unknown_fields_and_versions() {
        rejects(|v| v["wall_time_s"] = json!(1.5), "unknown field");
        rejects(|v| v["schema_version"] = json!(2), "schema_version 2");
        rejects(|v| v["scenario"] = json!("warm"), "unknown variant");
    }

    #[test]
    fn rejects_a_unit_count_that_contradicts_the_fixture() {
        rejects(
            |v| v["unit_count"] = json!(12),
            "doesn't match the fixture's 4 units",
        );
        rejects_from(
            with_nix(),
            |v| v["unit_count"] = json!(4),
            "fixture's 12 units",
        );
    }

    #[test]
    fn rejects_a_fixture_that_cannot_be_generated() {
        fn params(v: &mut Value) -> &mut Value {
            &mut v["fixture"]["parameters"]
        }
        rejects(|v| params(v)["crates"] = json!(0), "invalid value");
        rejects(|v| params(v)["depth"] = json!(0), "invalid value");
        rejects(|v| params(v)["depth"] = json!(4), "can't fill");
        rejects(
            |v| {
                params(v)["large_crate"] =
                    json!({ "modules": 2, "functions_per_module": 2, "dependencies": 9 })
            },
            "large crate's dependencies",
        );
        rejects_from(
            with_nix(),
            |v| v["fixture"]["units"] = json!(0),
            "invalid value",
        );
    }

    #[test]
    fn schema_requires_every_field_including_nullable_ones() {
        let schema = serde_json::to_value(record_schema()).unwrap();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let properties = schema["properties"].as_object().unwrap();
        assert_eq!(required.len(), properties.len());
        for name in ["nix", "transport", "incremental"] {
            assert!(required.contains(&name), "{name} is not required");
            let types = &properties[name];
            assert!(
                types.to_string().contains("null"),
                "{name} doesn't admit null: {types}"
            );
        }
        let crates = &schema["$defs"]["RustWorkspaceParams"]["properties"]["crates"];
        assert_eq!(crates["minimum"], json!(1), "{crates}");
    }
}
