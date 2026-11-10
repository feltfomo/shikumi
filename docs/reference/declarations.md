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
| `project.imports` | attribute set of identifier | default `{ }` | toolchains it uses from other projects: each toolchain's name, and the project that defines it, which a focused composition takes with it |
| `packages` | list of identifier | default `[ ]` | catalog packages on its shells' path, by catalog name |
| `env` | attribute set of text, or a catalog package's location, `{ package; path; }` by variable name | default `{ }` | environment variables its shells and steps get, by name |
| `toolchains` | attribute set of records | default `{ }` | toolchain settings it owns, by a name no other project defines; other projects import them |
| `toolchains.<name>.description` | text | required | what it does, in a phrase a person reads |
| `toolchains.<name>.packages` | list of identifier | default `[ ]` | catalog packages on the path of every project that uses it |
| `toolchains.<name>.env` | attribute set of text, or a catalog package's location, `{ package; path; }` by variable name | default `{ }` | environment variables every project that uses it gets |
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
| `overrides` | refused: the one overlay is the catalog's `overrides` | optional | never set: the repository has one overlay, the catalog's |

Each namespace under `extensions` is checked against the schema the manifest declares for it, and a namespace without one is refused. A catalog package's location, the value of a variable under `env`, has these fields:

| Field | Type | Presence | What it is |
| --- | --- | --- | --- |
| `package` | identifier | required | the catalog package, by catalog name |
| `path` | path | default `null` | a path inside it; null points at the package itself |

## The manifest

`shikumi.nix` at the repository root, a record or a function of `{ lib, types }` returning one.

| Field | Type | Presence | What it is |
| --- | --- | --- | --- |
| `projects` | attribute set of path | required | each project's directory, by project name |
| `extensions` | attribute set of schema | default `{ }` | the schema of each extension namespace, written with `shikumi.lib.types` |
| `catalog` | record | optional | the repository's catalog; without one, a project can pick no package |
| `catalog.directory` | path | required | the directory holding the catalog's file |
| `catalog.systems` | non-empty list of text | required | the systems its package set is read for, each one the pinned nixpkgs knows, such as `x86_64-linux` |

## The catalog

`catalog.nix` in the directory the manifest names, a record or a function of `{ lib }` returning one. It names each package by its attribute path in the pinned nixpkgs, which Shikumi resolves for each of the manifest's systems with the catalog's `overrides` applied as its one overlay. `<attribute>` stands for the name of the nixpkgs attribute an entry replaces or adds.

| Field | Type | Presence | What it is |
| --- | --- | --- | --- |
| `packages` | attribute set of nixpkgs attribute path | default `{ }` | every package a shell or step may use, by the name projects pick it with, each its attribute path in the pinned nixpkgs |
| `overrides` | attribute set of records | default `{ }` | the one overlay on the pinned nixpkgs, an entry per attribute it replaces or adds |
| `overrides.<attribute>.reason` | text | required | why the entry exists |
| `overrides.<attribute>.release` | text | required | what lets it go |
| `overrides.<attribute>.package` | function | required | `final: prev: ...`, what the attribute becomes, as in an overlay |
| `overlays` | refused: the one overlay is the catalog's `overrides` | optional | never set: the repository has one overlay, the catalog's |

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
| `location` | text that contains the repository's location, or a value built from where the evaluator reads it |
| `unknown-reference` | a name that refers to no project, step, catalog package or toolchain |
| `cycle` | projects that depend on each other, or steps that run after each other, in a loop |
| `duplicate` | a name that has one owner defined twice: a command, service or toolchain more than one project declares, or an environment variable a shell gets from two places |
| `conflict` | two different packages on one shell's path, propagated inputs included, under one package name or program name |
| `second-overlay` | an overlay outside the catalog's own, which is the repository's one overlay |
| `no-declaration` | a listed directory with no declaration file, or a catalog directory with no catalog file |
| `not-a-declaration` | a declaration file that doesn't return a `shikumi.project` result |
| `overlap` | a listed directory at or inside another project's directory |
| `syntax` | a declaration or manifest file that isn't valid Nix |
| `undefined-variable` | a variable a file uses that nothing defines |
| `type-error` | evaluation stopped at an operation on a value of the wrong type, such as calling something that isn't a function |
| `thrown` | evaluation reached a `throw` or an `abort` |
| `assertion` | evaluation reached an `assert` whose condition is false |
| `invalid-utf8` | text that isn't valid UTF-8, which the registry document can't carry |
| `not-keyed` | an import or read of a file that isn't a `.nix` file Git tracks inside a listed project's or the catalog's directory, reached through no symlink |
| `impure` | a read that would make the declarations depend on the machine or the moment: a file outside the repository, a store path, the clock, the host's system, the search path or a URL |
| `evaluation` | evaluation failed in a way Shikumi doesn't recognize, or the evaluator stopped without saying why |
| `shikumi-bug` | evaluation failed inside Shikumi's own Nix, which is a bug in Shikumi rather than in the declaration |
