# Contributing

Everyone who changes this repository reads this page, people and agents both. It covers the dev
shell, the gate, the benchmark harness, tests, dependencies and scratch. [The philosophy](philosophy.md)
has the reasons behind these rules, and [`docs/ai/`](ai/working.md) adds the rules that apply only to
agents.

Most of Shikumi isn't built yet. What exists is the repository's own tooling (a dev shell, the gate,
the benchmark harness in `bench/` and the gate's checks in `tools/repo-check/`) and the first crates
of the core in `crates/`: derivation synthesis, the store transport and the conformance harness that
checks them against every supported Nix.

## The shell

The flake's default dev shell has every tool the loop needs: Cargo and rustc, rustfmt, Clippy,
cargo-nextest, nixfmt, statix and deadnix. Enter it from the repository root.

```fish
nix develop
```

This flake is the repository's own tooling. Shikumi the product doesn't evaluate through flakes, and
nothing here is a pattern for how a consuming repository will use it.

## The gate

[The root AGENTS.md's Gates section](../AGENTS.md#gates) owns the gate: its checks, what each runs,
the quick forms to run while you work, and what a new file needs before the checks can see it. Run
those quick forms inside the shell.

A format finding is fixed by the formatter, not by hand: `cargo fmt --all` for Rust, and `nixfmt`
on the Nix file it names.

## The benchmark harness

`shikumi-bench`, in `bench/`, generates the benchmark fixtures, runs the plain-Cargo baseline and
validates result records. [The capability page](reference/capabilities.md#benchmark-fixtures) says
what each fixture and the baseline measure.

```fish
cargo run -q -p shikumi-bench -- generate rust-workspace --seed 1 --out .runtime/bench/ws
cargo run -q -p shikumi-bench -- generate overhead --shape chain --units 100 --out .runtime/bench/chain.json
cargo run -q -p shikumi-bench -- cargo-baseline --seed 1 --work-dir .runtime/bench/work --out .runtime/bench/records.jsonl
cargo run -q -p shikumi-bench -- validate .runtime/bench/records.jsonl
```

Generating prints the fixture's digest. The baseline appends one JSON line per build to its `--out`
file. `--help` on any subcommand lists its options.

When you change a type in one of the harness's contracts, regenerate its tracked schema in the same
change; a test fails until you do. [The Rust style guide](reference/rust-style.md#contracts) says how
contract types are written.

```fish
cargo run -q -p shikumi-bench -- schema --write bench/schema
```

## Tests

- Rust behavior is tested with the workspace's own tests under cargo-nextest.
- Nix evaluation and module behavior are tested in pure Nix with nix-unit and flake checks, never
  through a Python or shell driver.
- A test checks behavior the code promises: the regression, the boundary, the state change, the
  failure path. A test that asserts a constructor set the field you passed it proves nothing.
- A test that passes only when run alone is a defect to fix, not a reason to serialize the suite.

## Dependencies

A maintained crate is tested code; a hand-rolled one is untested code. Before adding one, check
whether it's already in the tree, that it's maintained and widely used, that its licence works with
MPL-2.0, and that it doesn't pull in a second async runtime or a second major version of a crate
already here. Declare it once in the root `Cargo.toml`'s `[workspace.dependencies]` and take it in a
crate with `.workspace = true`.

Write it yourself when it's a few lines, when it owns a core invariant, or when the crate brings a
framework to solve a small problem. `bench/src/rng.rs` is the example: the fixture's bytes are a
contract, and `rand` doesn't promise value stability between versions.

The Nix side takes no input beyond nixpkgs and nix-unit.

## Commits

The root [AGENTS.md owns the commit rules](../AGENTS.md#commits). Stage files by name.

## Scratch

Scratch lives where [the root AGENTS.md](../AGENTS.md#where-things-live) puts it. The work that
creates scratch removes it, apart from evidence someone asked to keep, and a probe worth keeping
becomes a maintained test or tool before the work is done.
