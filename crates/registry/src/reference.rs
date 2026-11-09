//! the declarations reference page, rendered from the vocabulary: the declaration record Shikumi
//! checks, the manifest, the constructors a repository writes extension schemas with, and the
//! finding codes. `docs/reference/declarations.md` is this page as tracked, and the gate's
//! `declarations-reference` check fails when the two differ.

use std::fmt::Write as _;

use crate::document::Code;
use crate::layer::{DECLARATION, MANIFEST};
use crate::vocabulary::{
    CONSTRUCTORS, Definition, Field, NAME_PLACEHOLDER, Presence, Scalar, Type, names,
};

const INTRODUCTION: &str = "\
# Declarations reference

What a project's declaration and the repository's manifest may contain, and the finding codes the
checker reports. `shikumi registry reference` generates this page from the vocabulary the checker
checks with, so it can't describe anything else; regenerate it rather than editing it.
[The capability page](capabilities.md#declarations) says how declarations work.
";

// the text before a definition's table, naming each file and field from its own definition.
fn before(definition: Definition) -> String {
    match definition {
        Definition::Declaration => format!(
            "\n## The declaration record\n\n\
             A project's `{}` returns `shikumi.project` applied to a record with these fields. A \
             field marked required has no default; one with a default takes it when it's absent. \
             `{NAME_PLACEHOLDER}` stands for any identifier.\n\n",
            DECLARATION.name
        ),
        Definition::Manifest => format!(
            "\nEach namespace under `{}` is checked against the schema the manifest declares for \
             it, and a namespace without one is refused.\n\
             \n## The manifest\n\n\
             `{}` at the repository root, a record or a function of `{}` returning one.\n\n",
            names::EXTENSIONS,
            MANIFEST.name,
            MANIFEST.head()
        ),
    }
}

/// the page, as `shikumi registry reference` prints it.
pub fn page() -> String {
    let mut text = String::from(INTRODUCTION);
    for &definition in Definition::ALL {
        text.push_str(&before(definition));
        table(&mut text, &definition.record());
    }
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
    walk("", record, &mut |path, shown, field| {
        let presence = match &field.presence {
            Presence::Required => "required".to_owned(),
            Presence::Optional => "optional".to_owned(),
            Presence::Default(empty) => format!("default `{}`", empty.nix()),
        };
        writeln!(
            text,
            "| `{path}` | {shown} | {presence} | {} |",
            field.description.unwrap_or("")
        )
        .unwrap();
    });
}

// every field of a record, depth first in the record's order, with its path as the page writes it
// and its type as the page shows it: a record's fields follow it, and so do those of a record each
// entry of an attribute set holds, under `<name>`.
fn walk(prefix: &str, ty: &Type, visit: &mut impl FnMut(&str, &str, &Field)) {
    let Type::Record(record) = ty else {
        return;
    };
    for (name, field) in record.fields() {
        let path = format!("{prefix}{name}");
        let (shown, inner) = match &field.ty {
            Type::Record(_) => ("record".to_owned(), Some((format!("{path}."), &field.ty))),
            Type::Attrs(element) if matches!(**element, Type::Record(_)) => (
                "attribute set of records".to_owned(),
                Some((format!("{path}.{NAME_PLACEHOLDER}."), &**element)),
            ),
            other => (other.describe(), None),
        };
        visit(&path, &shown, field);
        if let Some((prefix, inner)) = inner {
            walk(&prefix, inner, visit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // every definition the page has to render, named as the test reports it.
    fn definitions() -> Vec<(String, Type)> {
        Definition::ALL
            .iter()
            .map(|definition| (format!("{definition:?}"), definition.record()))
            .collect()
    }

    // each field of each definition whose row the page lacks: its path, type, presence and a
    // description, as the definition says them.
    fn missing_rows(page: &str) -> Vec<String> {
        let mut missing = Vec::new();
        for (name, definition) in definitions() {
            walk("", &definition, &mut |path, shown, field| {
                let row = format!("| `{path}` | {shown} |");
                let described = field
                    .description
                    .is_some_and(|description| page.contains(&format!("| {description} |")));
                if !page.contains(&row) || !described {
                    missing.push(format!("{name}: {path}"));
                }
            });
        }
        missing
    }

    // the page is generated, and the gate compares the tracked page with the generator; this holds
    // the generator to the definitions, so a field the checker reads can't be left off the page.
    #[test]
    fn every_field_of_every_definition_has_a_row_with_its_description() {
        assert_eq!(missing_rows(&page()), Vec::<String>::new());
        for (name, definition) in definitions() {
            walk("", &definition, &mut |path, _, field| {
                assert!(
                    field.description.is_some(),
                    "{name}: {path} has no description"
                );
            });
        }
    }

    #[test]
    fn a_page_without_one_of_the_rows_is_caught() {
        let whole = page();
        for (removed, definition) in [
            ("| `projects` |", "Manifest: projects"),
            (
                "| `steps.<name>.after` |",
                "Declaration: steps.<name>.after",
            ),
        ] {
            let page: String = whole
                .lines()
                .filter(|line| !line.starts_with(removed))
                .map(|line| format!("{line}\n"))
                .collect();
            assert_eq!(missing_rows(&page), [definition]);
        }
    }

    // the page says how each file the layer reads is called, as the layer calls it.
    #[test]
    fn the_page_names_each_files_arguments_as_the_layer_offers_them() {
        let page = page();
        for file in [MANIFEST, DECLARATION] {
            let named = format!("`{}`", file.name);
            assert!(page.contains(&named), "{named}");
        }
        let head = format!("a function of `{}`", MANIFEST.head());
        assert!(
            page.contains(&head),
            "the page doesn't say the manifest is {head}"
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
