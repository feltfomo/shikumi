//! a synthesized derivation as the JSON `nix derivation add` reads, in each dialect.
//!
//! output paths and output variables are always written out, and every implementation checks
//! them against its own computation (`checkInvariants`), so a Shikumi path that drifted fails the
//! add. CppNix 2.35.3 checks before it writes the `.drv` (`parseJsonAndValidate`); Lix 2.95.4 and
//! 2.96.0 write it first and check after (`lix/nix/derivation-add.cc`), so there a drifted `.drv`
//! is left valid in the store although the add fails.

use serde_json::{Map, Value, json};
use shikumi_derivation::{Ingestion, StoreDerivation, StoreDir, StorePath};

use super::implementation::Dialect;

pub(crate) fn render(dialect: Dialect, dir: &StoreDir, drv: &StoreDerivation) -> Vec<u8> {
    let path = |path: &StorePath| match dialect {
        Dialect::Legacy => dir.display(path),
        Dialect::V4 => path.base_name().to_owned(),
    };
    let mut outputs = Map::new();
    for (name, out) in drv.outputs() {
        let value = match (dialect, drv.fixed()) {
            (_, None) => json!({ "path": path(out) }),
            (Dialect::Legacy, Some(fixed)) => json!({
                "path": path(out),
                "hashAlgo": fixed.method_algo(),
                "hash": fixed.sha256.to_hex(),
            }),
            (Dialect::V4, Some(fixed)) => json!({
                "method": match fixed.ingestion {
                    Ingestion::Flat => "flat",
                    Ingestion::Recursive => "nar",
                },
                "hash": fixed.sha256.to_sri(),
            }),
        };
        outputs.insert(name.as_str().to_owned(), value);
    }
    let sources: Vec<String> = drv.input_sources().iter().map(path).collect();
    let mut inputs = Map::new();
    for (drv_path, names) in drv.input_derivations() {
        let names: Vec<&str> = names.iter().map(|name| name.as_str()).collect();
        inputs.insert(
            path(drv_path),
            json!({ "outputs": names, "dynamicOutputs": {} }),
        );
    }
    let mut document = json!({
        "name": drv.name().as_str(),
        "outputs": outputs,
        "system": drv.system(),
        "builder": drv.builder(),
        "args": drv.args(),
        "env": drv.env(),
    });
    let object = document
        .as_object_mut()
        .expect("the document is built as an object");
    match dialect {
        Dialect::Legacy => {
            object.insert("inputSrcs".to_owned(), json!(sources));
            object.insert("inputDrvs".to_owned(), Value::Object(inputs));
        }
        Dialect::V4 => {
            object.insert("version".to_owned(), json!(4));
            object.insert(
                "inputs".to_owned(),
                json!({ "srcs": sources, "drvs": inputs }),
            );
        }
    }
    serde_json::to_vec(&document).expect("a JSON value always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use shikumi_derivation::{Derivation, FixedHash, OutputName, Outputs, Sha256, StoreName};

    fn fixed() -> StoreDerivation {
        let drv = Derivation::new(
            StoreName::new("fetched").unwrap(),
            Outputs::Fixed(FixedHash {
                ingestion: Ingestion::Recursive,
                sha256: Sha256::digest(b"nar"),
            }),
            "x86_64-linux",
            "/bin/sh",
        );
        StoreDir::default().synthesize(drv).unwrap()
    }

    fn dependent(input: &StoreDerivation) -> StoreDerivation {
        let mut drv = Derivation::new(
            StoreName::new("user").unwrap(),
            Outputs::InputAddressed([OutputName::new("out").unwrap()].into()),
            "x86_64-linux",
            "/bin/sh",
        );
        drv.input_derivation(input, ["out"]).unwrap();
        StoreDir::default().synthesize(drv).unwrap()
    }

    fn parse(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn legacy_json_uses_absolute_paths_and_hex_hashes() {
        let dir = StoreDir::default();
        let input = fixed();
        let doc = parse(&render(Dialect::Legacy, &dir, &dependent(&input)));
        let key = dir.display(input.path());
        assert_eq!(doc["inputDrvs"][&key]["outputs"], json!(["out"]));
        assert!(doc.get("version").is_none());

        let doc = parse(&render(Dialect::Legacy, &dir, &input));
        assert_eq!(doc["outputs"]["out"]["hashAlgo"], "r:sha256");
        assert_eq!(
            doc["outputs"]["out"]["hash"],
            Sha256::digest(b"nar").to_hex()
        );
        assert_eq!(
            doc["outputs"]["out"]["path"],
            dir.display(input.output("out").unwrap())
        );
    }

    #[test]
    fn v4_json_uses_base_names_and_sri_hashes() {
        let dir = StoreDir::default();
        let input = fixed();
        let doc = parse(&render(Dialect::V4, &dir, &dependent(&input)));
        assert_eq!(doc["version"], 4);
        assert_eq!(
            doc["inputs"]["drvs"][input.path().base_name()]["outputs"],
            json!(["out"])
        );

        let doc = parse(&render(Dialect::V4, &dir, &input));
        assert_eq!(
            doc["outputs"]["out"],
            json!({ "method": "nar", "hash": Sha256::digest(b"nar").to_sri() })
        );
    }
}
