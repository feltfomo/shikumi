//! the type vocabulary declarations are checked against, as data. a repository writes its
//! extension schemas with `shikumi.lib.types`, whose constructors build tagged attribute sets; they
//! reach the checker serialized like any other value and become a [`Type`] here. Shikumi's own
//! declaration record is a [`Type`] too, built in [`declaration`], so one vocabulary describes
//! both, and the reference page is rendered from it.
//!
//! a repository can only name the public kinds. [`Type::Command`] has no constructor in
//! `shikumi.lib.types` and isn't read from a schema here, because it skips the checks that keep a
//! string out of the document: right for a command line Shikumi never copies, wrong for an
//! extension value it always does.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

pub use crate::contract::Scalar;
use crate::declarations::Node;
use crate::document::Restart;

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Scalar(Scalar),
    Enum(Vec<String>),
    Json,
    List {
        element: Box<Type>,
        non_empty: bool,
    },
    Attrs(Box<Type>),
    Record(Record),
    /// a command line: non-blank text that never reaches the document.
    Command,
}

/// a record's fields in the order they're shown, and an index by name: a repository's
/// `extensions` record has a namespace per schema, which grows with the teams sharing it, and the
/// checker looks up every attribute a declaration sets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Record {
    fields: Vec<(String, Field)>,
    index: HashMap<String, usize>,
}

impl Record {
    pub fn new(fields: Vec<(String, Field)>) -> Self {
        let index = fields
            .iter()
            .enumerate()
            .map(|(position, (name, _))| (name.clone(), position))
            .collect();
        Self { fields, index }
    }

    pub fn fields(&self) -> &[(String, Field)] {
        &self.fields
    }

    pub fn get(&self, name: &str) -> Option<&Field> {
        self.index
            .get(name)
            .map(|&position| &self.fields[position].1)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.fields.iter().map(|(name, _)| name.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub ty: Type,
    pub presence: Presence,
    /// what the field is for, on Shikumi's own records; a repository's schema carries none.
    pub description: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Presence {
    Required,
    /// absent stays absent: the field is left out rather than given a value nobody wrote.
    Optional,
    /// absent means this value, which is part of the schema Shikumi publishes. a repository's
    /// schema can't give one, since its values pass through exactly as written.
    Default(Value),
}

impl Type {
    /// reads a schema a repository wrote with `shikumi.lib.types`, or None when it isn't one built
    /// from the public vocabulary, all the way down.
    pub fn from_schema(node: &Node) -> Option<Self> {
        let fields = attrs(node)?;
        let tag = string(&fields.get("_tag")?.value)?;
        let only = |names: &[&str]| fields.keys().all(|key| names.contains(&key.as_str()));
        let scalar = |ty: Self| only(&["_tag"]).then_some(ty);
        if let Some(&kind) = Scalar::ALL.iter().find(|kind| kind.tag() == tag) {
            return scalar(Self::Scalar(kind));
        }
        match tag {
            "json" => scalar(Self::Json),
            "enum" if only(&["_tag", "values"]) => {
                let Node::List { items } = &fields.get("values")?.value else {
                    return None;
                };
                let values = items
                    .iter()
                    .map(|item| string(item).map(str::to_owned))
                    .collect::<Option<Vec<_>>>()?;
                Some(Self::Enum(values))
            }
            "list" if only(&["_tag", "element", "nonEmpty"]) => {
                let Node::Bool { value: non_empty } = fields.get("nonEmpty")?.value else {
                    return None;
                };
                let element = Self::from_schema(&fields.get("element")?.value)?;
                Some(Self::List {
                    element: Box::new(element),
                    non_empty,
                })
            }
            "attrs" if only(&["_tag", "element"]) => Some(Self::Attrs(Box::new(
                Self::from_schema(&fields.get("element")?.value)?,
            ))),
            "record" if only(&["_tag", "fields"]) => {
                let declared = attrs(&fields.get("fields")?.value)?;
                let mut record = Vec::with_capacity(declared.len());
                for (name, field) in declared {
                    let parts = attrs(&field.value)?;
                    if !parts.keys().all(|key| key == "type" || key == "required") {
                        return None;
                    }
                    let Node::Bool { value: required } = parts.get("required")?.value else {
                        return None;
                    };
                    record.push((
                        name.clone(),
                        Field {
                            ty: Self::from_schema(&parts.get("type")?.value)?,
                            presence: if required {
                                Presence::Required
                            } else {
                                Presence::Optional
                            },
                            description: None,
                        },
                    ));
                }
                Some(Self::Record(Record::new(record)))
            }
            _ => None,
        }
    }

    /// whether a checked value of this type goes into the registry document. a command line is
    /// checked like any other value and then left out, since the document has no field for it.
    pub fn carried(&self) -> bool {
        !matches!(self, Self::Command)
    }

    /// the type as the reference page names it, in `shikumi.lib.types`' own words.
    pub fn describe(&self) -> String {
        match self {
            Self::Scalar(kind) => kind.tag().to_owned(),
            Self::Json => "json".to_owned(),
            Self::Command => "command line".to_owned(),
            Self::Enum(values) => {
                let shown: Vec<String> = values.iter().map(|v| format!("`{v}`")).collect();
                format!("one of {}", shown.join(", "))
            }
            Self::List { element, non_empty } => {
                let list = if *non_empty { "non-empty list" } else { "list" };
                format!("{list} of {}", element.describe())
            }
            Self::Attrs(element) => format!("attribute set of {}", element.describe()),
            Self::Record(_) => "record".to_owned(),
        }
    }
}

fn attrs(node: &Node) -> Option<&BTreeMap<String, crate::declarations::Field>> {
    match node {
        Node::Attrs { fields } => Some(fields),
        _ => None,
    }
}

fn string(node: &Node) -> Option<&str> {
    match node {
        Node::String {
            value,
            context: false,
        } => Some(value),
        _ => None,
    }
}

fn field(ty: Type, presence: Presence, description: &'static str) -> Field {
    Field {
        ty,
        presence,
        description: Some(description),
    }
}

fn record(fields: Vec<(&'static str, Field)>) -> Type {
    Type::Record(Record::new(
        fields
            .into_iter()
            .map(|(name, field)| (name.to_owned(), field))
            .collect(),
    ))
}

fn empty_list() -> Presence {
    Presence::Default(Value::Array(Vec::new()))
}

fn empty_set() -> Presence {
    Presence::Default(Value::Object(serde_json::Map::new()))
}

fn identifiers() -> Type {
    Type::List {
        element: Box::new(Type::Scalar(Scalar::Identifier)),
        non_empty: false,
    }
}

/// what a project declaration may say, given the schemas the repository declared for its
/// extension namespaces. this is the one definition of the declaration record.
pub fn declaration(extensions: &BTreeMap<String, Type>) -> Type {
    use Presence::{Optional, Required};
    let description = || {
        field(
            Type::Scalar(Scalar::Text),
            Required,
            "what it does, in a phrase a person reads",
        )
    };
    let exec = || {
        field(
            Type::Command,
            Required,
            "the command line it runs; it never reaches the registry document",
        )
    };
    record(vec![
        (
            "project",
            field(
                record(vec![
                    (
                        "summary",
                        field(
                            Type::Scalar(Scalar::Text),
                            Required,
                            "what the project is, in one line",
                        ),
                    ),
                    (
                        "maintainers",
                        field(
                            Type::List {
                                element: Box::new(Type::Scalar(Scalar::Text)),
                                non_empty: true,
                            },
                            Required,
                            "who maintains it",
                        ),
                    ),
                    (
                        "dependsOn",
                        field(
                            identifiers(),
                            empty_list(),
                            "listed projects it depends on; a focused composition takes them with it",
                        ),
                    ),
                ]),
                Required,
                "the project itself",
            ),
        ),
        (
            "steps",
            field(
                Type::Attrs(Box::new(record(vec![
                    ("description", description()),
                    ("exec", exec()),
                    (
                        "after",
                        field(
                            identifiers(),
                            empty_list(),
                            "steps of the same project this one runs after when both run",
                        ),
                    ),
                ]))),
                empty_set(),
                "the project's steps, by name",
            ),
        ),
        (
            "commands",
            field(
                Type::Attrs(Box::new(record(vec![
                    ("description", description()),
                    ("exec", exec()),
                ]))),
                empty_set(),
                "entry points on the shell's path, by a name unique across the composition",
            ),
        ),
        (
            "services",
            field(
                Type::Attrs(Box::new(record(vec![
                    ("description", description()),
                    ("exec", exec()),
                    (
                        "restart",
                        field(
                            Type::Enum(
                                Restart::ALL
                                    .iter()
                                    .map(|restart| restart.as_str().to_owned())
                                    .collect(),
                            ),
                            Required,
                            "when the supervisor restarts it",
                        ),
                    ),
                ]))),
                empty_set(),
                "long-running workers, by a name unique across the composition",
            ),
        ),
        (
            "extensions",
            field(
                Type::Record(Record::new(
                    extensions
                        .iter()
                        .map(|(name, ty)| {
                            (
                                name.clone(),
                                Field {
                                    ty: ty.clone(),
                                    presence: Optional,
                                    description: None,
                                },
                            )
                        })
                        .collect(),
                )),
                empty_set(),
                "the repository's own metadata, one namespace per schema its manifest declares",
            ),
        ),
    ])
}

/// the parts of the vocabulary a repository writes extension schemas with, past the scalars in
/// [`Scalar`], and what each accepts, for the reference page.
pub const CONSTRUCTORS: [(&str, &str); 8] = [
    ("enum [ ... ]", "one of the listed strings"),
    (
        "json",
        "any JSON value: strings, numbers, booleans, null, lists and attribute sets of them",
    ),
    ("listOf t", "a list whose elements are `t`"),
    (
        "nonEmptyListOf t",
        "a list of `t` with at least one element",
    ),
    ("attrsOf t", "an attribute set keyed by identifiers, of `t`"),
    (
        "record { ... }",
        "an attribute set with exactly the named fields",
    ),
    ("required t", "a record field that has to be defined"),
    (
        "optional t",
        "a record field that may be left out, and is then absent",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarations::Field as NodeField;

    // a plain JSON value as the serializer writes it, without positions.
    fn node(value: &Value) -> Node {
        match value {
            Value::Null => Node::Null,
            Value::Bool(value) => Node::Bool { value: *value },
            Value::Number(n) => Node::Int {
                value: n.as_i64().unwrap(),
            },
            Value::String(value) => Node::String {
                value: value.clone(),
                context: false,
            },
            Value::Array(items) => Node::List {
                items: items.iter().map(node).collect(),
            },
            Value::Object(fields) => Node::Attrs {
                fields: fields
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.clone(),
                            NodeField {
                                at: None,
                                value: node(value),
                            },
                        )
                    })
                    .collect(),
            },
        }
    }

    // `crates/registry/fixtures/vocabulary.json` is what the Nix constructors build for the schema
    // in `nix/declarations/tests/vocabulary.nix`, which uses every one of them; the `declarations`
    // check holds the Nix side to it, and this test holds the reader to it, so the two halves of the
    // vocabulary can't drift apart.
    #[test]
    fn every_constructor_reads_as_the_type_it_builds() {
        let built: Value =
            serde_json::from_str(include_str!("../fixtures/vocabulary.json")).unwrap();
        let read = Type::from_schema(&node(&built)).unwrap();
        let expected = Type::Record(Record::new(vec![
            (
                "everything".to_owned(),
                Field {
                    ty: Type::List {
                        element: Box::new(Type::Attrs(Box::new(Type::Enum(vec![
                            "a".to_owned(),
                            "b".to_owned(),
                        ])))),
                        non_empty: true,
                    },
                    presence: Presence::Required,
                    description: None,
                },
            ),
            (
                "scalars".to_owned(),
                Field {
                    ty: Type::List {
                        element: Box::new(Type::Record(Record::new(
                            [
                                ("glob", Type::Scalar(Scalar::Glob)),
                                ("identifier", Type::Scalar(Scalar::Identifier)),
                                ("json", Type::Json),
                                ("path", Type::Scalar(Scalar::Path)),
                                ("text", Type::Scalar(Scalar::Text)),
                            ]
                            .into_iter()
                            .map(|(name, ty)| {
                                (
                                    name.to_owned(),
                                    Field {
                                        ty,
                                        presence: Presence::Optional,
                                        description: None,
                                    },
                                )
                            })
                            .collect(),
                        ))),
                        non_empty: false,
                    },
                    presence: Presence::Optional,
                    description: None,
                },
            ),
        ]));
        assert_eq!(read, expected);
    }

    #[test]
    fn a_schema_naming_a_kind_outside_the_public_vocabulary_is_refused() {
        for schema in [
            serde_json::json!({"_tag": "command"}),
            serde_json::json!({"_tag": "schema"}),
            serde_json::json!({"_tag": "record", "fields": {"a": {"type": {"_tag": "command"}, "required": true}}}),
            serde_json::json!({"_tag": "text", "extra": 1}),
            serde_json::json!({"_tag": "enum", "values": [1]}),
            serde_json::json!("not a schema"),
            serde_json::json!({"_tag": "record", "fields": {"a": {"type": {"_tag": "text"}, "required": true, "default": "x"}}}),
        ] {
            assert_eq!(Type::from_schema(&node(&schema)), None, "{schema}");
        }
    }

    #[test]
    fn a_string_that_carries_a_store_reference_is_no_tag() {
        let mut fields = BTreeMap::new();
        fields.insert(
            "_tag".to_owned(),
            NodeField {
                at: None,
                value: Node::String {
                    value: "text".to_owned(),
                    context: true,
                },
            },
        );
        assert_eq!(Type::from_schema(&Node::Attrs { fields }), None);
    }
}
