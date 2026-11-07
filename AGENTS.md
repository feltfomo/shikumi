# Working in Shikumi

The global standing instructions apply. This file adds the rules specific to this repository; where they
disagree, this file wins.

## Where things live

- Plans, roadmaps and research live outside this repository. Nothing here depends on them.
- `docs/` is product documentation: what Shikumi does and how to use it.
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

None yet. The first phase that lands code defines the gate here. Long runs go through the maintainer's
run queue, as the standing instructions say.
