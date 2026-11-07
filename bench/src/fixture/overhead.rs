//! the overhead fixture: units whose builders do nothing, so a measurement of them is the floor
//! cost an executor adds per unit. the graph is plain JSON rather than a Nix expression or a Cargo
//! workspace, so no executor gets a format it alone reads natively; each one translates it into its
//! own unit of work.

use std::collections::HashSet;
use std::num::NonZeroU32;

use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};

use crate::contract::{ContractError, Identifier, Sha256, Version};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    /// independent units with no inputs, for throughput.
    Wide,
    /// each unit takes the one before it as its only input, for latency.
    Chain,
}

/// what a unit's builder does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// create an empty output and nothing else. an executor must not add a setup environment, such
    /// as nixpkgs' stdenv, whose own cost would hide the floor being measured.
    EmptyOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    pub name: Identifier,
    /// units whose outputs must exist before this one runs. each names an earlier unit.
    pub inputs: Vec<Identifier>,
    pub action: Action,
}

/// a validated unit graph: unique names, inputs that name earlier units, and a structure that
/// matches its declared shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "RawGraph")]
pub struct UnitGraph {
    schema_version: GraphVersion,
    shape: Shape,
    units: Vec<Unit>,
}

type GraphVersion = Version<1>;

/// the wire form before validation; the published schema describes this shape.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "Shikumi overhead unit graph")]
struct RawGraph {
    schema_version: GraphVersion,
    shape: Shape,
    /// in dependency order: every unit appears after all of its inputs.
    units: Vec<Unit>,
}

impl TryFrom<RawGraph> for UnitGraph {
    type Error = ContractError;

    fn try_from(raw: RawGraph) -> Result<Self, Self::Error> {
        let fail = |message: String| Err(ContractError(message));
        if raw.units.is_empty() {
            return fail("a unit graph needs at least one unit".to_owned());
        }
        // the shape fixes every unit's inputs, which also makes each input an earlier unit, so
        // the shape is the only input rule.
        let mut names = HashSet::with_capacity(raw.units.len());
        for (index, unit) in raw.units.iter().enumerate() {
            if !names.insert(&unit.name) {
                return fail(format!("unit {} appears twice", unit.name));
            }
            let expected: &[&Identifier] = match (raw.shape, index) {
                (Shape::Wide, _) | (Shape::Chain, 0) => &[],
                (Shape::Chain, _) => &[&raw.units[index - 1].name],
            };
            if !unit.inputs.iter().eq(expected.iter().copied()) {
                return fail(format!(
                    "unit {} has inputs that don't fit a {:?} graph",
                    unit.name, raw.shape
                ));
            }
        }
        Ok(Self {
            schema_version: raw.schema_version,
            shape: raw.shape,
            units: raw.units,
        })
    }
}

impl UnitGraph {
    pub fn generate(shape: Shape, units: NonZeroU32) -> Self {
        let names: Vec<Identifier> = (0..units.get())
            .map(|i| Identifier::new(format!("u{i:04}")).expect("generated names are identifiers"))
            .collect();
        let units = names
            .iter()
            .enumerate()
            .map(|(index, name)| Unit {
                name: name.clone(),
                inputs: match shape {
                    Shape::Chain if index > 0 => vec![names[index - 1].clone()],
                    _ => Vec::new(),
                },
                action: Action::EmptyOutput,
            })
            .collect();
        Self {
            schema_version: Version,
            shape,
            units,
        }
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub fn units(&self) -> &[Unit] {
        &self.units
    }

    /// the canonical bytes: what the generator writes and what the digest covers.
    pub fn to_canonical_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self).expect("a unit graph always serializes");
        json.push('\n');
        json
    }

    pub fn digest(&self) -> Sha256 {
        Sha256::of(self.to_canonical_json().as_bytes())
    }
}

pub fn unit_graph_schema() -> Schema {
    schemars::schema_for!(UnitGraph)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn nz(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    fn names(units: &[Unit]) -> Vec<&str> {
        units.iter().map(|u| u.name.as_str()).collect()
    }

    #[test]
    fn wide_graph_has_independent_units() {
        let graph = UnitGraph::generate(Shape::Wide, nz(12));
        assert_eq!(graph.units().len(), 12);
        assert!(graph.units().iter().all(|u| u.inputs.is_empty()));
        assert!(
            graph
                .units()
                .iter()
                .all(|u| u.action == Action::EmptyOutput)
        );
    }

    #[test]
    fn chain_graph_links_each_unit_to_the_one_before() {
        for n in 1..=30 {
            let graph = UnitGraph::generate(Shape::Chain, nz(n));
            let units = graph.units();
            assert_eq!(units.len(), n as usize);
            assert!(units[0].inputs.is_empty());
            for pair in units.windows(2) {
                assert_eq!(pair[1].inputs, [pair[0].name.clone()]);
            }
        }
        let graph = UnitGraph::generate(Shape::Chain, nz(3));
        assert_eq!(names(graph.units()), ["u0000", "u0001", "u0002"]);
    }

    // a record names a graph by shape, unit count and digest, so the canonical bytes are pinned:
    // a change to them is a change to every recorded overhead fixture.
    #[test]
    fn canonical_json_round_trips_and_is_pinned() {
        let pinned = [
            (
                Shape::Wide,
                "b9010236f8303fe05ab12818689c1bc6e9a1b406cef4e80ff4668e0460e5b131",
            ),
            (
                Shape::Chain,
                "fec7ec7be14409b49815cf980c93f1da612c7d5485cf210884f0edd6d014a910",
            ),
        ];
        for (shape, digest) in pinned {
            let graph = UnitGraph::generate(shape, nz(5));
            let json = graph.to_canonical_json();
            let back: UnitGraph = serde_json::from_str(&json).unwrap();
            assert_eq!(back, graph);
            assert_eq!(back.to_canonical_json(), json);
            assert_eq!(
                graph.digest().to_string(),
                digest,
                "{shape:?} graph changed"
            );
        }
    }

    #[test]
    fn rejects_graphs_that_break_their_shape_or_order() {
        let unit = |name: &str, inputs: &[&str]| json!({ "name": name, "inputs": inputs, "action": "empty-output" });
        let graph = |shape: &str, units: Vec<serde_json::Value>| {
            json!({ "schema_version": 1, "shape": shape, "units": units }).to_string()
        };
        let cases = [
            graph("wide", vec![]),
            graph("wide", vec![unit("a", &[]), unit("a", &[])]),
            graph("wide", vec![unit("a", &[]), unit("b", &["a"])]),
            graph("chain", vec![unit("a", &["b"]), unit("b", &[])]),
            graph("chain", vec![unit("a", &[]), unit("b", &[])]),
            graph(
                "chain",
                vec![unit("a", &[]), unit("b", &["a"]), unit("c", &["a"])],
            ),
            graph("ring", vec![unit("a", &[])]),
            json!({ "schema_version": 2, "shape": "wide", "units": [unit("a", &[])] }).to_string(),
        ];
        for case in cases {
            assert!(
                serde_json::from_str::<UnitGraph>(&case).is_err(),
                "accepted {case}"
            );
        }
    }
}
