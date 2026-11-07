# Working in Shikumi

The global standing instructions apply. This file adds the rules specific to this repository; where they
disagree, this file wins.

## Where things live

- Plans, roadmaps and research live outside this repository. Nothing here depends on them.
- `docs/` is product documentation: what Shikumi does and how to use it. Its front door is
  [docs/README.md](docs/README.md). [The philosophy](docs/philosophy.md) holds the laws behind these
  rules. Read [contributing](docs/contributing.md) before any change, [writing](docs/writing.md) before
  any doc or comment, and [working as an agent](docs/ai/working.md) if you're one. The Nix and Rust
  style guides are in [docs/reference/](docs/reference/).
- `.runtime/` is gitignored. Scratch work and anything you keep as evidence of a run go there, never in
  a tracked path.

## Worktrees

Worktrees live outside the checkout, never inside it.

## Protected invariants

These hold for every change. Relaxing one is a design decision, not something a change does on its own.

- Shikumi runs on CppNix, Determinate Nix and Lix. It uses only what all three share: input-addressed
  and fixed-output derivations, content-addressed store paths and the daemon worker protocol. No
  content-addressed derivations, dynamic derivations, recursive Nix or import-from-derivation.
- The Nix side depends on nixpkgs and nix-unit and nothing else. The Rust side may use crates under the
  dependency rules in the standing instructions.
- The host's Nix is never changed. Other implementations are tested in VMs or separate stores.
- Every machine contract is JSON with a published schema, validated in Rust through serde.
- Each concern has one owning component. A superseded path is removed in the change that replaces it.
- No repository-specific vocabulary in Shikumi. A consuming repository fills Shikumi's extension slots
  from its own declarations.
- This repository is public. It never names, describes or vendors code from a private consumer.
- A claimed speedup is measured on the benchmark fixtures, not asserted.

## Commits

Subject `area: message`, lowercase, imperative. A body is a blank line then `- ` bullets, one imperative
sentence per change. No attribution trailers. Stage by name.

## Gates

The gate is `nix flake check` from the repository root. It builds every check in the flake's `checks`
output for the current system:

- `format`: `cargo fmt --check` over the workspace and `nixfmt --check` over every `.nix` file.
- `nix-lint`: statix and deadnix over every `.nix` file.
- `clippy`: Clippy on every workspace target, warnings denied.
- `nextest`: the workspace's tests under cargo-nextest, including a real Cargo baseline run over a tiny
  generated workspace.
- `links`: every relative link in a tracked Markdown file reaches a tracked path, and a fragment into a
  Markdown file names one of its headings or HTML anchors.
- `private-names`: no tracked file or path contains a protected name. The names are held only as salted
  digests in `tools/repo-check`; add one with `repo-check digest`, which reads it from stdin.

The checks see only what Git tracks, so a new file needs `git add --intent-to-add` before the gate sees
it. For quick passes while working, run the same tools inside `nix develop`: `cargo nextest run`,
`cargo clippy --all-targets`, and `git ls-files -z | cargo run -q -p repo-check -- links` (or `names`).

A full gate run is a long run. It goes through the maintainer's run queue, as the standing instructions
say.
