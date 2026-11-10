# `shikumi.lib.types`, the vocabulary a repository writes its extension schemas in. each constructor
# returns plain data that serializes as it stands, and Shikumi's checker, in Rust, owns what every
# type accepts. nothing here checks a value or a schema: a schema built wrong is still data, and the
# checker reports it against the manifest. the checker's own types, such as the one for a command
# line, have no constructor here, so a repository can't name them.
let
  scalar = tag: { _tag = tag; };
in
{
  # non-blank text the document may carry.
  text = scalar "text";
  # a lowercase name: a letter, then letters, digits and hyphens.
  identifier = scalar "identifier";
  # a normalized path relative to the repository root.
  path = scalar "path";
  # a pattern over paths relative to the repository root, such as `docs/**/*.md`.
  glob = scalar "glob";
  enum = values: {
    _tag = "enum";
    inherit values;
  };
  # any JSON value: strings, numbers, booleans, null, lists and attribute sets of them.
  json = scalar "json";
  listOf = element: {
    _tag = "list";
    inherit element;
    nonEmpty = false;
  };
  nonEmptyListOf = element: {
    _tag = "list";
    inherit element;
    nonEmpty = true;
  };
  # an attribute set keyed by identifiers.
  attrsOf = element: {
    _tag = "attrs";
    inherit element;
  };
  record = fields: {
    _tag = "record";
    inherit fields;
  };
  required = type: {
    inherit type;
    required = true;
  };
  # absent stays absent: the field is left out rather than given a value nobody wrote.
  optional = type: {
    inherit type;
    required = false;
  };
}
