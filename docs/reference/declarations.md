# Declarations reference

What a project's declaration and the repository's manifest may contain, and the finding codes the
checker reports. `shikumi registry reference` generates this page from the vocabulary the checker
checks with, so it can't describe anything else; regenerate it rather than editing it.
[The capability page](capabilities.md#declarations) says how declarations work.

## The declaration record

A project's `project.nix` returns `shikumi.project` applied to a record with these fields. A field marked required has no default; one with a default takes it when it's absent. `<name>` stands for any identifier.

| Field | Type | Presence | What it is |
| --- | --- | --- | --- |
| `project` | record | required | the project itself |
| `project.summary` | text | required | what the project is, in one line |
| `project.maintainers` | non-empty list of text | required | who maintains it |
| `project.dependsOn` | list of identifier | default `[ ]` | listed projects it depends on; a focused composition takes them with it |
| `steps` | attribute set of records | default `{ }` | the project's steps, by name |
| `steps.<name>.description` | text | required | what it does, in a phrase a person reads |
| `steps.<name>.exec` | command line | required | the command line it runs; it never reaches the registry document |
| `steps.<name>.after` | list of identifier | default `[ ]` | steps of the same project this one runs after when both run |
| `commands` | attribute set of records | default `{ }` | entry points on the shell's path, by a name unique across the composition |
| `commands.<name>.description` | text | required | what it does, in a phrase a person reads |
| `commands.<name>.exec` | command line | required | the command line it runs; it never reaches the registry document |
| `services` | attribute set of records | default `{ }` | long-running workers, by a name unique across the composition |
| `services.<name>.description` | text | required | what it does, in a phrase a person reads |
| `services.<name>.exec` | command line | required | the command line it runs; it never reaches the registry document |
| `services.<name>.restart` | one of `never`, `on-failure`, `always` | required | when the supervisor restarts it |
| `extensions` | record | default `{ }` | the repository's own metadata, one namespace per schema its manifest declares |

Each namespace under `extensions` is checked against the schema the manifest declares for it, and a namespace without one is refused.

## The manifest

`shikumi.nix` at the repository root, a record or a function of `{ types }` returning one.

| Field | Type | Presence | What it is |
| --- | --- | --- | --- |
| `projects` | attribute set of path | required | each project's directory, by project name |
| `extensions` | attribute set of schemas | default `{ }` | the schema of each extension namespace, written with `shikumi.lib.types` |

## Types

The constructors in `shikumi.lib.types`, which extension schemas are written with.

| Constructor | Accepts |
| --- | --- |
| `text` | non-blank text |
| `identifier` | an identifier: a lowercase letter, then lowercase letters, digits and hyphens |
| `path` | a normalized path relative to the repository root: no leading or trailing slash, no empty, `.` or `..` part |
| `glob` | a non-blank pattern relative to the repository root, with no leading slash and no `..` part |
| `enum [ ... ]` | one of the listed strings |
| `json` | any JSON value: strings, numbers, booleans, null, lists and attribute sets of them |
| `listOf t` | a list whose elements are `t` |
| `nonEmptyListOf t` | a list of `t` with at least one element |
| `attrsOf t` | an attribute set keyed by identifiers, of `t` |
| `record { ... }` | an attribute set with exactly the named fields |
| `required t` | a record field that has to be defined |
| `optional t` | a record field that may be left out, and is then absent |

Text, identifiers, paths, globs and JSON strings can't contain the repository's location or refer to a store path, since the registry document carries neither.

## Finding codes

| Code | What it reports |
| --- | --- |
| `missing` | a required field has no value |
| `type` | a value of the wrong kind, such as a string where a list belongs |
| `invalid` | a value of the right kind that breaks its type's rule |
| `enum` | a value outside a closed enumeration |
| `unknown-option` | a field the record doesn't have |
| `store-reference` | text that refers to a store path |
| `location` | text that contains the repository's location |
| `unknown-reference` | a name that refers to no project or step |
| `cycle` | projects that depend on each other, or steps that run after each other, in a loop |
| `duplicate` | a command or service name more than one project declares |
| `no-declaration` | a listed directory with no declaration file |
| `not-a-declaration` | a declaration file that doesn't return a `shikumi.project` result |
| `overlap` | a listed directory at or inside another project's directory |
