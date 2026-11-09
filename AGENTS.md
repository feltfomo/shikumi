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
- `conformance-cppnix-2-35`, `conformance-determinate-3-24`, `conformance-lix-2-95` and
  `conformance-lix-2-96`: one NixOS VM test per Nix implementation, defined in `nix/conformance.nix`.
  Inside each VM an untrusted user runs `shikumi-conformance` three times, through the CLI, the daemon
  socket and `ssh-ng`, comparing every store path Shikumi computes, and what early cutoff and roots do
  with them, with what that VM's daemon registers and reports. Each run ends by collecting garbage in
  the VM's store. These checks exist only on Linux, and they
  need KVM: without `/dev/kvm` and the `kvm` system feature the builder refuses them. Each boots a VM
  and builds its Nix if the cache lacks it; Determinate Nix is built from source.
- `declarations`: the declaration layer's nix-unit tests, in `nix/declarations/tests/`: the factory,
  serialization and its source positions, the vocabulary, `compose` on the fixture repositories
  beside them, and the module adapter in NixOS, in Home Manager from nixpkgs' pin of its source, and
  against a copy of flake-parts' option declarations.
- `registry-no-builds-cppnix-2-35`, `registry-no-builds-determinate-3-24`, `registry-no-builds-lix-2-95`
  and `registry-no-builds-lix-2-96`: under that implementation, the packaged `shikumi` serializes the
  basic and broken fixtures' declarations exactly as `crates/registry/fixtures/` holds them, and
  refuses each planted declaration under `nix/declarations/tests/planted/`, which reaches for a
  build's output or a program; a control evaluation without the producer's refusals has to reach
  it. The document and the rendered findings those declarations give are held by the registry
  crate's tests. Defined, with the next two, in `nix/declarations-checks.nix`, and run by
  `nix/declarations-no-builds.sh`.
- `registry-cli`: the packaged `shikumi`, on one implementation, accepts the basic fixture with its
  tracked document and refuses the broken one with its tracked findings, each on the stream and
  with the exit code each view uses.
- `registry-worktrees`: the document from two Git worktrees of one commit at different paths is
  byte-identical, neither worktree's files change, and a planted location leak makes the two differ.
- `declarations-reference`: `docs/reference/declarations.md` is exactly what `shikumi registry
  reference` generates from the vocabulary.
- `docs-history`: no page under `docs/` outside `docs/records/` carries a history marker: a phase or
  band number, a date, an effort, or changelog phrasing such as "as of" or "no longer".

The checks see only what Git tracks, so a new file needs `git add --intent-to-add` before the gate sees
it. For quick passes while working, run the same tools inside `nix develop`: `cargo nextest run`,
`cargo clippy --all-targets`, and `git ls-files -z | cargo run -q -p repo-check -- links` (or `names`,
or `history`).
One conformance VM builds on its own with `nix build .#checks.x86_64-linux.conformance-lix-2-95`; its
reports are `report-cli.txt`, `report-socket.txt` and `report-ssh.txt` in the result, each beside the
run's stderr, and the build log prints them too.

The declaration checks build alone the same way, such as `nix build .#checks.x86_64-linux.declarations`.
The `registry-no-builds-*`, `registry-cli` and `registry-worktrees` checks keep each run's output and
stderr in their result.

A full gate run is a long run. It goes through the maintainer's run queue, as the standing instructions
say.
