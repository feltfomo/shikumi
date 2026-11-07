# Shikumi feature spec

Shikumi is a build system and project registry for multi-language repositories, with the Nix store as its
cache. Its first consumer is a multi-language repository, and that sets the first language set: C, Rust,
Kotlin and Java, plus format, lint and test support for Python and Nushell. Nothing in Shikumi is shaped
only for that repository. Another repository adopts Shikumi by declaring its projects and, for a new
language, adding a backend.

Every capability below is an accepted target unless its status says otherwise. **Open** means the
mechanism is still being chosen and the roadmap names the phase that settles it. Nothing is implemented.

## Decisions

These are settled with the user. Don't reopen them without contradicting evidence.

| Decision | Ruling |
| --- | --- |
| Nix implementations | CppNix and Determinate Nix are primary, Lix is supported. All three must work. |
| Platforms | Every Linux, and macOS. macOS has no test machine yet, so its status stays unverified until one exists. |
| devenv | Not used, by Shikumi or by the first consumer after adoption. Shikumi owns everything devenv did there. |
| Nix dependencies | nixpkgs and nix-unit only. No lexicon, no nix-effects, no flake frameworks. |
| External run queues | No dependency. A lease concept, if Shikumi needs one, is its own. |
| Contracts | JSON with a published schema. People author declarations in Nix. |
| Native code | Rust, wherever Nix evaluation or the module system would be too slow. |
| Evaluation | The real Nix evaluator runs declarations and toolchains. The Rust core writes per-unit `.drv` files itself; Nix doesn't generate them. |
| Rust toolchains | Shikumi never ships a patched rustc. It may use rustc's internals from outside, through `rustc_driver` or a codegen backend, on an unmodified toolchain. |
| Granularity | The goal is granularity below the compiler's coarsest unit. "Per crate" is not an accepted end state for Rust. |
| JVM | Built last. Gradle may run as an opaque step until native JVM support lands. |
| Licence | MPL-2.0. |
| Name | Shikumi. |

## Declarations

- **Projects declare themselves.** A project has one declaration file in its own directory. A repository
  lists its projects in one manifest; nothing is discovered by scanning, and joining is a reviewed
  change to that list.
- **Schema.** Required fields have no silent default. Enumerations are closed. Paths and globs are typed.
- **Validation in one pass.** Every finding across every project is reported at once, each naming its
  option path and the file that defined it. Every entry point refuses the same declarations: build,
  shell, verb, hook, service and query.
- **Diagnostics as data.** The registry document carries its diagnostics, and consumers refuse a
  document with an error.
- **Extension namespace.** A repository adds its own metadata, such as a project kind or a docs layout,
  under a namespace it validates with its own schema. Shikumi passes it through to the
  document unchanged.
- **Compositions.** Global, at the repository root, composes every project plus the remainder.
  Focused, inside a project, holds that project and the toolchains it names. A focused composition runs
  that project's whole loop on a clean machine.
- **The remainder.** Sources no project owns are a first-class scope with their own format, lint and
  checks, in the global composition.
- **The API** keeps devenv's shape: a flat set of small records with one edge model. Praxis's extras are
  optional fields: typed parameters, sensitive inputs, confirmation, timeout and locks, plus a generated
  CLI with listing, planning and completions.

## Registry document

- A versioned JSON document, `shikumi.registry`, with a schema and a major and minor version.
- It holds no store paths and no command strings, and two worktrees of one commit produce identical
  bytes.
- It's available as a store path to every step, command and service, in global and focused
  compositions alike.
- Producing it creates no state in the checkout.
- A read-only viewer prints the project list, one project's detail, or the document.

## Packages, overrides and toolchains

- **Catalog.** Every package a shell or step may use is named once, in the repository's catalog.
  Projects pick by name. A package outside the catalog is refused.
- **Conflicts.** Two definitions of one name on the shell's path, propagated inputs included, are
  refused with both files named. That's the check that catches a JDK 21 against JDK 25 clash.
- **Overrides.** One overlay on the pinned nixpkgs. Each entry states why it exists and what lets it go.
- **Single-owner toolchain settings.** A setting such as the JDK or the Rust toolchain has exactly one
  owning project. Other projects import it by name, and a second definition is refused.
- **Repository tools** are built as derivations, so entering a shell never compiles a tool.

## Lock

- One lock, Shikumi's own, holds everything. Toolchain inputs, nixpkgs and nix-unit, are pinned there
  and fetched by hash. Shikumi doesn't evaluate through flakes, which copy the whole source tree into the
  store on every evaluation miss.
- Third-party artifacts, such as crates and Maven artifacts, are pinned in the same lock:
  content-addressed, one entry per artifact, sorted, sharded by ecosystem and package so merges stay
  local, and searchable with grep. Each entry becomes a fixed-output derivation.
- Native lock data is reused where it carries a usable hash. `Cargo.lock` checksums are sha256 of the
  `.crate` file. Where an ecosystem gives only sha1 or nothing, as Gradle does, Shikumi records sha256
  and the URL itself.
- One lock per repository. No per-directory copies.

## Build graph

- **Cut points per stage.** Each language backend declares the stages of its pipeline where an output can
  be cached on its own, the unit of work at each stage, its declared inputs, and the reproducibility flags
  it needs. Stages stack.
- **Early cutoff on all three implementations.** A rebuilt unit whose output is byte-identical stops the
  rebuild there. Shikumi does this itself by re-adding outputs as content-addressed store paths. See the
  architecture.
- **Evaluation outside the per-edit loop.** A one-file edit costs work proportional to the units it
  touches, not to the size of the repository.
- **Execution mechanism: open.** Candidates are one derivation per unit, batched derivations with
  per-unit outputs, Shikumi running the compiler itself and adding outputs to the store by content, or a
  mix per stage. Band 0 measures them and the numbers decide.
- **Third-party dependencies** are fixed-output derivations from the lock, fetched one by one.
- **Escape hatches.** Code generation, Rust build scripts, proc macros, annotation processors and
  compiler plugins are declared steps with declared inputs and outputs. A step that can't declare its
  inputs is marked impure, runs outside the cache, and is reported as such.
- **Non-redistributable outputs.** An output can be marked so it's never pushed to a public binary
  cache, such as a third-party jar whose licence forbids redistribution.
- **Affected query.** Given a change, Shikumi reports every unit, test and verb it affects, and scopes a
  gate to them.
- **Queries.** Dependencies, reverse dependencies, and why a unit rebuilt.
- **Traces.** Every build can record where time went: evaluation, `.drv` writing, store round trips,
  sandbox setup, compiling and linking. This is maintained product, not a debugging aid.
- **GC roots.** Each worktree's live outputs stay rooted through one manifest per build. Shikumi owns
  eviction, since Nix GC has no LRU.
- **Remote cache and builders.** Shikumi pushes to and substitutes from binary caches, and sends builds
  to remote daemons over `ssh-ng`. It runs as an untrusted daemon user. Two things need the machine's
  admin, and Shikumi detects and reports each when it's missing: a Shikumi cache in
  `trusted-substituters` with its key in `trusted-public-keys`, and, for remote builders, Shikumi's
  signing key trusted by the remote.
- **Provenance.** Every output is either built by a sandboxed derivation or asserted by Shikumi (warm
  state, fingerprint reuse, or Shikumi running the compiler itself). Asserted outputs are never pushed
  to a shared cache until a sandboxed build confirms them.

## Languages

| Language | Cut points known so far | Status |
| --- | --- | --- |
| C | Header discovery, compile to `.o`, archive for shared layers, check steps gating link, link. Assembly and `.i` aren't worth a unit each | Accepted target. clang and gcc are separate toolchain identities |
| Rust | Per crate as the baseline: metadata, codegen, proc macros built for the host, link. Below the crate, two candidates on an unmodified toolchain: store-seeded incremental for frontend and codegen reuse, and a codegen driver caching each CGU's object by its bitcode hash. Upstream's public-API hash for cross-crate cutoff is expected on nightly around 2027 | Open until phase 0.4. See [rust](research/rust.md), [below the crate](research/rust-subcrate.md), [upstream](research/rust-upstream.md) |
| Kotlin and Java | Per source set, with ABI-jar cutoff through `jvm-abi-gen`. Pinned downloads and binary patches as derivations | Last. Gradle as one opaque offline step first |
| Python, Nushell | Format, lint and test steps; no compile | Accepted target |

Every compiling backend also generates the IDE's project model, because the IDE can't read it from a
build tool Shikumi has replaced: `compile_commands.json` for clangd, `rust-project.json` for
rust-analyzer, and a project model for JVM IDEs.

## Steps, verbs and tasks

- **Projects declare steps, never tasks.** Shikumi derives the tasks.
- **Verbs** are derived aggregates over steps. The defaults are `fmt`, `check`, `test`, `gate` and
  `gate-full`; a repository can rename or add verbs. The root verb aggregates every project and the
  remainder.
- **Ordering and selection are separate.** `after` orders steps within a run and never pulls a step into
  it. `afterVerbs` expands to every step a verb selects. A step that waits for a verb selecting it is
  refused.
- **Resource classes.** A step can be exclusive or shared within a run, and can hold named checkout
  resources as locks, taken in a fixed order. A lock is never an edge.
- **Requirement tags.** Free-form tags such as `gpu` or `display` reach the scheduler. The vocabulary
  belongs to the repository.
- **Lease hook.** A step class can be wrapped by a lease provider the repository names. Shikumi doesn't
  ship a lease.
- **Cached steps.** A step with fully declared inputs becomes a derivation and is skipped when they're
  unchanged.
- **Cached tests.** A test target is a derivation, so an unchanged test with unchanged inputs isn't run
  again. Execution follows nextest's model: list the tests, run each in its own process, partition
  stably.
- **Output.** A step's output is visible on success as well as failure.
- **Declared outputs.** A step declares the paths it writes, and two steps declaring overlapping paths
  are refused with both named.
- **Generated files.** A repository can declare files generated from the registry, such as CODEOWNERS.
  Shikumi writes them and a check reports drift.
- **Commands.** Named entry points on the shell's path, with argument forwarding and optional typed
  parameters.

## Shells

- **Activation replaces the path.** The caller's PATH doesn't leak in, and nothing is appended.
- **Environment variables** are declared on projects and toolchains, and reach shells and steps alike.
  Two definitions of one variable are refused with both files named.
- **Entry tasks and banner.** A project can declare steps that run on shell entry, and a banner.
- **Cached entry** at 0.2 s or less when nothing changed, which matches devenv 2.4 with its eval cache.
  Invalidation is exact.
- **Focused and global** shells follow the compositions.
- **Shells for the user's shell.** Bash, zsh and fish at least.
- **Entry refuses invalid declarations** instead of opening a shell over them.

## Services

- **User-level services.** A project declares long-running workers that run in the checkout: started by
  name, foreground or background, one instance per worktree, stopped with SIGTERM and then SIGKILL after
  a timeout. A service runs without entering the shell.
- **Readiness, dependencies, logs and restart policy** are part of a service declaration. Background
  services run under a per-worktree Shikumi supervisor that applies the restart policy, so restarts
  work whether or not a Shikumi command is running.
- **Native services.** Shikumi provides the unopinionated ones that most repositories need. A project
  declares its own by pointing at a program. A doc tool is a service the repository declares, not one
  Shikumi ships.
- **Soft pre-run services.** A bounded advisory service can run before steps without ever failing a verb.

## Hooks

- **Derived from the registry.** Per-project format and lint hooks, scoped to the project's directory,
  plus remainder hooks. Each staged file is checked by exactly one format hook and one lint hook.
- **Per worktree.** Each worktree owns its hook shims. Removing one worktree leaves the others working.
- **The installer owns its shims.** It writes them, retires stale ones and reports drift. A conflicting
  hooks path fails setup and stops shell entry.
- **No shell needed.** Hooks run on a fixed tool path.

## Formatting

- **A format multiplexer** owned by Shikumi. Formatters are declared per project scope, with the
  remainder as its own scope. Each file has exactly one formatter.
- **A missing formatter fails only its own scope.** treefmt v2 refuses to start at all in that case.
- **Cache shared across worktrees**, keyed by file content and formatter identity.
- **Mechanism: open.** Wrap treefmt or build our own, settled when Band 3's formatting phase opens.

## Testing Shikumi itself

- Nix evaluation and module behaviour are tested in pure Nix with nix-unit and flake checks.
- Rust behaviour is tested with nextest.
- `.drv` path computation has a conformance test against each of CppNix, Determinate and Lix.
- Benchmark fixtures are maintained product, so every backend's speed claims are measured the same way.

## Non-goals

- Shipping a patched compiler.
- Reimplementing the Nix evaluator.
- Depending on content-addressed derivations, dynamic derivations, recursive Nix or import-from-derivation.
- Windows.
- Shipping repository-specific tools: doc tools, docs layouts, scratch policies, licence-header rules,
  leases. Those are declarations a repository makes.
- Kori. Kori files are Nix with another extension, so nothing special is needed.
