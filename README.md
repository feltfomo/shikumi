# Shikumi

Shikumi is a build system and project registry that uses the Nix store as its cache. Each language's
compiler pipeline is cut at the stage boundaries where an output can be cached on its own, so an edit
rebuilds only the units it actually changed. The projects in a repository declare themselves once, and
Shikumi derives their dev shells, verbs, hooks, services, formatting and build graph from those
declarations.

Status: the design is approved, and the product is partly built. The repository has its dev shell, its
gate, the benchmark harness that every speed claim is measured with, and the core's first crates.
Projects declare themselves and come out as one validated registry document, which `shikumi registry`
prints. [The capability page](docs/reference/capabilities.md) gives each part's status.

The docs start at [docs/README.md](docs/README.md).

Licence: MPL-2.0. The text is in [LICENSE](LICENSE).
