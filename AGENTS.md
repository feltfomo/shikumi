# Working in Shikumi

The global standing instructions apply. This file adds the rules specific to this repository; where they
disagree, this file wins.

## Where things live

- `docs/spec.md` is what Shikumi provides, with a status per capability.
- `docs/architecture.md` is how it's put together: owners, flows, invariants and contracts.
- `docs/efforts/<effort>/` holds each effort's page and roadmap. A completed roadmap is never
  overwritten; a new effort gets a new directory.
- `docs/research/` holds the research reports the design rests on. They're evidence, read-only once
  landed. A later finding goes in a new report that links to the one it supersedes.
- `.runtime/` is gitignored scratch and evidence. Phase evidence is kept under
  `.runtime/<effort>/<phase>/` in the main checkout.

## Worktrees

Phase worktrees live outside the checkout, in the maintainer's worktree container, one directory per
phase, and are removed after their commit is verified and evidence is copied. Never put a worktree
inside the checkout.

## Protected invariants

These hold for every change. A change that needs one relaxed goes to the architect first.

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
