# Shikumi capabilities

Shikumi is a build system and project registry for multi-language repositories, with the Nix store as
its cache. A repository declares its projects once, and Shikumi derives their builds, dev shells,
verbs, hooks, services and formatting from those declarations. Each language's compiler pipeline is
cut where an output can be cached on its own, so an edit rebuilds only the units it changed.

| Area                                                          | What a repository gets                                    | Status          |
| ------------------------------------------------------------- | --------------------------------------------------------- | --------------- |
| [Where it runs](#where-it-runs)                               | CppNix, Determinate Nix and Lix, on Linux and macOS       | Accepted target |
| [Declarations](#declarations)                                 | projects that declare themselves, validated in one pass   | Accepted target |
| [Registry document](#registry-document)                       | one versioned JSON view of every project                  | Accepted target |
| [Packages and toolchains](#packages-overrides-and-toolchains) | one catalog, one owner per setting                        | Accepted target |
| [Lock](#lock)                                                 | one lock for toolchains and third-party artifacts         | Accepted target |
| [Build graph](#build-graph)                                   | per-stage caching in the store, early cutoff, queries     | Accepted target |
| [Execution mechanism](#execution-mechanism)                   | how each unit actually runs                               | Open            |
| [Languages](#languages)                                       | C, Rust per crate, Kotlin and Java, Python and Nushell    | Accepted target |
| [Rust below the crate](#rust-below-the-crate)                 | caching finer than one crate                              | Open            |
| [IDE project models](#ide-project-models)                     | the model each IDE needs, from the build                  | Accepted target |
| [Steps and verbs](#steps-verbs-and-tasks)                     | derived tasks, cached steps and tests                     | Accepted target |
| [Shells](#shells)                                             | focused and global shells for bash, zsh and fish          | Accepted target |
| [Services](#services)                                         | per-worktree services under Shikumi's own supervisor      | Accepted target |
| [Hooks](#hooks)                                               | per-project format and lint hooks, per worktree           | Accepted target |
| [Formatting](#formatting)                                     | one formatter per file, cache shared across worktrees     | Accepted target |
| [Formatting mechanism](#formatting-mechanism)                 | treefmt wrapped, or Shikumi's own                         | Open            |
| [Rust tests](#testing-shikumi-itself)                         | Shikumi's own Rust tested under nextest                   | Current         |
| [Nix tests and conformance](#testing-shikumi-itself)          | nix-unit tests, `.drv` paths checked on all three         | Accepted target |
| [Benchmark fixtures](#benchmark-fixtures)                     | seeded fixtures, the Cargo baseline, the result record    | Current         |
| [Out of scope](#out-of-scope)                                 | what Shikumi deliberately doesn't do                      | Out of scope    |

## Where it runs

| Target                     | Role                                                    | Status          |
| -------------------------- | ------------------------------------------------------- | --------------- |
| CppNix and Determinate Nix | primary                                                 | Accepted target |
| Lix                        | supported; all three implementations have to work       | Accepted target |
| Linux, any distribution    | target platform                                         | Accepted target |
| macOS                      | target platform, unverified until a test machine exists | Accepted target |
| Windows                    | not a platform                                          | Out of scope    |

Shikumi uses only what the three Nix implementations share, runs as an untrusted daemon user and
never changes the host's Nix. Shikumi itself doesn't use devenv, and a repository that adopts it
needs no devenv either: Shikumi owns what devenv would have done.

## Declarations

Status: accepted target.

- **Projects declare themselves.** A project has one declaration file in its own directory. A
  repository lists its projects in one manifest; nothing is discovered by scanning, and joining is a
  reviewed change to that list.
- **Schema.** Required fields have no silent default. Enumerations are closed. Paths and globs are
  typed.
- **Validation in one pass.** Every finding across every project is reported at once, each naming its
  option path and the file that defined it. Every entry point refuses the same declarations: build,
  shell, verb, hook, service and query.
- **Diagnostics as data.** The registry document carries its diagnostics, and consumers refuse a
  document with an error.
- **Extension namespace.** A repository adds its own metadata, such as a project kind or a docs
  layout, under a namespace it validates with its own schema. Shikumi passes it through to the
  document unchanged.
- **Compositions.** The global composition, at the repository root, holds every project plus the
  remainder. A focused composition, inside a project, holds that project and the toolchains it names,
  and runs that project's whole loop on a clean machine.
- **The remainder.** Sources no project owns are a scope of their own, with their own format, lint and
  checks, in the global composition.
- **The API** keeps devenv's shape: a flat set of small records with one edge model. Optional fields
  add typed parameters, sensitive inputs, confirmation, a timeout and locks, plus a generated CLI with
  listing, planning and completions.

People author declarations in Nix, and the real Nix evaluator runs them.

## Registry document

Status: accepted target.

- A versioned JSON document, `shikumi.registry`, with a schema and a major and minor version.
- It holds no store paths and no command strings, and two worktrees of one commit produce identical
  bytes.
- It's available as a store path to every step, command and service, in global and focused
  compositions alike.
- Producing it creates no state in the checkout.
- A read-only viewer prints the project list, one project's detail, or the document.

## Packages, overrides and toolchains

Status: accepted target.

- **Catalog.** Every package a shell or step may use is named once, in the repository's catalog.
  Projects pick by name. A package outside the catalog is refused.
- **Conflicts.** Two definitions of one name on the shell's path, propagated inputs included, are
  refused with both files named. That's the check that catches a JDK 21 against a JDK 25.
- **Overrides.** One overlay on the pinned nixpkgs. Each entry states why it exists and what lets it
  go.
- **Single-owner toolchain settings.** A setting such as the JDK or the Rust toolchain has exactly one
  owning project. Other projects import it by name, and a second definition is refused.
- **Repository tools** are built as derivations, so entering a shell never compiles a tool.

## Lock

Status: accepted target.

- One lock, Shikumi's own, holds everything. Toolchain inputs, nixpkgs and nix-unit, are pinned there
  and fetched by hash. Shikumi doesn't evaluate through flakes, which copy the whole source tree into
  the store on every evaluation miss.
- Third-party artifacts, such as crates and Maven artifacts, are pinned in the same lock:
  content-addressed, one entry per artifact, sorted, sharded by ecosystem and package so merges stay
  local, and searchable with grep. Each entry becomes a fixed-output derivation.
- Native lock data is reused where it carries a usable hash. `Cargo.lock` checksums are the sha256 of
  the `.crate` file. Where an ecosystem gives only sha1 or nothing, as Gradle does, Shikumi records
  sha256 and the URL itself.
- The lock's format is JSON lines with a published schema, one object per artifact per line.
- One lock per repository. No per-directory copies.

## Build graph

| Capability                                                 | Status          |
| ---------------------------------------------------------- | --------------- |
| Cut points per stage                                       | Accepted target |
| Early cutoff on all three implementations                  | Accepted target |
| Evaluation outside the per-edit loop                       | Accepted target |
| Third-party dependencies from the lock                     | Accepted target |
| Escape hatches for generated code and build-time plugins   | Accepted target |
| Non-redistributable outputs                                | Accepted target |
| Affected query, dependency queries, "why did this rebuild" | Accepted target |
| Build traces                                               | Accepted target |
| GC roots and eviction                                      | Accepted target |
| Remote cache and builders                                  | Accepted target |
| Provenance                                                 | Accepted target |

- **Cut points per stage.** Each language backend declares the stages of its pipeline where an output
  can be cached on its own, the unit of work at each stage, its declared inputs, and the
  reproducibility flags it needs. Stages stack.
- **Early cutoff.** A rebuilt unit whose output is byte-identical stops the rebuild there, on CppNix,
  Determinate Nix and Lix alike. Shikumi does this itself by re-adding outputs as content-addressed
  store paths, without content-addressed derivations.
- **Evaluation outside the per-edit loop.** A one-file edit costs work proportional to the units it
  touches, not to the size of the repository.
- **Third-party dependencies** are fixed-output derivations from the lock, fetched one by one.
- **Escape hatches.** Code generation, Rust build scripts, proc macros, annotation processors and
  compiler plugins are declared steps with declared inputs and outputs. A step that can't declare its
  inputs is marked impure, runs outside the cache, and is reported as such.
- **Non-redistributable outputs.** An output can be marked so it's never pushed to a public binary
  cache, such as a third-party jar whose licence forbids redistribution.
- **Affected query.** Given a change, Shikumi reports every unit, test and verb it affects, and scopes
  a gate to them.
- **Queries.** Dependencies, reverse dependencies, and why a unit rebuilt.
- **Traces.** Every build can record where its time went: evaluation, `.drv` writing, store round
  trips, sandbox setup, compiling and linking. Traces are maintained product, not a debugging aid.
- **GC roots.** Each worktree's live outputs stay rooted through one manifest per build. Shikumi owns
  eviction, since Nix GC has no notion of least recently used.
- **Remote cache and builders.** Shikumi pushes to and substitutes from binary caches, and sends
  builds to remote daemons over `ssh-ng`, as an untrusted daemon user. Two things need the machine's
  admin, and Shikumi detects and reports each when it's missing: a Shikumi cache in
  `trusted-substituters` with its key in `trusted-public-keys`, and, for remote builders, Shikumi's
  signing key trusted by the remote.
- **Provenance.** Every output is either built by a sandboxed derivation or asserted by Shikumi: warm
  state, fingerprint reuse, or Shikumi running the compiler itself. Asserted outputs are never pushed
  to a shared cache until a sandboxed build confirms them.

## Execution mechanism

Status: open.

How each unit runs is still being chosen. The candidates are one derivation per unit, batched
derivations with per-unit outputs, Shikumi running the compiler itself and adding outputs to the store
by content, or a mix per stage. Measurements on the benchmark fixtures decide. Whichever wins, the
build graph's capabilities above hold.

## Languages

Status: accepted target, for every language below. The cut points are the ones known so far.

| Language        | Cut points                                                                                  |
| --------------- | ------------------------------------------------------------------------------------------- |
| C               | header discovery, compile to `.o`, archive for shared layers, check steps gating link, link |
| Rust, per crate | metadata, codegen, proc macros built for the host, link                                     |
| Kotlin and Java | per source set, with ABI-jar cutoff through `jvm-abi-gen`                                   |
| Python, Nushell | format, lint and test steps; no compile                                                     |

- **C.** Assembly and preprocessed `.i` output aren't worth a unit each. clang and gcc are separate
  toolchain identities.
- **Rust.** Per crate is the baseline, not the end state: the goal is granularity below the crate.
- **Kotlin and Java.** Pinned downloads and binary patches are derivations. Until native JVM support
  lands, Gradle may run as one opaque offline step.

## Rust below the crate

Status: open.

Two candidates both work on an unmodified toolchain: store-seeded incremental state, for frontend and
codegen reuse, and a codegen driver that caches each codegen unit's object by its bitcode hash.
Cross-crate cutoff on an interface hash waits on upstream rustc recording one.

## IDE project models

Status: accepted target.

Every compiling backend also generates the IDE's project model, because the IDE can't read it from a
build tool Shikumi has replaced: `compile_commands.json` for clangd, `rust-project.json` for
rust-analyzer, and a project model for JVM IDEs.

## Steps, verbs and tasks

Status: accepted target.

- **Projects declare steps, never tasks.** Shikumi derives the tasks.
- **Verbs** are derived aggregates over steps. The defaults are `fmt`, `check`, `test`, `gate` and
  `gate-full`; a repository can rename them or add its own. The root verb aggregates every project and
  the remainder.
- **Ordering and selection are separate.** `after` orders steps within a run and never pulls a step
  into it. `afterVerbs` expands to every step a verb selects. A step that waits for a verb selecting it
  is refused.
- **Resource classes.** A step can be exclusive or shared within a run, and can hold named checkout
  resources as locks, taken in a fixed order. A lock is never an edge.
- **Requirement tags.** Free-form tags such as `gpu` or `display` reach the scheduler. The vocabulary
  belongs to the repository.
- **Lease hook.** A step class can be wrapped by a lease provider the repository names. Shikumi
  doesn't ship a lease.
- **Cached steps.** A step with fully declared inputs becomes a derivation and is skipped when they're
  unchanged.
- **Cached tests.** A test target is a derivation, so an unchanged test with unchanged inputs isn't
  run again. Execution follows nextest's model: list the tests, run each in its own process, partition
  stably.
- **Output.** A step's output is visible on success as well as failure.
- **Declared outputs.** A step declares the paths it writes, and two steps declaring overlapping paths
  are refused with both named.
- **Generated files.** A repository can declare files generated from the registry, such as
  CODEOWNERS. Shikumi writes them and a check reports drift.
- **Commands.** Named entry points on the shell's path, with argument forwarding and optional typed
  parameters.

## Shells

Status: accepted target.

- **Activation replaces the path.** The caller's PATH doesn't leak in, and nothing is appended.
- **Environment variables** are declared on projects and toolchains, and reach shells and steps
  alike. Two definitions of one variable are refused with both files named.
- **Entry tasks and banner.** A project can declare steps that run on shell entry, and a banner.
- **Cached entry** in 0.2 s or less when nothing changed. Invalidation is exact.
- **Focused and global** shells follow the compositions.
- **Shells for the user's shell.** Bash, zsh and fish at least.
- **Entry refuses invalid declarations** instead of opening a shell over them.

## Services

Status: accepted target.

- **User-level services.** A project declares long-running workers that run in the checkout: started
  by name, foreground or background, one instance per worktree, stopped with SIGTERM and then SIGKILL
  after a timeout. A service runs without entering the shell.
- **Readiness, dependencies, logs and restart policy** are part of a service declaration. Background
  services run under a per-worktree Shikumi supervisor that applies the restart policy, so restarts
  work whether or not a Shikumi command is running.
- **Native services.** Shikumi provides the unopinionated ones most repositories need. A project
  declares its own by pointing at a program. A doc tool is a service the repository declares, not one
  Shikumi ships.
- **Soft pre-run services.** A bounded advisory service can run before steps without ever failing a
  verb.

## Hooks

Status: accepted target.

- **Derived from the registry.** Per-project format and lint hooks, scoped to the project's directory,
  plus remainder hooks. Each staged file is checked by exactly one format hook and one lint hook.
- **Per worktree.** Each worktree owns its hook shims. Removing one worktree leaves the others
  working.
- **The installer owns its shims.** It writes them, retires stale ones and reports drift. A
  conflicting hooks path fails setup and stops shell entry.
- **No shell needed.** Hooks run on a fixed tool path.

## Formatting

Status: accepted target.

- **A format multiplexer** owned by Shikumi. Formatters are declared per project scope, with the
  remainder as its own scope. Each file has exactly one formatter.
- **A missing formatter fails only its own scope.** treefmt v2 refuses to start at all in that case.
- **The cache is shared across worktrees**, keyed by file content and formatter identity.

### Formatting mechanism

Status: open.

The candidates are wrapping treefmt or building Shikumi's own. Either has to keep the guarantees
above, including a missing formatter failing only its own scope.

## Testing Shikumi itself

| Capability                                                                           | Status          |
| ------------------------------------------------------------------------------------ | --------------- |
| Nix evaluation and module behavior tested in pure Nix with nix-unit and flake checks | Accepted target |
| Rust behavior tested with nextest                                                    | Current         |
| `.drv` path computation conformance-tested against CppNix, Determinate Nix and Lix   | Accepted target |
| Benchmark fixtures                                                                   | Current         |

The gate's `nextest` check runs every workspace test today. nix-unit is pinned in the flake, and the
first Nix logic lands with its tests.

## Benchmark fixtures

Status: current.

Every candidate's and every backend's speed claims are measured with the same fixtures, as
[the philosophy](../philosophy.md#speed-is-measured-not-claimed) requires. `shikumi-bench`, in
`bench/`, provides:

- **A seeded Cargo workspace generator.** Crate count, depth, dependencies per crate, modules,
  functions and the share of generic functions are tunable, with an optional large crate. A seed gives
  the same bytes on every release, and generating prints the fixture's digest.
- **An overhead fixture.** A graph of units whose builders do nothing, wide for throughput or a chain
  for latency, so a measurement of it is the floor cost an executor adds per unit.
- **The Cargo baseline.** Plain Cargo builds a generated workspace through four scenarios, cold,
  no-op, a private function edit and a public signature edit, with incremental compilation on and off.
- **The result record.** One JSON line per measured build, in a published schema at version 1, so
  comparing a candidate with the baseline is a join on the record's fields. `validate` checks a
  records file against it.

The record's schema and the overhead fixture's unit graph schema are tracked in `bench/schema/`.
[Contributing](../contributing.md#the-benchmark-harness) has the commands.

## Out of scope

| Excluded | Why |
| --- | --- |
| Shipping a patched compiler | Shikumi works on unmodified toolchains, using their internals only from outside |
| Reimplementing the Nix evaluator | the host's evaluator is the only one that reads a repository's Nix |
| Content-addressed derivations, dynamic derivations, recursive Nix, import-from-derivation | not shared by all three Nix implementations |
| Windows | not a target platform |
| Repository-specific tools: doc tools, docs layouts, scratch policies, licence-header rules, leases | they're declarations a repository makes |
| Depending on an external run queue | a lease, if Shikumi needs one, is its own concept |
