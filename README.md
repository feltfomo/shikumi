# Shikumi

Shikumi is a build system and project registry that uses the Nix store as its cache. Each language's
compiler pipeline is cut at the stage boundaries where an output can be cached on its own, so an edit
rebuilds only the units it actually changed. The projects in a repository declare themselves once, and
Shikumi derives their dev shells, verbs, hooks, services, formatting and build graph from those
declarations.

Status: design approved, nothing implemented yet.

Licence: MPL-2.0 (decided; the licence file lands with the first code).
