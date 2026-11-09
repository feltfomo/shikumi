//! the declarations reference page, rendered from the vocabulary: the declaration record Shikumi
//! checks, the manifest, the constructors a repository writes extension schemas with, and the
//! finding codes. `docs/reference/declarations.md` is this page as tracked, and the gate's
//! `declarations-reference` check fails when the two differ.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::document::Code;
use crate::vocabulary::{self, CONSTRUCTORS, Presence, Scalar, Type};

const INTRODUCTION: &str = "\
# Declarations reference

What a project's declaration and the repository's manifest may contain, and the finding codes the
checker reports. `shikumi registry reference` generates this page from the vocabulary the checker
checks with, so it can't describe anything else; regenerate it rather than editing it.
[The capability page](capabilities.md#declarations) says how declarations work.
";

/// the page, as `shikumi registry reference` prints it.
pub fn page() -> String {
    let mut text = String::from(INTRODUCTION);
    text.push_str(
        "\n## The declaration record\n\n\
         A project's `project.nix` returns `shikumi.project` applied to a record with these fields. \
         A field marked required has no default; one with a default takes it when it's absent. \
         `<name>` stands for any identifier.\n\n",
    );
    table(&mut text, &vocabulary::declaration(&BTreeMap::new()));
    text.push_str(
        "\nEach namespace under `extensions` is checked against the schema the manifest declares for \
         it, and a namespace without one is refused.\n",
    );
    text.push_str(
        "\n## The manifest\n\n\
         `shikumi.nix` at the repository root, a record or a function of `{ types }` returning one.\n\n\
         | Field | Type | Presence | What it is |\n\
         | --- | --- | --- | --- |\n\
         | `projects` | attribute set of path | required | each project's directory, by project name |\n\
         | `extensions` | attribute set of schemas | default `{ }` | the schema of each extension namespace, written with `shikumi.lib.types` |\n",
    );
    text.push_str(
        "\n## Types\n\n\
         The constructors in `shikumi.lib.types`, which extension schemas are written with.\n\n\
         | Constructor | Accepts |\n\
         | --- | --- |\n",
    );
    for scalar in Scalar::ALL {
        writeln!(text, "| `{}` | {} |", scalar.tag(), scalar.accepts()).unwrap();
    }
    for (constructor, accepts) in CONSTRUCTORS {
        writeln!(text, "| `{constructor}` | {accepts} |").unwrap();
    }
    text.push_str(
        "\nText, identifiers, paths, globs and JSON strings can't contain the repository's location \
         or refer to a store path, since the registry document carries neither.\n",
    );
    text.push_str("\n## Finding codes\n\n| Code | What it reports |\n| --- | --- |\n");
    for code in Code::ALL {
        writeln!(text, "| `{code}` | {} |", code.meaning()).unwrap();
    }
    text
}

fn table(text: &mut String, record: &Type) {
    text.push_str("| Field | Type | Presence | What it is |\n| --- | --- | --- | --- |\n");
    rows(text, "", record);
}

fn rows(text: &mut String, prefix: &str, ty: &Type) {
    let Type::Record(record) = ty else {
        return;
    };
    for (name, field) in record.fields() {
        let path = format!("{prefix}{name}");
        let (shown, inner) = match &field.ty {
            Type::Record(_) => ("record".to_owned(), Some((format!("{path}."), &field.ty))),
            Type::Attrs(element) if matches!(**element, Type::Record(_)) => (
                "attribute set of records".to_owned(),
                Some((format!("{path}.<name>."), &**element)),
            ),
            other => (other.describe(), None),
        };
        let presence = match &field.presence {
            Presence::Required => "required".to_owned(),
            Presence::Optional => "optional".to_owned(),
            Presence::Default(value) => format!("default `{}`", nix(value)),
        };
        writeln!(
            text,
            "| `{path}` | {shown} | {presence} | {} |",
            field.description.unwrap_or("")
        )
        .unwrap();
        if let Some((prefix, inner)) = inner {
            rows(text, &prefix, inner);
        }
    }
}

// a default as a Nix expression; the defaults are empty lists and attribute sets.
fn nix(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Array(_) => "[ ]",
        _ => "{ }",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_of_the_declaration_record_has_a_row_and_a_description() {
        let page = page();
        for row in [
            "| `project.summary` | text | required |",
            "| `project.dependsOn` | list of identifier | default `[ ]` |",
            "| `steps.<name>.exec` | command line | required |",
            "| `services.<name>.restart` | one of `never`, `on-failure`, `always` | required |",
            "| `extensions` | record | default `{ }` |",
        ] {
            assert!(page.contains(row), "missing {row:?}:\n{page}");
        }
        assert!(
            !page.contains("|  |"),
            "a field has no description:\n{page}"
        );
    }

    #[test]
    fn every_code_has_a_row() {
        let page = page();
        for code in Code::ALL {
            assert!(page.contains(&format!("| `{code}` |")), "{code}");
        }
    }
}
