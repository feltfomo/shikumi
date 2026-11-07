# Shikumi: the Nix substrate

Research date: 2026-11-07. Host: Lix 2.95.4 (`nix --version`). I ran no builds and made no store writes.

Marking: **[V]** means I read it in a primary source this session (release notes, source, an issue or PR). **[R]** means a secondary or third-party report. **[U]** means unverified, from memory or inference, and needs checking before anyone relies on it.

## Versions in scope

| Implementation | Latest | Date | Source |
|---|---|---|---|
| CppNix (NixOS/nix) | 2.35.4 | 2026-10-22 | github.com/NixOS/nix/tags, endoflife.date/nix [V] |
| Determinate Nix | 3.24.2, based on upstream 2.35.3 | 2026-11-03 | manual.determinate.systems/release-notes-determinate/v3.24.2 [V] |
| Lix | 2.96.0 "Bingsu" (2.96.1 announced, not yet out) | 2026-10-13 | docs.lix.systems rl-2.96, lix.systems/blog/2026-10-13-lix-2.96-release [V] |

The host runs Lix 2.95.4, a security point release from 2026-10-13, one major behind.

## Headline

1. **Lix removed CA derivations, dynamic derivations, impure derivations, recursive Nix and `fetchClosure`.** The first three went in 2.94.0 (2025-11-17), recursive Nix earlier (RFD #767) and `fetchClosure` in 2.95. Any `.drv` that uses them can't even be read by Lix any more [V]. So a design that has to run on all three implementations can't rest on Nix-native early cutoff or on RFC 92. Shikumi has to do early cutoff itself, with plain input-addressed derivations and content-addressed *store paths* (as opposed to CA *derivations*). There's a concrete way to do this; see "Early cutoff without CA derivations".
2. **The only programmatic interface all three share is the legacy worker (daemon) protocol, frozen at its Nix 2.18 level in Lix.** Lix has no C API, and the Lix team says it intends to remove the legacy protocols once its Cap'n Proto RPC is stable [V]. That's the largest long-term portability risk.
3. **Per-derivation build overhead is roughly 8 to 30 ms of wall time on Linux** in recent measurements, and it's mostly scheduler and sandbox management, not the kernel. Determinate and Lix both cut it substantially during 2026. A compile unit that takes 200 ms or more absorbs that. A 20 ms unit doesn't.
4. **Evaluation must not scale with the number of units on every invocation.** The Rust core should synthesize `.drv` files itself, which is exact, cheap and portable, and use the Nix evaluator only for the toolchain context, which changes rarely.

## Capability matrix

| Feature | CppNix 2.35.4 | Determinate 3.24.2 | Lix 2.96.0 |
|---|---|---|---|
| `ca-derivations` | Experimental. Stabilisation milestone #35 is about 71% done (updated 2026-08-14) [V]. 2.35 reworked realisations into a "build trace", keyed by `drvPath^out`, and removed deep realisations. The binary cache directory moved to `build-trace-v2/`, which is incompatible with older caches [V] | Experimental, inherited from 2.35.3. 3.24.2 fixes CA crashes: enabling the flag after the store was opened, `queryRealisation` on `ssh://`, and a "callback called twice" in sandboxed builds [V] | **Removed** in 2.94 [V] |
| Early cutoff via CA | Works in principle. Build trace GC "not yet implemented" [V]. Resolving unresolved derivations is now slower because of the 2.35 change [V] | Same as CppNix | n/a |
| `dynamic-derivations` (RFC 92) | Experimental. Ericson says the "basic primitives are feature complete" (working group, 2026-08) [R]. Hydra support is still in PRs. Cachix and nixbuild don't support them yet [R] | Flag present (listed in the manual) [V]. No sign of Determinate using it; its own fine-grained build (nix-make) avoids it [V] | **Removed** in 2.94 [V] |
| `recursive-nix` | Experimental [V] | Inherited [U] | **Removed** (RFD #767, 2025) [R] |
| `impure-derivations` | Experimental | Inherited | Removed in 2.94 [V] |
| IFD | Yes. Blocks the evaluating thread | Yes. `builtins.parallel` overlaps IFDs: `parallel-eval-test#ifd` went from 50 s to 10 s [V] | Yes |
| Parallel evaluation | No. Edolstra's multithreaded evaluator PR #10938 was closed and moved to Determinate. Small "parallel-eval friendly" PRs are still landing upstream [V] | Yes: `eval-cores`, fibers since 3.23, parallel instantiation of drv dependencies since 3.24 [V] | No [U] |
| `builtins.wasm` | No. An upstream PR exists [V] | Experimental (`wasm-builtin`) [V] | No [U] |
| C API (libstore-c, libexpr-c) | Yes, explicitly unstable. Store side has `nix_add_derivation`, `nix_derivation_from_json`, `nix_store_realise` (blocking, single path), copy and closure. No GC-root or add-path functions, and `build_paths`/`ensure` are commented out [V] | Yes, but versions are skewed against upstream (nix-bindings-rust #50) [V] | **None.** nixpkgs added `nixForLinking`/`nixComponents` because Lix lacks it [V] |
| Worker protocol | Current upstream minor version [U: exact number] | Same plus Determinate extensions [U] | Frozen at the 2.18 level. Protocols older than 2.18 were removed in 2.94 [V]. Experimental capnp RPC ships behind `rpc-sockets` (`lix-xp-1`), "NO stability guarantees". Legacy protocols to be removed after RPC stabilises [V]. `ssh://` deprecated in 2.96 and to be removed in 2.97. `ssh-ng://` is stable [V] |
| Sandbox launch cost work | PR #15611 ("Worker not choke on 70k derivation builds"): the event loop still blocks on sandbox setup and teardown, and `killUser()` took 150 ms from fork slowness [V] | 3.24.2 creates builders directly in their cgroup: 10,000 trivial drvs went from 164.5 s to 127.9 s. Child spawn on macOS is faster with high fd limits [V] | 2.95 launches builders with vfork through a libexec helper, making management overhead constant: 3000 small `runCommand` went from 80 s to 24 s. On macOS it calls `sandbox_init_with_parameters` directly [V] |
| `external-builders` (pluggable sandbox) | Experimental flag (milestone 62) [V] | Inherited [U] | No [U] |
| macOS | Supported. 2.35.3 fixed a seatbelt bypass (GHSA-m4vq-qf6v-82pj) [V] | Supported | Supported. Gives GC TCC diagnostics in 2.96.1 [V] |

## 1. Content-addressed derivations

- **Status.** CA derivations are experimental in CppNix and Determinate and gone from Lix [V]. The 2.35 release reshaped them substantially. Build-trace entries are now keyed by `/nix/store/…-foo.drv^out` instead of hash modulo, deep realisations are gone, the binary cache layout moved to `build-trace-v2/<drv>/<out>.doi`, and the JSON shape changed [V]. 2.35 also fixed a self-reference hash collision that **changes CA store paths for self-referential NARs** compared with 2.17 through 2.34 [V]. The format is still moving.
- **Known blockers** [R], from the stabilisation milestone and a 2026 survey: realisation trust and conflicting realisations (#4248), no CLI access to a realisation without the `.drv` (#6769), the scheduler over-fetching inputs to resolve a CA derivation before it can query the cache (#11928), IFD incompatibility (#5805) and recursive-Nix incompatibility (#4353). Hydra issue #838 is still open: CA is enabled on staging but not on prod, and "deep resolutions" code isn't fixed [V].
- **Early cutoff** works by design. If a CA output is byte-identical, the downstream derivation *resolves* to the same resolved derivation, and its build-trace entry hits, so it isn't rebuilt. Since 2.35, resolution walks underlying entries and is slower [V].
- **Substituters.** Caches need build-trace support. In 2.35 the format changed, so a cache populated by 2.34 and one populated by 2.35 don't share entries [V]. Cachix is "learning about realisations" and nixbuild has CA but no dynamic derivations [R].
- **macOS.** Nothing specific was found. The working group notes that Sandstone, the Haskell dynamic-derivation builder, has an open Darwin failure [R].
- **Verdict for Shikumi.** Shikumi can't depend on CA derivations: they don't exist in Lix, and their wire and cache formats changed in the latest upstream release.

## 2. Dynamic derivations and recursive Nix

- **CppNix.** Experimental. Primitives are claimed complete. There's an active working group (Discourse, 2026-08) and an RFC draft to stabilise the "RPC and derivation formats". Open items: start building before the whole plan exists, remove placeholders, fix the signature format, Hydra [R]. Another quote from the same notes: "not remotely ready for prime time in nixpkgs" [R].
- **Determinate.** The flag is present [V]. Determinate's own per-compilation-unit build, nix-make, deliberately uses **neither** dynamic derivations nor IFD. It uses `builtins.wasm` to scan `#include` at eval time [V].
- **Lix.** Removed, with this rationale: they're built on the CA framework, the Lix team calls them "functionally useless in their current state", and the working group notes they must keep feature detection for Lix [V]. On recursive Nix, the RFD said it puts the legacy daemon protocol into the stable derivation ABI, so it "will never be stabilized" [R].
- **Verdict.** Out for a three-implementation system. Shikumi's own core plays the role dynamic derivations would: it discovers the graph and emits derivations at build time.

## 3. Import-from-derivation

- **Behaviour.** When evaluation forces a value whose string context needs a build (`import`, `readFile`, `pathExists` on a derivation output), the evaluator stops, realises that context through the store and then continues [V for the concept, U for the exact batching]. One forced site realises its whole context in one call, but separate IFD sites run one after another on the single eval thread. With N independent IFDs, total time is the sum of their latencies.
- **Setting.** `allow-import-from-derivation`. My understanding is that it defaults to true in CppNix and Lix, while Hydra and nixpkgs CI disable it [U: check the defaults per implementation]. `nix flake check` and `nix flake show` with IFD need builds at eval time.
- **Determinate.** `builtins.parallel` plus `eval-cores` lets several IFD builds overlap (50 s to 10 s on the test flake) [V]. That's Determinate-only.
- **Cost** is a full build round trip: daemon connection, a sandbox per IFD derivation, then re-reading the output. crate2nix's own docs list "May reduce parallelism" as the cost of IFD mode [V].
- **Verdict.** Shikumi's design should use no IFD. Scanning dependencies (headers, `use`/`mod`, classpaths) belongs in the Rust core, outside the evaluator.

## 4. Programmatic APIs: can a Rust core talk to the daemon directly?

**C API.** CppNix's `nix_api_store.h` covers open, parse path, is-valid, `nix_derivation_from_json`, `nix_add_derivation`, `nix_store_realise` (blocking, one path, with a callback per output), copy closure or path, and fs closure. There's no add-path-from-NAR, no GC roots, and no batch build [V]. devenv 2 uses it through `cachix/nix-bindings-rust` branch `2.35` "with patches against Nix to extend the C FFI" [V]. It's in-process, links C++ and needs Boehm GC thread registration on every thread [V]. **Lix has none** [V]. So it's a non-starter for portability, and it also pins Shikumi's binary to one Nix ABI.

**Worker protocol** is the portable option today.
- The ops Shikumi needs exist in the protocol enum [V, snix `worker_protocol.rs`]: `AddToStore`(7), `AddToStoreNar`(39), `AddMultipleToStore`(44), `QueryValidPaths`(31), `QueryPathInfo`(26), `QueryMissing`(40), `BuildPaths`(9), `BuildPathsWithResults`(46), `BuildDerivation`(36), `AddTempRoot`(11), `AddIndirectRoot`(12), `AddPermRoot`(47), `NarFromPath`(38), `QuerySubstitutablePathInfos`(30).
- `BuildDerivation` runs a `BasicDerivation` without it being in the store, but only for trusted users. For untrusted users the route is to add the `.drv` with `AddToStore` (text CA, references = input drvs + input srcs) and then call `BuildPathsWithResults` [U: confirm the trust rules per implementation]. Lix 2.96 tightened trust checks on `repair` [V].
- Rust implementations:
  - **snix `nix-compat`** (features `wire`, `daemon`): wire framing, the op enum, handshake, and derivation ATerm parsing and path computation. It's mostly *server-side* and targets protocol 1.37 [V for wire and daemon, U for its derivation-hashing API].
  - **psyclyx/fix**: a Rust client that requires protocol 1.26 or later, with "CI exercises both CppNix and Lix daemons". It supports `unix://`, `tcp://` and `ssh-ng://`, and explicitly not `lix-xp-1` [V, from its store-compatibility doc].
  - **harmonia**: a binary cache server. It started on C++ libnixstore bindings [V]. I didn't verify whether it now has a pure-Rust daemon client [U].
  - The crates.io `nix-daemon` crate: I didn't verify its status [U].
- **Portability verdict.** Yes, today. A Rust client speaking worker protocol 1.35 or later can add sources, write `.drv`s, request builds and register roots on CppNix, Determinate and Lix, on Linux and macOS, over a unix socket or `ssh-ng`. The risk is that Lix will drop this protocol for capnp RPC on a timescale it hasn't stated. The mitigation: put the store client behind one `StoreTransport` trait in the core, with worker-protocol and Lix-RPC implementations, and keep a CLI implementation (`nix-store --realise` of many drvs in one invocation) as the always-works baseline and conformance oracle.
- **Concurrency note** [U]. The daemon forks a worker process per connection, and `max-jobs` applies per worker. So N connections can run N × `max-jobs` builds, bounded by the build-user pool (`nix.nrBuildUsers`, typically 32 on NixOS) unless `auto-allocate-uids` is on. Shikumi should own global concurrency: one long-lived connection pool and one build-slot semaphore. It shouldn't rely on daemon-side limits.

## 5. Measured per-derivation overhead

All numbers are wall clock unless noted. Hardware is mostly unstated.

| Measurement | Number | Conditions | Source |
|---|---|---|---|
| Trivial drvs, Determinate | 10,000 in 164.5 s, then 127.9 s (about 16.5 to 12.8 ms/drv) | `use-cgroups` on. Parallelism unstated | Determinate 3.24.2 notes [V] |
| Small `runCommand`, Lix | 3,000 in 80 s, then 24 s (26.7 to 8 ms/drv) | Synthetic. "Most optimistic case". Linux, Lix 2.95 | Lix gerrit cl/5073 [V] |
| `mkDerivation` vs `derivation` | 10k: 475 s vs 318 s (47.5 vs 31.8 ms/drv) | 2022 Nix. CPU about 10%, so scheduler-bound, not CPU-bound | Discourse 24298 [R] |
| Sandbox cost, 100 trivial drvs `-j8` | 1.75 s without sandbox, 7.6 s with sandbox, 3.7 s without network namespace | Linux 4.9, 2017. The network namespace dominated, with occasional 200 ms `unshare -n` outliers | NixOS/nix#179 [V] |
| Same test, 2017 | 1.8 s without sandbox, 5.0 s with (about 30 ms extra per drv at -j8) | Kernel 4.10 | NixOS/nix#179 [V] |
| Sandboxing share | "sandboxing costs 30–40%" | Dynamic-derivation builds, a few versions ago | Working group notes [R] |
| Worker at scale | Event loop blocks on sandbox setup and teardown. `fork()` slows as the worker grows. `killUser()` took 150 ms | 70k-derivation builds | NixOS/nix PR #15611 [V] |
| nix-ninja building Nix | ninja -j16: 6m12s. nix-ninja (`nix build -j16`): 371.9 s | Dynamic-derivation store. 290 ninja edges | nix-ninja #7 [V] |
| nix-ninja drv generation | 2m12s real, 7m14s user, 2m56s sys | Nix 2.27.1. Generating derivations alone | nix-ninja #4 [V] |
| Shinobi, LLVM `llc` cold | 229.0 s vs ninja 207.0 s (+22 s, about 12 ms wall per unit over 1,773 units) | One drv per source file | WG notes [R] |
| Shinobi, 21.1.7 to 21.1.8 incremental | 15.0 s vs ninja 9.8 s, for 93 to 102 units | Fixed overhead dominates small increments | WG notes [R] |
| Shinobi, patch bumps | 88.1 vs 91.0 s, 89.7 vs 84.5 s | About 550 units | WG notes [R] |
| Eval through daemon | `nix flake show` (3 NixOS configs): 12.8 s, then 8.5 s after the async path writer batched validity queries | Each `.drv`/`toFile` write used to be "a synchronous round trip to the Nix daemon". One eval wrote the same file 5,279 times | Determinate 3.23 [V] |
| Parallel instantiation | `nix eval nixpkgs#firefox`: 1.51 s, then 0.77 s | Determinate 3.24 | [V] |
| nix-make cold eval | About 1.2 s for the whole `nix` derivation graph (per-TU drvs) | Determinate, Linux only | [V] |
| `builtins.wasm` call | About 123,000 calls/s vs 2.8M native Nix calls/s | i7-1260P | Determinate blog [V] |
| ATerm parse | 14 µs (8 deps) to 225 µs (150 deps) | Third-party fork's doc | straylight-software/nix PERFORMANCE.md [R] |
| `nix build --rebuild` hello | About 334 ms | nix-hyperfine README. Includes CLI startup | [R] |

I found no measured numbers for:
- **macOS per-derivation overhead.** Bazel's macOS investigation found `sandbox-exec` itself cheap and symlink trees and per-action caches expensive [R]. Nix on Darwin doesn't build symlink trees. It runs as a build user under a seatbelt profile, so I'd expect the cost to be process spawn, profile compilation and SQLite registration [U]. Measure it.
- **Raw daemon round trip.** I'd estimate a unix socket round trip in tens of µs, with path registration dominated by SQLite plus fsync [U]. Measure it.
- **Substitution queries for many small paths.** See section 9.

**Reading the numbers.** On current Linux releases, fixed cost per unit is about 8 to 15 ms of wall time at moderate parallelism, and it's mostly daemon-side management, not the kernel. At 16 jobs, 2,000 units cost about 2,000 × 12 ms / 16 ≈ 1.5 s of extra wall time on a cold build. That's tolerable against minutes of compiling. The real risks are elsewhere:
1. **Fixed latency on small increments.** Shinobi took 5 s extra on about 100 units, which says per-invocation setup and the serial critical path matter more than per-unit cost.
2. **The worker's event loop** blocking on sandbox setup at tens of thousands of drvs (PR #15611).
3. **Drv generation through the evaluator.** nix-ninja spent 2m12s generating about 290 derivations, which is pathological and evidence of a bad generation path, not of an inherent cost.

## 6. Evaluation speed and incrementality

- **Flake eval cache.** SQLite per flake fingerprint, which covers the lock and the source tree hash. It caches attribute *values* along a path, not derivation-level dependencies [V for SQLite, U for the exact key]. Editing any tracked file changes the fingerprint and throws away the whole cache. Dirty git trees aren't cached at all in CppNix [U: confirm in 2.35, where lazier source copying landed]. 2.35 hashes sources without copying them to the store, but it still touches and hashes the whole tree on every eval [V]. Determinate fixed the cache only paying off on the third run [V]. Verdict: useless for a single edited file among thousands.
- **devenv 2 eval cache** (`devenv-eval-cache/DESIGN.md`) [V]:
  - Key: `blake3(serialize(NixArgs) + ":" + attr_name)`.
  - Inputs: files and env vars observed through Nix log events and primop hooks, which needs devenv's patched Nix.
  - Validation: content hash plus mtime. Storage: SQLite.
  - Deliberately over-inclusive: "a cache row for attr A will contain every file the session has seen so far", because module merging runs once per session.
  - Good for a small number of big attributes. For per-unit derivations it degenerates to "any source change invalidates everything", the same failure as the flake cache. It also needs a patched C API, so it's CppNix-only.
- **nix-eval-jobs.** A process pool that evaluates many attributes in parallel workers, with a memory cap per worker that triggers a restart. It streams JSON lines with `drvPath`. Lix ships its own copy, which gained `--apply` and `--select` in 2.96 [V]. It helps evaluate many top-level attributes, not one edited file.
- **Recommended shape: generate unit derivations outside the evaluator.**
  1. Nix evaluates a small **toolchain context** once per change to `flake.lock` or the Shikumi Nix files. The context is compiler and linker store paths, the builder program, base env and system. Shikumi caches it keyed on the hash of those files, so it's a fixed, small evaluation (about 0.5 to 1.5 s with nixpkgs) [U for the timing].
  2. The Rust core turns `(context, unit action, input CA paths)` into an ATerm `.drv`. The core computes the drv path, through `hashDerivationModulo` and the output path, and writes only drvs whose path isn't already valid, in batches with `AddMultipleToStore` or with concurrent `AddToStore` calls. Each drv costs µs of CPU plus a store write. A one-file edit creates O(affected units) drvs.
  3. Sources go in as CA store paths (per file or per small group, Shikumi decides), keyed by content hash, so unchanged files are never re-added. The core checks first with `QueryValidPaths`, one call for many paths.
  4. Correctness hinges on drv path computation matching Nix bit for bit. The plain input-addressed ATerm format is identical across all three implementations [U: true in practice, since Lix kept it, but write a conformance test]. Lock it with a test that compares the core's computed paths against `nix derivation add` or `nix-instantiate` output on each implementation in CI. snix `nix-compat` has derivation ATerm and path code to reuse or cross-check [U: API].
  - This is what nix-ninja recommends ("generating JSON derivation format directly to avoid the overhead of Nix evaluation") [V], and it matches how Bazel and Buck keep the action graph out of Starlark evaluation.
  - **Cost model:** O(changed units) per edit, plus one `QueryValidPaths` round trip per batch. The evaluator is never in the per-edit loop.

## Early cutoff without CA derivations

This applies on all three implementations, and it replaces the CA plus dynamic derivations path.

1. Every unit derivation is input-addressed. Its `inputSrcs` are **CA store paths only**: sources, and re-addressed outputs of upstream units. Its `inputDrvs` hold only the toolchain context derivations, which are stable.
2. After unit A builds, the core gets A's `narHash` from `QueryPathInfo`. That's free, because the daemon computes it at registration. The core then needs a content-addressed path for the same bytes. Two options:
   - (a) Stream `NarFromPath(A)` into `AddToStoreNar` with `ca = fixed:r:sha256:<narHash>` and A's references. That's one extra copy of a small artifact. Or:
   - (b) Use `AddToStore` with recursive and NAR content-addressing, if the daemon accepts streaming.
   - The CA path depends only on content, name and references [U: confirm that CA paths with references to *other* paths are accepted for the NAR/sha256 "source" type on all three. Self-references are not allowed].
3. Downstream unit B names the CA path. If A's bytes are unchanged, B's `.drv` is byte-identical, so its output path is already valid and nothing rebuilds. That's early cutoff.
4. Requirement: compile outputs must be reproducible and must not embed the input-addressed path of the unit that produced them. Use `-ffile-prefix-map`, `--remap-path-prefix` and deterministic jar ordering. Inputs are CA already, so embedded source paths are stable.
5. Cost: one extra store write per unit output. Also, B's drv can only be written after A finishes. The core schedules the graph dynamically, as nix-ninja's local mode and Bazel do, so build requests go in waves along the critical path, not as one `nix build`. Run several daemon connections at once so independent waves overlap.

On CppNix and Determinate the same graph *could* use real CA derivations later. Don't build a second path for it now: the re-addressing component owns early cutoff, and swapping its implementation is a future decision once milestone #35 closes.

## 7. GC roots and eviction

- **Mechanisms.** `nix-store --add-root <link> --indirect -r <drv>` and `nix build --out-link` both create indirect roots, recorded as `/nix/var/nix/gcroots/auto/<hash>` → `<link>`. Over the protocol it's a symlink plus `AddIndirectRoot`(12), or `AddPermRoot`(47) where supported. During a session, `AddTempRoot`(11) protects paths that aren't rooted yet. Temp roots last for the connection's lifetime [V for the ops, U for `AddPermRoot` support in Lix].
- **Scalable design.** Don't create thousands of symlinks per worktree. Per build, write one **manifest store path**: a tiny text CA path whose references are every live unit output and drv. Root only that, at `<worktree>/.shikumi/roots/current`, plus N previous generations. A deleted worktree leaves a dangling auto root, and the next GC collects its closure. Cost: O(units) references in one path. GC root scanning is O(worktrees × generations).
- **Eviction.** Nix GC is global reachability with no LRU or access times, so Shikumi must own the policy:
  - Keep the last K manifests per worktree, plus a shared "warm" manifest of recently hit units across worktrees, aged out by Shikumi's own last-used timestamps.
  - Then let `nix-store --gc --max-freed`, or the daemon's `min-free`/`max-free` auto-GC, reclaim the rest.
  - GC speed matters with millions of small paths. Determinate 3.24 made GC and registration up to 32% faster on large stores, and 3.23 fixed a GC race with paths being created [V]. 3.24.2 fixed a readdir error that could make GC miss roots [V].
- **Risk.** One store path per unit stage means stores with millions of tiny entries. GC time grows with store size and SQLite size. Measure GC on a store of about 1M small paths before committing.

## 8. Locking third-party artifacts

- **`flake.lock`** is a JSON graph of nodes, one per flake input, with `locked` fields such as `type`, `narHash`, `rev` and `lastModified`. It's rewritten whole, it's meant for tens of inputs, and each input is fetched as a whole tree at eval time [V for the format, R for the scale intent]. Thousands of entries would bloat every eval (lock parsing, `fetchTree` per used input) and cause merge conflicts across teams.
- **Prior art:**
  - **crate2nix** takes hashes from `Cargo.lock` checksums first: the crates.io `checksum` is the sha256 of the `.crate`, directly usable as a flat fetchurl hash. Then `crate-hashes.json` for git dependencies, then prefetch [V]. It has a known merge-conflict problem with the single `crate-hashes.json` (issue #82) [V]. Manual mode emits `Cargo.nix` (no IFD). Auto mode uses IFD [V].
  - **gradle2nix v2** writes its own `gradle.lock`, a map of coordinates → filename → `{url, sha256}`, readable from Nix [V].
  - **cargo2nix** generates a `Cargo.nix` with hashes inline [U].
- **Recommendation.** `flake.lock` holds only the toolchain (nixpkgs, nix-unit). Third-party artifacts go in **Shikumi's own lock**:
  - Content-addressed, one line per artifact, sorted, sharded by ecosystem and package so merges stay local.
  - Reuse native lock hashes where they exist (`Cargo.lock` checksums). Add Maven sha256s where the ecosystem only gives sha1 or nothing.
  - Each artifact becomes a fixed-output derivation, written by the core like any other drv, with `builder = "builtin:fetchurl"` and a flat sha256 hash. The output path depends only on name and hash, so it's stable and shareable [V for builtin:fetchurl existing, U for whether it skips sandbox cost].
  - The lock is searchable with grep, and its size grows linearly with dependencies.

## 9. Remote builders and binary caches for many small paths

- **Binary cache lookups** are one `GET <hash>.narinfo` per path. Nix runs them concurrently (`http-connections`, default 25 [U]) over HTTP/2 [U], and 2.35 adds optional HTTP/3 [V]. Results go in a local SQLite narinfo cache with positive and negative TTLs [U: defaults believed to be 30 days and 1 hour]. Round trip estimate: 2,000 units × about 20 ms / 25 ≈ 1.6 s of pure miss checking on a cold cache, plus one NAR GET per hit. Most lookups for new units are 404s, which become negative-cache entries.
- **Recommendation.** Keep a Shikumi-level action index so the core asks the cache only for paths it expects to exist. Ideally one fetched manifest per commit that lists unit outputs, so a fresh worktree does one request and then targeted NAR fetches. Upload with batched multi-path pushes (attic, cachix daemon, or `nix copy` of the manifest closure in one invocation). A self-hosted harmonia or attic near the builders cuts round trip time.
- **Remote builders via the build hook** spawn one `build-remote` hook per derivation. Each one negotiates slots, does `QueryValidPaths` and copies the input closure [V for the architecture, U for cost: likely tens to hundreds of ms per drv]. That's poor for thousands of small units.
  - Better: send whole waves to a remote daemon directly, with `--store ssh-ng://host` or the core holding `ssh-ng` worker-protocol connections to several builders. Copy CA inputs once with `AddMultipleToStore`. `ssh-ng` is the protocol all three keep [V]. Lix 2.96 just reached `ssh-ng` parity, but only Lix-to-Lix gets the full feature set [V].
  - Determinate has OpenTelemetry tracing for builds and substitutions, which helps profile this [V].

## Risks, ranked

1. **Lix protocol churn.** Its legacy worker protocol is slated for removal after capnp RPC stabilises, and the RPC schema has "NO stability guarantees" [V]. Mitigation: a single store-transport trait with a CLI fallback, and a CI matrix against all three implementations.
2. **Divergence across the three implementations** is permanent. CA, dynamic and recursive builds are gone in Lix. Parallel eval and `builtins.wasm` are Determinate-only. The C API is CppNix and Determinate only, with version skew. Use only the 2.18-level common subset: input-addressed drvs, fixed-output drvs, CA *paths*, and the worker protocol.
3. **Store scale.** Millions of tiny store paths stress GC, SQLite registration and `fsync`, and the worker event loop (PR #15611). It's unmeasured at Shikumi's target scale.
4. **Small-increment latency.** Fixed per-invocation and critical-path costs (Shinobi: +5 s on about 100 units) may dominate the edit-compile loop that users feel most.
5. **Reproducibility of compiler outputs.** Early cutoff only pays if outputs are bit-identical. Embedded paths, timestamps and nondeterministic codegen ordering silently defeat it.
6. **macOS** has no published per-derivation overhead numbers. Darwin builds share one global seatbelt mechanism, and there's no cgroup equivalent.
7. **Build-user pool and `max-jobs` semantics** (per connection) can either cap or oversubscribe parallelism unless Shikumi owns the slot accounting [U].
8. **Drv-path computation drift.** If any implementation changes ATerm or hashing for input-addressed drvs, every path computed by the core is wrong. Guard with conformance tests against each implementation.

## The smallest spike that settles whether per-unit derivations pay

**Goal:** measure per-unit overhead and incremental latency, compared with ninja, on all three implementations, without building any of Shikumi.

**Fixture:** a generated C project. 2,000 `.c` files, each including 5 of 200 headers, sized so each TU compiles in about 150 ms at `-O2`, linked into one binary. Plus a second variant where every TU is a `true` builder, to isolate pure overhead.

**Driver:** a throwaway Rust binary of about 500 lines in `.runtime/`.
1. Evaluate a toolchain context once (gcc, coreutils, a tiny builder). Add sources as CA paths.
2. Write per-TU ATerm drvs whose inputs are CA paths. Request builds. Re-address outputs as CA paths. Write and build the link drv.
3. Do it twice:
   - **Transport A:** the worker protocol over the unix socket, through nix-compat framing or a hand-rolled client.
   - **Transport B:** one `nix-store --realise <all drvs>` CLI call per wave.

**Matrix:** CppNix 2.35.4, Determinate 3.24.2 (cgroups on and off), Lix 2.96.0. Linux (max-jobs = cores) and one Apple Silicon Mac. Sandbox on.

**Measurements**, each run in isolation on a quiet machine:
1. Cold build, wall and CPU, compared with `ninja -jN` on the same fixture.
2. `true`-builder variant: wall time ÷ (N / jobs) gives per-unit overhead in ms.
3. No-op rebuild with everything valid: should be dominated by one batched `QueryValidPaths`.
4. Edit one `.c` file: end-to-end latency, from drv generation to linked binary.
5. Edit one header included by 10% of TUs.
6. Edit a comment only, with output reproducible: early cutoff should rebuild one TU and skip the link.
7. Fresh store substituting all 2,000 outputs from a local harmonia or `file://` cache: query time and fetch time.
8. GC of 10 rooted manifests on top of 20k unit paths.

**Pass criteria** (proposed):
- Per-unit overhead of 15 ms or less on Linux, 30 ms or less on macOS.
- Cold build within 15% of ninja.
- One-file edit loop of 1 s or less on top of the compile time.
- No-op of 300 ms or less.
- Early cutoff measurably skips downstream work.
- Results agree between transports A and B.

If the `true`-builder overhead exceeds about 25 ms/unit on any primary target, units must be coarser (per-module or per-crate batching) or need a cheaper sandbox (`external-builders` on CppNix). If transport A isn't clearly faster than B, keep the CLI transport and drop the protocol client.

## Sources (primary unless noted)

- NixOS/nix tags and rl-2.35: https://github.com/NixOS/nix/tags, https://github.com/NixOS/nix/blob/master/doc/manual/source/release-notes/rl-2.35.md, https://manual.determinate.systems/release-notes/rl-2.35
- CA milestone #35: https://github.com/NixOS/nix/milestone/35. Hydra CA: https://github.com/NixOS/hydra/issues/838
- C API header: https://github.com/NixOS/nix/blob/master/src/libstore-c/nix_api_store.h
- Worker PR #15611: https://github.com/NixOS/nix/pull/15611. Sandbox issue #179: https://github.com/NixOS/nix/issues/179. `builtin:write-file` #14011. Multithreaded eval #10938
- Determinate 3.24.2: https://manual.determinate.systems/release-notes-determinate/v3.24.2. Blogs: changelog-determinate-nix-3-24-1, changelog-determinate-nix-3-23-3, builtins-wasm, changelog-determinate-nix-3111. Wasm docs: https://docs.determinate.systems/determinate-nix/wasm/. nix-make scanner: https://github.com/DeterminateSystems/nix-wasm-rust/pull/9
- Lix: rl-2.94 (removals), rl-2.95, rl-2.96, blog 2.95 and 2.96, RFD #815 https://git.lix.systems/lix-project/lix/issues/815, gerrit cl/3118 and cl/5073, daemon.capnp in cl/5533, core team notes 2026-02-11
- Dynamic derivations WG notes: https://discourse.nixos.org/t/dynamic-derivation-working-group-meeting-notes-1/79575. Next steps: https://discourse.nixos.org/t/dynamic-derivations-dynamic-build-graphs-next-steps/79409
- nix-ninja: https://github.com/pdtpartners/nix-ninja, issues #4 and #7, docs/dynamic-derivations.md
- nix-bindings-rust: https://github.com/nixops4/nix-bindings-rust, issue #50. devenv 2: https://devenv.sh/blog/2026/03/05/devenv-20-a-fresh-interface-to-nix/, devenv-eval-cache/DESIGN.md, devenv-nix-backend
- snix nix-compat: https://snix.dev/rustdoc/nix_compat/. psyclyx/fix store-compatibility.md. harmonia: https://github.com/nix-community/harmonia
- crate2nix (tools.nix, issue #82, README). gradle2nix V2 PR #62
- Secondary: garnix incremental builds blog (2024). Discourse 24298 benchmark (2022). wyattgill9 "Advanced Nix Breakdown" gist [R]. straylight-software PERFORMANCE.md [R]
