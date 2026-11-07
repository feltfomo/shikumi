# Shikumi architecture

This page says how Shikumi is put together: who owns what, how a build and a shell entry flow, and which
invariants hold. [The spec](spec.md) says what it provides. Sections marked **open** name the phase that
settles them in [the foundation roadmap](efforts/foundation/README.md).

## Shape

```mermaid
graph LR
  decl["project declarations<br/>(Nix)"] --> compose["declaration layer<br/>shikumi.lib (Nix)"]
  lock["shikumi lock"] --> core
  compose -->|"registry document,<br/>toolchain context"| core["core<br/>shikumi (Rust)"]
  src["source tree"] --> backends["language backends<br/>(Rust, in the core)"]
  backends --> core
  core -->|".drv files, sources,<br/>build requests"| transport["store transport"]
  transport --> daemon["nix daemon<br/>CppNix / Determinate / Lix"]
  daemon --> store[("/nix/store")]
  core --> env["shells, verbs, hooks,<br/>services, formatting"]
```

There are two halves. The **declaration layer** is Nix and is small: it turns project declarations into
the registry document and a toolchain context. The **core** is one Rust binary, `shikumi`, and does
everything that scales with the size of the repository: unit discovery, `.drv` synthesis, scheduling,
early cutoff, caching, and the environment features.

## The declaration layer

- **Entry point.** `shikumi.lib.compose { root; manifest; }` is pure: it reads the manifest, imports each
  listed project's declaration, and returns the registry document, the toolchain context and the shell
  environments. nix-unit tests call it directly, with no CLI.
- **Schema and validation.** The schema is a Nix module set with real required options. Validation runs
  one pass over every project and returns diagnostics with option paths and defining files. The module
  system handles hundreds of records; nothing per compile unit goes through it.
- **Toolchain context.** The store paths and settings a backend needs to write `.drv` files: compiler,
  linker and builder paths, base environment, system and flags. It's small and changes only when the
  lock or a declaration changes.
- **No builds during evaluation.** Nothing in the layer forces a derivation output.

The API keeps devenv's flat shape. Lexicon is a reference for how factories bind declarations, not a
dependency.

## The core

One process per command. One Tokio runtime per process. CPU work such as hashing and scanning runs on a
bounded blocking pool and never on the runtime's workers. Every task belongs to the command that started
it and is cancelled with it; Ctrl-C stops outstanding builds through the transport before the process
exits.

### Context cache

The core evaluates the toolchain context through the host's Nix CLI and caches the result.

- **The evaluation can read only what the key hashed.** The keyed file set is the lock, Shikumi's own
  Nix sources, and every tracked `.nix` file inside the manifest's project directories and the catalog.
  The core copies exactly that set into the store with a filtered `builtins.path` and evaluates from the
  copy. An import from outside the set, or of an untracked file, fails loudly and names the file. So the
  key is exact by construction, and the set is also the declared list of what declarations may import.
  Evaluating from a store copy also keeps the evaluator away from sockets and FIFOs in the checkout.
- **The key also covers the evaluator:** its implementation and version, the system, `NIX_PATH`, and the
  experimental features and settings that change evaluation.
- **No flake entry.** Evaluating a flake copies the whole source tree into the store on CppNix and Lix,
  which costs seconds per miss on a large repository. The layer has a plain entry point instead, and
  the lock's toolchain pins are fetched as fixed-output sources by hash.
- **Cheap hits.** A stat cache (size, mtime, ctime, inode) per keyed file means a hit rehashes only files
  whose stat changed, the way git and devenv keep this cheap. A hit then costs one `git ls-files` plus a
  stat per keyed file, which is what the 0.2 s shell-entry target rests on. Band 3's acceptance measures
  it on a large generated repository, not only on the fixtures.

### Language backends

A backend is a Rust component compiled into the core. For v1 there's no dynamic plugin ABI. Each backend
owns:

- **Discovery.** Finding units and their inputs. For C that's `clang-scan-deps` on the host with a
  two-level cache: explicit inputs, then the recorded discovered inputs, re-checked by content, the
  scheme ccache, Zig and Buck2 use. For Rust it's `cargo metadata` for crates, the resolved features and
  the lock, never `cargo build`. The sandboxed build checks the discovered inputs, so a stale cache entry
  fails loudly.
- **Actions.** For each stage, a unit's command, its declared inputs as content-addressed store paths,
  its toolchain context and its reproducibility flags (`-ffile-prefix-map`, `--remap-path-prefix`,
  `--remap-path-scope=all`, stable `-C metadata`, deterministic jar ordering).
- **IDE metadata.** `compile_commands.json`, `rust-project.json` or the JVM project model.

The cut points each backend uses are in the research reports. JVM is last.

**Rust below the crate is open** until phase 0.4. Both candidates work on an unmodified toolchain and
introduce concepts the core must own regardless of which wins:

- **Warm state.** A store-seeded incremental snapshot is an input that may change how fast a unit
  builds but never what it produces. It isn't part of the unit's cache key. Phase 0.4 proves on fixtures
  that output with and without it is byte-identical after normalization. That proves the mechanism, not
  each build, so warm-built outputs are asserted under the provenance rule below. Warm state never
  crosses into a unit's declared inputs.
- **Cutoff by fingerprint.** Byte-identical outputs aren't the only cutoff signal. The codegen driver
  keys a CGU's object on a hash of its unoptimized bitcode, the JVM backend can key downstream compiles
  on an ABI jar's hash, and upstream rustc's planned public-API hash keys dependents on the interface.
  The core keeps one memo table for these. Its entries are owned by the backend that defines the
  fingerprint, and each carries the backend's **load-compatibility condition**: when a reused output is
  still accepted by whatever consumes it. Reused outputs are asserted under the provenance rule.
  - Within a crate, CGU objects keyed by bitcode hash are joined by the linker, not loaded by rustc, so
    reuse is sound.
  - Across crates, Rust reuse is off. rustc records each dependency's crate hash (SVH) in a dependent's
    metadata and refuses a dependent built against a different one. Reuse turns on only once rustc
    records the public-API hash there instead, and the upstream plan says whether it will. Phase 0.4
    runs the failing case to confirm the check.
  - The re-addressing mechanism above is the case where the fingerprint is the output's own bytes.

### Derivation synthesis

The core writes ATerm `.drv` files itself and computes their paths. It never asks the evaluator to
generate per-unit derivations. A one-file edit writes O(affected units) `.drv` files.

- Unit derivations are input-addressed. Their `inputSrcs` are content-addressed store paths only:
  sources, and the re-addressed outputs of upstream units. Their `inputDrvs` are the toolchain context
  derivations, which rarely change.
- Unit builders aren't stdenv. stdenv derives `-frandom-seed` from `$out`, so it changes on every edit,
  and its setup cost adds to the overhead of every derivation. Shikumi ships minimal builders of its own.
- **Invariant:** the computed `.drv` path matches what each implementation computes, bit for bit. A
  conformance test against CppNix, Determinate and Lix guards it.

### Early cutoff

Lix removed content-addressed derivations, and CppNix and Determinate still mark them experimental, so
Shikumi does early cutoff itself. See [nix: early cutoff without CA derivations](research/nix.md).

1. A unit builds.
2. The core reads its NAR hash and references from the daemon and computes the content-addressed path
   for those bytes locally, with the same conformance-tested path code. If that path is already valid,
   nothing is transferred. Only new bytes are copied in, which is exactly when the downstream rebuild
   costs more anyway.
3. Downstream units name that path. Identical bytes give an identical downstream `.drv`, whose output is
   already valid, so nothing rebuilds.

Cutoff is only as good as the backends' reproducibility, so every backend has a determinism check that
builds a unit twice in separate sandboxes and compares the bytes.

**No self-references.** A content-addressed source path can't reference itself, so a re-addressed unit
output must contain no reference to its own store path. Each backend's determinism check also scans the
output for its own path. Units whose output has to self-reference, such as a shared library with a
RUNPATH to itself or a wrapper script, are leaves that aren't re-addressed. They need no cutoff, because
nothing in the graph depends on their bytes.

**Dataflow, not barriers.** A downstream `.drv` can only be written once its inputs are re-addressed, so
the scheduler starts each unit as soon as its own inputs are ready, never waiting for a level of the
graph. The latency floor of an edit is then the depth of the edited path times one step's overhead: the
derivation's fixed cost, the re-add, and a validity query. Band 0 measures that floor as a curve.

### Provenance

Some mechanisms put an output in front of a consumer without a Nix sandbox having produced it from the
unit's declared inputs: warm state, reuse through the fingerprint memo, and Shikumi running the compiler
itself. Nix can't tell such an output from a real one, because an input-addressed path is accepted from
whoever registers it first, and a binary cache then hands it to everyone.

So every output carries a provenance, owned by the core:

- **Built:** a sandboxed derivation produced it from its declared inputs.
- **Asserted:** Shikumi produced or reused it some other way.

An asserted output is local only. It's never pushed to a shared cache, and never re-addressed into a
downstream input that gets pushed, until a sandboxed build confirms its bytes. The push path enforces
that. CI and the push path build cold under the sandbox, so shared caches hold only built outputs. On a
developer machine, asserted outputs are bounded by a periodic cold rebuild of the units they cover.

### Execution: open

Band 0 measures the candidates on the same fixture and the numbers decide:

| Candidate | Sandbox per unit | Strength | Cost |
| --- | --- | --- | --- |
| One derivation per unit | Yes | Simplest, hermetic, remote builders work unchanged | 8 to 15 ms of fixed overhead per unit on Linux |
| Batched derivations with per-unit outputs | One per batch | Fewer sandbox setups, still cached per unit | Batch boundaries decide what rebuilds |
| Shikumi runs the compiler and adds outputs by content | Shikumi's own | Fastest edit loop, allows warm compiler processes | Shikumi guarantees hermeticity, and remote builds need their own path |
| A mix per stage | Varies | Each stage gets the cheapest fitting mechanism | More than one execution path to keep correct |

Whichever wins, the action, its inputs and its output path are the same records, so the choice is
internal to the execution component and changing it later doesn't touch backends.

Candidate 3 is admissible only with a sandbox Shikumi owns that enforces "only declared inputs visible",
and its outputs are asserted until a derivation build confirms them. There's no namespace sandbox on
macOS, so phase 0.3 costs it per platform, not on Linux alone.

### Store transport

All daemon access goes through one trait with two implementations:

- **Worker protocol.** The only interface CppNix, Determinate and Lix all share, frozen at the 2.18 level
  in Lix. Over a unix socket locally and `ssh-ng` remotely. `psyclyx/fix` already runs against both
  CppNix and Lix daemons. snix's `nix-compat` has framing and ATerm code to reuse or cross-check.
- **CLI.** `nix-store --realise` over many paths per invocation. It always works and serves as the
  conformance oracle.

Lix plans to replace the legacy protocol with Cap'n Proto RPC once that's stable. When it does, that
becomes a third implementation of the same trait. The core owns global concurrency: a pool of
long-lived connections and one build-slot semaphore. It doesn't rely on daemon-side `max-jobs`, which is
counted per connection.

**Trust.** Shikumi works as an untrusted daemon user. Adding `.drv` files, building them, adding
content-addressed paths with references, and registering indirect roots are all expected to work
untrusted; phases 0.2a and 0.2b prove it with an untrusted test user on every implementation. Two things need
the machine's admin, and the core detects and reports each instead of failing silently:

- **Substitution** from a Shikumi cache needs the cache in `trusted-substituters` and its key in
  `trusted-public-keys`.
- **Remote builders** need input-addressed inputs that don't verify themselves (toolchain context
  outputs, Shikumi's builders, locally built tools) to be signed. Shikumi signs them with a key the
  remote's admin adds to `trusted-public-keys`. Content-addressed and fixed-output paths need no
  signature.

### Caches and roots

- **Action index.** A Shikumi-level index of which unit outputs are known to exist locally and in each
  binary cache, so the core asks a cache only for paths it expects to find. A cache can publish one
  manifest per commit, so a fresh worktree fetches one manifest and then targeted NARs.
- **GC roots.** Each build writes one manifest store path whose references are every live unit output and
  `.drv`. The worktree roots that manifest at `.shikumi/roots/`, plus a bounded number of previous
  generations. A deleted worktree leaves a dangling root, which the next GC collects.
- **Eviction.** Nix GC has no LRU, so Shikumi keeps a shared warm manifest of recently hit units with its
  own last-used times, and leaves the rest to the daemon's GC.
- **Non-redistributable marks** travel with an output, and the push path refuses to upload a marked path
  to a public cache.
- **One owner for shared state.** Commands run concurrently: two builds in two worktrees, or a build
  beside a format. The action index, provenance records, the warm manifest with its last-used times, the
  memo table and the format cache live in one per-user state database that every command writes through
  transactions, so the database is their single owner. Each worktree's root generations are written by
  atomic rename under a per-worktree lock. The database's schema is versioned like any contract.

### Environment features

Each has one owner in the core, and each reads the registry document rather than its own copy:

- **Task engine.** Verbs, steps, `after` and `afterVerbs`, resource classes, checkout locks, requirement
  tags, the lease hook, soft edges for advisory pre-run services, and cached steps as derivations.
  Ordering and selection are different relations in its graph.
- **Service manager.** Per worktree, independent of the shell. A background service runs under a
  per-worktree Shikumi supervisor process, which owns every service's process group, readiness, logs,
  restart policy and shutdown. The supervisor lives exactly as long as at least one service in its
  worktree does, and exits when the last stops. Shikumi uses its own supervisor rather than systemd
  `--user` or launchd, because not every Linux runs systemd and one implementation behaves the same
  on every platform. Its lifetime rules, each with a test that drives it:
  - **Supervisor death.** Per-worktree state records each service's pid, process group and start time,
    so a reused pid isn't mistaken for a live service. If the supervisor dies, the next command adopts or
    stops the orphaned services before anything starts. Test: kill -9 the supervisor, start the service,
    get exactly one instance.
  - **No exit race.** One per-worktree lock decides supervisor lifetime. The supervisor exits only while
    holding it with zero services, and a client that finds no live supervisor under the lock starts one.
    Test: start and stop in a tight loop from two processes, never two supervisors and never a lost start.
  - **Worktree removal.** A supervisor whose worktree directory is gone stops its services and exits.
    Test: `git worktree remove` with a service running.
  - **Version skew.** The control protocol carries a version. A CLI that finds a supervisor on another
    version either speaks that version or asks it to hand its services over to the new binary. It never
    sends a command the old one would misread. Test: a newer CLI against an older supervisor.
- **Hook installer.** Writes, retires and reports its own shims under the worktree's hooks path, and
  dispatches staged files to exactly one format and one lint scope each.
- **Format multiplexer.** One formatter per file, scoped per project, with a content-keyed cache shared
  across worktrees. Treefmt or our own is **open**.
- **Shell activation.** Builds the environment from the cached context, replaces PATH, and emits the
  activation for bash, zsh or fish.
- **Query and trace.** The affected query, dependencies, reverse dependencies and "why did this rebuild",
  plus build traces with time by phase.

## Contracts

| Contract | Format | Owner | Readers |
| --- | --- | --- | --- |
| Registry document `shikumi.registry` | JSON with a schema, major and minor version | Declaration layer produces it, the core validates it | Every step and service, repository tools, the viewer |
| Lock | Sharded, sorted text with one entry per artifact | Core | Core, reviewers, grep |
| Build trace | JSON lines with a schema | Core | Benchmarks, profiling tools |
| Cache manifest | JSON with a schema | Core | Core on other machines |
| State database | Versioned schema with migrations | Core | Core, across Shikumi versions |

A contract changes only through the architect, by a version bump the schema records.

## Scale

Shikumi is designed for repositories of 100k source files, a few thousand crates and many teams.

- **Edit loop:** O(affected units) `.drv` writes and builds, plus one batched validity query. The
  evaluator isn't in it.
- **No-op build:** one batched validity query against the action index. Target 300 ms or less.
- **Shell entry on a cache hit:** a stat per keyed Nix file, rehashing only changed ones. Target 0.2 s or less.
- **Store:** one path per unit stage, so millions of small paths over time. GC and SQLite registration at
  that scale are measured in Band 0 before anything depends on them.
- **Lock:** linear in the number of artifacts, sharded so concurrent changes rarely conflict.

## Invariants

- Only the subset all three implementations share: input-addressed and fixed-output derivations,
  content-addressed store paths and the worker protocol.
- No import-from-derivation and no builds during evaluation.
- The host's Nix evaluator is the only evaluator that reads a repository's Nix. Shikumi never bundles one.
- The registry document holds no store paths and is byte-identical across worktrees of one commit.
- Each concern has one owner. The registry document is the only source of project facts.
- Outputs of cached units are reproducible, and the determinism check enforces it per backend.
- A cached output was produced with only its declared inputs visible.
- An asserted output is never shared until a sandboxed build confirms its bytes.
- A re-addressed output contains no reference to its own store path.
- No repository-specific vocabulary in Shikumi. Extension slots carry it.
