# Foundation effort (Band 0)

Status: **approved 2026-11-07, not started.**

## Goal

Prove the foundation before anything is built on it. Rust is the first language measured, because
cached Rust builds below the crate have never been done and carry the most risk. Measure the ways
Shikumi could execute fine-grained units through the Nix store against plain Cargo on all three Nix
implementations, and settle how Rust gets granularity below the crate. The results decide the
architecture's two open sections.

| | |
| --- | --- |
| Branch | `effort/foundation` |
| Worktrees | `foundation-<phase>` in the maintainer's worktree container, one per phase, removed at phase closure |
| Evidence | `.runtime/foundation/<phase>/` in the main checkout |
| Owned paths | The repository root's Nix and Cargo files, `bench/`, the store and derivation crates, `docs/research/` additions, `AGENTS.md`'s gate section, `LICENSE` |
| Changes no published interface | It creates the first ones: the benchmark record schema and the derivation and transport crates |
| Depends on | Nothing |
| Merge order | First |

## Phases

### 0.1 Repository bootstrap and benchmark harness

- **Goal.** The repository's dev shell, its gate, and a maintained benchmark harness every later backend
  measures itself with.
- **Scope.**
  - A flake with nixpkgs and nix-unit as its only inputs. A dev shell with the Rust toolchain from
    nixpkgs. A Cargo workspace.
  - The MPL-2.0 licence file.
  - A fixture generator for Rust, the first language measured. It's seeded and deterministic, and it
    makes a workspace whose crate count, crate size, module count, generics and dependency depth are
    tunable, including one large crate for the below-crate work.
  - A language-independent overhead fixture: N units whose builders do nothing, which measures the
    floor cost of one unit with no compile time mixed in.
  - A Cargo baseline runner for the scenarios Band 0 measures: cold build, no-op, one-function edit,
    public signature edit. It runs with incremental on and off.
  - Result records as JSON lines with a published schema: machine, Nix implementation and version,
    candidate, transport, scenario, unit count, wall time, CPU time.
  - The gate, written into `AGENTS.md`.
- **Acceptance.**
  - The same seed produces a byte-identical fixture, and a different seed a different one. A test asserts
    both.
  - The Cargo baseline runs every scenario on the maintainer's Linux machine and writes valid records.
  - The record schema rejects a malformed record in a Rust test.
  - The gate passes.
- **Gate.** The flake's checks, covering formatting, clippy and nextest, run through the run queue.
- **Closure evidence.** The gate log and the baseline records.

### 0.2a Derivation synthesis, CLI transport and conformance harness

- **Depends on** 0.1.
- **Goal.** Writing `.drv` files and computing store paths exactly as every implementation does, and the
  harness everything after this is tested by.
- **Scope.**
  - ATerm synthesis and path computation for input-addressed and fixed-output derivations, and
    content-addressed path computation from a NAR hash, name and references.
  - One transport interface, with its CLI implementation.
  - A conformance harness against CppNix 2.35, Determinate 3.24 and Lix 2.95 and 2.96, run in NixOS VM
    tests so the host's Nix isn't touched. Every check runs as an **untrusted** daemon user, because a
    trusted test user would hide failures.
- **Acceptance.**
  - Computed `.drv` paths equal what `nix derivation add` returns on every implementation. The set
    covers: no inputs, input sources, input derivations, multiple outputs, and a fixed-output derivation.
  - Computed content-addressed paths equal what the daemon registers for the same bytes and references.
  - The CLI transport adds, builds and queries on every implementation as an untrusted user.
- **Gate.** 0.1's gate plus the VM conformance checks, through the run queue.
- **Closure evidence.** The gate log, and the conformance results for each implementation.
- **Subagents.** An adversarial reviewer for the conformance set, kept apart from the implementing
  context, is worth it here. Drv-path drift is the failure the whole design can't survive.

### 0.2b Worker-protocol transport and re-addressing

- **Depends on** 0.2a.
- **Goal.** The daemon client and early cutoff by re-addressing, as maintained code.
- **Scope.**
  - The worker-protocol implementation of the transport interface, over a unix socket and `ssh-ng`.
  - Adding sources as content-addressed paths, batched validity queries, re-adding a built output, and
    registering indirect roots.
  - Re-adding computes the content-addressed path locally first and transfers bytes only when that path
    isn't already valid.
- **Acceptance**, on every implementation, as an untrusted user:
  - Both transports build the same derivation to the same output.
  - **Early cutoff, positive.** A test builds unit A, re-adds its output, and writes B against it. It then
    rebuilds A so the output comes out identical, and asserts that B's `.drv` path is unchanged and
    already valid without B being built, and that the re-add transferred nothing. The test has to fail
    if re-addressing is removed.
  - **Early cutoff, negative.** A changed output gives B a new path that isn't valid yet.
  - **Self-reference.** Re-adding an output that contains its own store path is refused with a
    diagnostic naming the unit. The test fails if such an output is re-added.
  - `AddIndirectRoot` registers a root manifest.
- **Gate.** 0.2a's gate.
- **Closure evidence.** The gate log, and the conformance results for each implementation.

### 0.3 Execution measurement

- **Depends on** 0.1 and 0.2b.
- **Goal.** Choose the execution mechanism and the transport by measurement.
- **Settles.** The architecture's "Execution: open" section, and which transport is the default.
- **Scope.**
  - Candidate executors: one derivation per unit; batched derivations with per-unit outputs; Shikumi
    running the compiler itself and adding outputs by content. They're experiments. They live in the
    phase's `.runtime` and don't land.
  - The matrix:
    - **Implementations:** CppNix, Determinate with cgroups on and off, and Lix, each in a VM. Lix also on
      the host's bare metal, for absolute numbers.
    - **Transports:** worker protocol and CLI.
    - **Scenarios,** on the Rust fixture unless noted: cold build; overhead per unit, on the do-nothing
      fixture; no-op; one-function edit; public signature edit in a crate many others depend on; an
      edit whose output is byte-identical, where early cutoff must skip everything downstream;
      substitution from a local cache; and GC with 10 rooted manifests over 20k unit paths.
    - **Latency floor:** a chain of N do-nothing units, N from 1 to 30, per transport, so the edit-loop
      bar is a curve of depth against time, not a single point.
    - **Candidate 3 per platform:** the cost of the sandbox Shikumi would have to own, on Linux and,
      where a machine exists, macOS.
    - **Store at scale:** GC and registration on a store that already holds a few hundred thousand small
      paths.
  - A report, landed as `docs/research/execution.md`, with the records it rests on kept as evidence.
- **Acceptance.**
  - Every matrix cell has records, or an explicit failure with its cause.
  - The report compares each candidate with Cargo against the proposed bars:
    - overhead of 15 ms or less per unit on Linux;
    - a cold build within 15% of Cargo;
    - an edit loop of 1 s or less on top of compile time;
    - a no-op of 300 ms or less;
    - measurable early cutoff.
  - It recommends a mechanism, and says what each losing candidate would need in order to win.
- **Gate.** 0.2b's gate. The measurement runs themselves are long runs: they need the user's approval and
  a quiet machine, and run through the run queue alone, never beside another measurement.
- **Closure evidence.** The report, the raw records with a content manifest, the fixture seeds, and the
  versions of every implementation.

### 0.4 Rust below the crate

- **Depends on** 0.1 only. It's the highest-risk question in the design, so it starts as soon as 0.1
  closes and runs alongside 0.2a and 0.2b.
- **Goal.** Prove which mechanisms give Rust granularity below the crate on an unmodified toolchain, and
  what each saves on a one-function edit. Background:
  [rust-subcrate](../../research/rust-subcrate.md) and [rust-upstream](../../research/rust-upstream.md).
- **Settles.** The shape of Band 1's Rust work, and whether the codegen driver is worth its upkeep of one
  port per rustc release.
- **Scope.** Experiments, so they live in the phase's `.runtime` and don't land. The findings land as
  `docs/research/rust-spike.md`.
  - **Fixtures.** A generated Rust fixture with a large crate (many modules, generics, a few dependency
    crates), plus one real-world crate from a private repository, built out of tree at a recorded commit.
  - **Store-seeded incremental.** An unmodified stable rustc. Shikumi keeps the incremental snapshot in
    the store, copies it into a scratch directory, normalizes session names and temp suffixes, and
    re-archives the rlib.
  - **Codegen driver.** Built against the stable toolchain's `rustc-dev` with `RUSTC_BOOTSTRAP=1`. Each
    CGU's name derives from its own module, and the object is cached keyed by a hash of the unoptimized
    bitcode plus codegen settings. nixpkgs may not ship `rustc-dev`; if it doesn't, the phase fetches the
    official component as a fixed-output derivation, not through rust-overlay.
  - **Linking.** lld and wild on the same edits, since relinking is the fixed cost after any edit.
  - **Optional.** `-Clinker-plugin-lto` with lld's ThinLTO cache, for release builds.
- **Acceptance.**
  - **Seeded output equals a fresh build.** After normalization, a build seeded from a snapshot produces
    a byte-identical rlib, and a byte-identical linked binary, to a fresh build without a snapshot. That
    holds for the generated fixture and the real-world crate, after a one-function edit and after a public
    signature edit. The snapshot is only a speed hint, and this is the proof.
  - **Snapshots move.** A snapshot taken in one checkout path seeds a build in another, with the same
    result.
  - **CGU hashes are stable.** The driver's per-CGU bitcode hashes are identical across two runs and two
    checkout paths. A one-function edit changes only the CGUs that contain the function or instantiate it.
    The report lists exactly which ones.
  - **Time is broken down.** For each mechanism and for plain Cargo incremental: frontend, codegen and
    link time on the one-function edit, against a fresh per-crate build.
  - **The SVH check.** A test builds crate B against crate A, changes a private body in A, and tries to
    use B's old output with A's new one. It records whether rustc refuses it. That confirms or overturns
    the ruling that Rust cross-crate memo reuse is off.
  - The report recommends a Rust strategy and names every rustc patch that would have helped, each with
    the outside alternative chosen instead.
- **Gate.** 0.1's gate. Measurement runs go through the run queue on a quiet machine.
- **Closure evidence.** The report, the raw timings, the toolchain versions, and the hashes behind each
  acceptance claim.

## Closure

Band 0 closes when 0.3 and 0.4 are closed and the architect has:

1. ruled on the execution mechanism and the Rust strategy;
2. updated the spec and architecture on main;
3. written full phase pages for Band 1a, and for Band 2 if it hasn't opened.

The closure owes the user the opener for Band 1a, and for Band 2 if it didn't open beside Band 0.

## Baton

- **Next action:** approved by the user on 2026-11-07. The orchestrator opens phase 0.1.
- **Open decisions for the user:**
  - the speed-bar decision in [the roadmap](../../roadmap.md#decisions-waiting-on-the-user);
  - approving 0.3's measurement runs when they're ready;
  - whether a macOS CI runner should be arranged.
- **Not to relitigate:** [the spec's decisions](../../spec.md#decisions).

## Limits

None yet.
