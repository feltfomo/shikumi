//! Shikumi's benchmark harness: fixture generators, the Cargo baseline runner and the result
//! record every executor measures itself with.

pub mod baseline;
pub mod contract;
pub mod fixture;
pub mod measure;
pub mod record;
pub mod steps;

mod rng;

/// the published schemas, by the file name they're tracked under in `bench/schema/`.
pub fn schemas() -> [(&'static str, schemars::Schema); 2] {
    [
        ("record.schema.json", record::record_schema()),
        (
            "overhead-units.schema.json",
            fixture::overhead::unit_graph_schema(),
        ),
    ]
}

/// how a schema is written to disk, so the tracked file and the drift test agree byte for byte.
pub fn render_schema(schema: &schemars::Schema) -> String {
    let mut json = serde_json::to_string_pretty(schema).expect("a schema always serializes");
    json.push('\n');
    json
}

#[cfg(test)]
mod tests {
    // the tracked schemas are generated from the types; this fails when one drifts, and
    // `shikumi-bench schema --write bench/schema` regenerates them.
    #[test]
    fn tracked_schemas_match_the_types() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema");
        for (name, schema) in super::schemas() {
            let tracked = std::fs::read_to_string(dir.join(name))
                .unwrap_or_else(|e| panic!("reading {name}: {e}"));
            assert!(
                tracked == super::render_schema(&schema),
                "bench/schema/{name} is stale; run `shikumi-bench schema --write bench/schema`"
            );
        }
    }
}
