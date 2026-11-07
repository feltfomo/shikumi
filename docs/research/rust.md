# Shikumi research: Rust cut points

Scope: rustc and the Cargo ecosystem. Research only, nothing was built or run.

## Sources read

All of these were shallow clones made on 2026-11-07. Line references point into those trees.

| Repo | Commit | Notes |
|---|---|---|
| rust-lang/rust | `e30d778f3029b78bb9c2e0dbe11a3b7389b652c4` (2026-11-07, `src/version` = 1.102.0, i.e. 1.102 nightly) | sparse: rustc_monomorphize, rustc_metadata, rustc_session, rustc_codegen_ssa, rustc_codegen_llvm/back, rustc_interface, rustc_middle/hir, rustc_hir, rustc_driver_impl, src/doc/rustc, unstable-book |
| rust-lang/cargo | `c07ac2c` (2026-11-06) | `src/compiler/*` |
| facebook/buck2 | `5d36df5` (2026-11-07) | `prelude/rust` |
| bazelbuild/rules_rust | `d08ddac` (2026-11-06) | |
| facebookincubator/reindeer | `864a276` | `docs/MANUAL.md` |
| ipetkov/crane | `79fd635` (2026-11-04) | docs, FAQ |
| nix-community/crate2nix | `1cb6033` (2026-09-28) | docs |
| cargo2nix/cargo2nix | `a709c74` (2025-06-19, last commit) | `overlay/mkcrate.nix` |
| nextest-rs/nextest | `238d60e` (cargo-nextest 0.9.151) | `site/src/docs` |
| mozilla/sccache | `c1d7204` | `docs/Rust.md` |

Web pages read: the rust-analyzer book's non-Cargo projects page, crane discussion #213, rust PR #139453, crate2nix #347 and PR #443 (via search excerpt), nixpkgs `build-rust-crate/default.nix` (master, via search excerpt), nixpkgs #89563, Mic92/cargo-nix-plugin README, and the NCI and crane changelogs.

Tags: **[V]** means verified in source or docs at the versions above. **[I]** means inferred and not verified. **[U]** means unverified and needs a spike.

Short paths below: `R/` = rust-lang/rust, `C/` = cargo, `B/` = buck2 `prelude/rust`, `RR/` = rules_rust.

---

## 1. Candidate cut points

| # | Boundary (unit to output) | (1) Hermetic: declared inputs | (2) Granularity | (3) Deterministic / what breaks it | (4) Worth a derivation? |
|---|---|---|---|---|---|
| A | **Dependency resolution** (Cargo.lock + manifests to unit graph: features, cfg, targets) | Cargo.lock, every Cargo.toml, registry index entries, target cfg set | one per workspace × target platform | Deterministic if computed by a resolver given the same inputs. It's an eval-time product, not a build product. | Not a derivation. It's eval or generation work, and it's the eval-cost hotspot (§3). |
| B | **Build script compile** (`build.rs` to host exe) | build.rs sources, `[build-dependencies]` rlibs (host), host rustc, `-C metadata` | one per package with a build script | Same as an ordinary bin. Paths need remapping. **[I]** | Usually small, but it's a separate edge, and Cargo/Bazel/Buck all model it separately. Yes. |
| C | **Build script run** (exe to `OUT_DIR` + `cargo::` directives) | the exe; env (`CARGO_PKG_*`, `CARGO_FEATURE_*`, `CARGO_CFG_*`, `TARGET`, `HOST`, `OUT_DIR`, `RUSTC`, `DEP_<links>_*` from deps with `links`); package source; native tools/libs (pkg-config, cc) | one per package × feature set × target | **Arbitrary code.** Embeds paths and sometimes timestamps; may probe the network or the host. Outputs (rustc-cfg, rustc-env, link-lib/search, OUT_DIR files) can be byte-stable if the script is tame. | Yes. This is the main hermeticity boundary, and it's already a separate action in Buck2/rules_rust/reindeer. |
| D | **Proc-macro crate** (crate to host dylib) | sources, host deps' rmeta/rlib, host rustc | one per proc-macro crate × *host* config | Output is a host `.so`. Deterministic if paths are remapped. **[I]** Buck2 builds it "host-agnostic" by keeping target config out of `-Cmetadata` **[V]**. | Yes. It's on the critical path of every user. |
| E | **Crate frontend to full metadata** (sources + upstream rmeta to `.rmeta` that carries the MIR codegen needs) | sources (dep-info lists them), upstream **full** rmeta, `--cfg feature=...`, rustc flags in SVH (`opt-level`, `debuginfo`, the emit *kinds*, edition, target...), build-script outputs, env read via `env!` | one per crate × profile × target × feature set | Not stable across *any* source edit, even one to a private body (§2.2). Stable across reruns given identical inputs and remapped paths **[I]**. | Yes, for pipelining. The critical path drops from "full build of every dep" to "metadata of every dep". Costs a second frontend run (§2.1). |
| E′ | **Check metadata** (`--emit=metadata`, no codegen MIR) | as E | as E | Different SVH from E's because the emit kinds are in the SVH. Optimized MIR for generics/inline items is missing. | Only for `check`/clippy/IDE graphs. **It can't feed codegen** (Buck2 calls it "metadata-fast"). |
| F | **Crate codegen to rlib** (sources + upstream full rmeta to `.rlib` = per-CGU `.o` + metadata [+ embedded bitcode]) | as E | one per crate × config (CGUs run in-process) | Deterministic without `-Cincremental` given remapped paths and stable `-C metadata` **[I]**. Under `-Cincremental`, temp names get a random per-invocation suffix and rlibs aren't byte-reproducible **[V source, rlib effect per Buck2 comment]**. | Yes. This is the standard granularity (crate2nix, Bazel, Buck2). |
| G | **Per-CGU codegen across processes** (LLVM module to `.o`) | per-CGU bitcode plus the exact TargetMachine and pass config | one per CGU | **No supported way.** CGU membership in non-incremental mode is size-sorted and renamed `-cgu.N` on every build (§2.3). rustc's in-process pipeline (pre-LTO opt, then local ThinLTO, then codegen) isn't reproducible from outside without the same LLVM fork and options **[U]**. | No (spike G). |
| H | **Linker-plugin / distributed ThinLTO backends** (`-Clinker-plugin-lto` objects are bitcode; a thin-link step builds an index; per-module backend jobs; final link) | bitcode `.o`s from F (bitcode mode), the thin-link index, the LLVM matching rustc's | one backend job per module (CGU) in the final link | LLVM ThinLTO backends are designed to run distributed (clang's `-fthinlto-index`). rustc has `--emit=thin-link-bitcode`, but only behind `-Zunstable-options` **[V]**. Whether rustc bitcode works with an external backend (LLVM version, rustc-specific passes) is **[U]**. | Only for release/LTO builds of big binaries. That's where codegen dominates. Spike H. |
| I | **Final link** (rlibs + native libs to bin/cdylib/test exe) | all transitive rlibs (objects), native libs and link args from build scripts, linker, sysroot std rlibs | one per binary/test target | Linker determinism is usually fine with lld/mold. Apple OSO/PDB paths aren't remapped by rustc **[V docs]**. | Yes. Buck2 even splits "rustc emits objects" from "cxx links" via a fake `-Clinker` (§2.6). |
| J | **Test binary listing** (test exe to list of test names) | the test exe, its runtime env | one per test binary | `--list --format terse` is deterministic for libtest **[I]** | Cheap. A derivation is fine; it feeds J′'s fan-out. |
| J′ | **Test execution** (exe + test name to result) | exe, test data, env, *declared* runtime deps | one per test (nextest) or per partition | Only as deterministic as the tests. Store the result, not stdout timing. | Process-per-test is right for running. Derivation-per-test is too fine for small tests, so batch by partition (§2.8). |
| K | **Unit-test crate build** (`--test` rebuild of the lib crate) | as F plus dev-deps | one per crate with unit tests | as F | Required. It's a *second full compile* of the crate, not reuse of F (§2.7). |

Cut points stack like this: A → (B → C) → D/E → F → (H) → I → J → J′. E and F are separate rustc processes, and E′ is a parallel graph used only by check builds.

---

## 2. Open questions, answered

### 2.1 `--emit` kinds, `--json=artifacts`, and pipelining

- The emit kinds at this commit are `asm, llvm-bc, dep-info, link, llvm-ir, metadata, mir, obj, thin-link-bitcode` **[V]** (`R/compiler/rustc_session/src/config.rs` `define_output_types!`, lines ~778-855). `thin-link-bitcode` is fatal without `-Zunstable-options` (`config.rs` ~2495) **[V]**.
- Only `dep-info`, `link` and `metadata` are "compatible with codegen units and single output file" **[V]**. If you request `asm/llvm-bc/llvm-ir/mir/obj` **without** an explicit `-C codegen-units`, rustc forces `codegen-units=1` *and* turns local ThinLTO off (`should_override_cgus_and_disable_thinlto`, config.rs ~2519) **[V]**. So `--emit=llvm-bc` silently changes codegen. It doesn't observe it.
- `should_codegen()` is true for every kind except `metadata` and `dep-info` (config.rs ~931) **[V]**.
- `--json=artifacts` sends one JSON message as each artifact lands on disk, with `emit` set to one of `link, dep-info, metadata, asm, llvm-ir, llvm-bc, mir, obj`. The docs note there can be several per type when there are several CGUs **[V]** (`R/src/doc/rustc/src/json.md` §Artifact notifications).
- Cargo runs `--emit=dep-info,metadata,link` for rlib-type units, and `--emit=dep-info,link` for units that need upstream objects (bins, tests, dylibs). Check mode uses `--emit=dep-info,metadata`. On nightly it passes `-Zembed-metadata=no` and emits metadata separately **[V]** (`C/src/compiler/mod.rs` ~1388-1420). Cargo passes `--json=diagnostic-rendered-ansi,artifacts,future-incompat,unused-externs-silent` (~1321) and unblocks dependents when an artifact path ends in `.rmeta` (`rmeta_produced`, ~2410) **[V]**. Pipelining here is *within one process*: rustc keeps running codegen after it writes the rmeta.
- **Nix consequence.** A derivation's output only exists once the builder exits, so in-process pipelining can't be expressed. You need two derivations per crate:
  1. **Metadata derivation.** Buck2 uses `-Zno-codegen --emit=link` and produces a "hollow rlib" with full metadata and no objects. The comment explains why: "this flag is the only way that the Emit("metadata") and Emit("link") operations are allowed to diverge without causing them to get different crate hashes" **[V]** (`B/build.bzl` ~1833-1850). `-Zno-codegen` is `[TRACKED_NO_CRATE_HASH]` (`R/.../options.rs` 2750) **[V]**. This needs nightly or `RUSTC_BOOTSTRAP`.
  2. **Stable alternative.** rules_rust runs the *same* command line as the full action and has `process_wrapper --rustc-quit-on-rmeta` kill rustc when the rmeta artifact notification arrives **[V]** (`RR/rust/private/rustc.bzl` ~1310). The rmeta is byte-identical to the full action's only if both rustc runs are deterministic **[I]**. This works on stable.
  3. The codegen derivation re-runs the frontend. Pipelining in Nix therefore costs ~1 extra frontend pass per crate in CPU, in exchange for a shorter critical path. The full action can also run once all its deps' *metadata* derivations exist **[I]**, which is the same scheduling rule Buck2/Bazel use. It's still bounded by rustc's own check that SVHs match at link time.
- Why E′ can't feed F: with `--emit=metadata` only, `should_codegen()` is false, so `should_encode_mir` doesn't encode optimized MIR for reachable generic or cross-crate-inlinable fns (`R/compiler/rustc_metadata/src/rmeta/encoder.rs` ~1173-1203) **[V]**. The emit *kinds* are hashed into the SVH, though not their paths (`DepTrackingHash for OutputTypes`, config.rs ~3640) **[V]**, so E′ and F also disagree on SVH.

### 2.2 What `.rmeta` contains, and whether a private body edit changes its bytes

**Yes, any source edit changes the `.rmeta` bytes. Byte-level early cutoff on `.rmeta` isn't real in current rustc.** **[V]**

- The SVH (crate hash) is written at a fixed header offset in the rmeta (`encode_crate_hash`, encoder.rs ~2839) **[V]**.
- On this nightly, the SVH is an XXH3-128 of the encoded metadata bytes, plus a "supplement" that hashes `dep_tracking_hash(true)` (every `[TRACKED]` flag) **and `compute_hir_hash(tcx)`** (`with_encode_metadata_header`, encoder.rs ~2711-2730) **[V]**. The old HIR-based SVH is still behind `-Zmetadata-crate-hash=no`, with the new one as the default (options.rs 2705; `R/compiler/rustc_middle/src/hir/map.rs` `crate_hash` ~1182) **[V]**.
- `compute_hir_hash` combines each HIR owner's `fingerprint()`, which is `nodes.opt_hash` combined with the attrs hash (`R/compiler/rustc_hir/src/hir.rs` ~1409). `OwnerNodes.opt_hash` is documented as the "Pre-computed hash of the full HIR, including bodies", and `hash_owner_nodes` explicitly hashes `bodies` (`R/compiler/rustc_middle/src/hir/mod.rs` ~175-195) **[V]**. Stable hashing includes spans unless `-Zincremental-ignore-spans` (`R/compiler/rustc_middle/src/ich.rs` ~38) **[V]**. So a private body edit changes the SVH, and so does an edit that only *moves spans* (a comment or blank line above an item) **[V by construction; not run]**.
- The SVH also propagates. Every dependent's rmeta records `CrateDep { hash: crate_hash(cnum), ... }` for each upstream (encoder.rs ~2220) **[V]**, so a downstream crate's rmeta changes when any upstream SVH changes, even if the downstream source is untouched. The cascade is transitive.
- Other rmeta content that moves with private edits: spans for every encoded item (and the source-file table), and optimized MIR for generic and `#[inline]` or cross-crate-inlinable items (above). Exported symbol lists also change when non-generic functions are added or removed **[V for the encoded tables; I for which edits move which table]**.
- rustc enforces the SVH at load time. Reusing a downstream rlib built against SVH₁ with an upstream rebuilt to SVH₂ fails ("found possibly newer version of crate") **[I: the locator source wasn't read]**. So you can't "early cut off" by keeping the old downstream output, even if you write your own interface hash.
- **What does give cutoff?** Content-addressed derivations (CA) still cut off when inputs change but the crate is rebuilt byte-identically. Examples: a build-script rerun that produces the same OUT_DIR, an edit to an unrelated file in a filtered `src`, a change to a non-rustc input, or a dependency whose rmeta is identical. Cutoff granularity is "inputs that actually reach rustc". It isn't "public API". **[I]**
- A real API-level cutoff would need an *interface hash* that rustc doesn't currently produce (span-free, body-free, SVH-free), *and* a way to stop the SVH from flowing into dependents. Neither exists **[I]**. File it as a possible upstream proposal, not something Shikumi can build on.

### 2.3 Codegen-unit partitioning

From `R/compiler/rustc_monomorphize/src/partitioning.rs` and `rustc_session/src/session.rs` **[V]**:

- Default CGU count: target default if set, else **256 with `-Cincremental`**, else **16** (`Session::codegen_units`, session.rs ~1188-1253). `-C codegen-units` is `[UNTRACKED]` (options.rs 2261), so it doesn't affect the SVH.
- The initial placement uses one CGU per source module, plus a separate **volatile** CGU per module for generic instances. The volatile split happens **only in incremental builds**: `is_volatile = is_incremental_build && mono_item.is_generic_fn()` (partitioning.rs ~230).
- Merge step: while there are more CGUs than the max, the smallest ones merge into the most-overlapping one. **Non-incremental builds with the default count** also merge any CGU under `NON_INCR_MIN_CGU_SIZE = 1800` (~385-418).
- Naming: incremental CGUs keep module-path-derived names. Merged ones get the hash of the joined names (~423-465). **Non-incremental CGUs are re-sorted by size and renamed `<crate>.<hash>-cgu.N`** (~471-500). So in the default release/CI mode, CGU identity isn't stable across edits. A per-CGU derivation has no stable name and no stable contents.
- `#[inline]` items (`LocalCopy`) are copied into every CGU that uses them (~219-226 and the module doc).
- LTO defaults (`Session::lto`, session.rs ~992-1062): at opt-level > 0 with more than 1 CGU and nothing requested, rustc uses **ThinLocal**, which is ThinLTO across the crate's own CGUs. So the per-CGU objects depend on every other CGU in the crate through the ThinLTO import step. CGUs aren't independent work units in release builds.
- Per-CGU bitcode can be emitted. With an explicit `-C codegen-units=N` and no `-o` or explicit emit path, rustc leaves the per-CGU `<crate>.<cgu>.rcgu.bc` (or `.o`, `.ll`) files in place (`produce_final_output_artifacts`, `R/compiler/rustc_codegen_ssa/src/back/write.rs` ~505-590) **[V]**. `-C save-temps` keeps intermediates. The names visible in source are `no-opt` (pre-optimization, `rustc_codegen_llvm/src/back/write.rs` ~894) and `thin-lto-input`, `thin-lto-after-rename/resolve/internalize/import/pm` (`back/lto.rs` ~740-793) **[V]**.
- Can a separate process compile that bitcode to a `.o` identical to rustc's? **Unverified [U].** rustc drives LLVM through its own wrappers and forked LLVM, with target machine options from its session (target-cpu/features, reloc and code model, frame pointers, split-dwarf, sanitizers and so on). Stock `llc`/`opt` of the same LLVM version would need all of those replicated. No rustc doc promises equivalence.
- Is there a flag that splits frontend from codegen across processes?
  - `-Zno-link` / `-Zlink-only` (unstable) split **codegen from link**, not frontend from codegen. `-Zno-link` serializes `CompiledModules` (paths to already-produced `.o`/`.bc`), `CrateInfo`, `EncodedMetadata` and `OutputFilenames` into a `.rlink` file, and `-Zlink-only` links it (`R/compiler/rustc_codegen_ssa/src/lib.rs` ~110-118, ~335-380; `rustc_interface/src/queries.rs` 127; `rustc_driver_impl/src/lib.rs` 272) **[V]**. The rlink is tied to the rustc version and holds temp paths.
  - `-Zno-codegen` (unstable) stops before codegen and produces nothing for codegen to resume from **[V]**.
  - `-Clinker-plugin-lto` (stable) makes rustc emit bitcode objects and defer LTO and codegen to the linker (`R/src/doc/rustc/src/linker-plugin-lto.md`) **[V docs]**. That's the only stable way to move LLVM codegen out of rustc's process, and it moves it into the *link* step (cut point H). The MIR→LLVM IR lowering and pre-LTO optimization still run inside rustc.
  - **No flag of any kind lets rustc emit a per-CGU unit that another rustc process codegens.** **[V by absence in options.rs and the unstable book listing; a negative claim, moderate confidence]**
- Related: `-Zhint-mostly-unused` defers codegen of a crate's functions to the crates that use them (unstable-book `hint-mostly-unused.md`) **[V]**. Work then moves *downstream*, which matters if you model per-crate cost.

### 2.4 The incremental cache as a derivation input or output

**Don't use it as either in hermetic builds.** **[V for the facts, I for the recommendation]**

- `-C incremental` is `[UNTRACKED]` (options.rs 2292), so outputs are meant to be identical with or without it. Since PR #139453 (merged 2025-04-11, 1.88) each incremental invocation adds a **random per-invocation string** to temp CGU file names (`R/compiler/rustc_interface/src/util.rs` ~674-678: only `if sess.opts.incremental` is set) **[V]**. Buck2 states the rlib "is not byte-for-byte reproducible under `-Cincremental`, even if the source binary is unchanged", and cites that PR (`B/build.bzl` ~2077-2090) **[V Buck2 comment; the mechanism (random names end up as archive members) is I]**.
- Incremental also changes partitioning: 256 CGUs, volatile CGUs and different naming. The output *code* differs from a non-incremental build.
- Feeding the previous cache in as an input makes the output depend on build history, which is the opposite of a derivation. And emitting it as an output yields a big, non-reproducible blob that's useless to other users.
- Buck2: `incremental_enabled` is an attr that defaults to False. When it's set the action is `local_only`, `no_outputs_cleanup`, and `allow_cache_upload = False` (`B/build.bzl` ~2044-2104) **[V]**. It's a local developer mode that stays out of the cache.
- rules_rust: there's no incremental support in `rust/` at this commit. A grep for "incremental" only hits an LTO comment **[V]**.
- sccache refuses `CARGO_INCREMENTAL=1` unless `SCCACHE_ALLOW_INCREMENTAL=1`, and then skips caching incremental crates (`docs/Rust.md`) **[V]**.
- For Shikumi: the inner dev loop can use Cargo+incremental *outside* the store, or Shikumi's own impure local mode. The Nix graph stays non-incremental.

### 2.5 Build scripts, proc macros, `links`, features

- **build.rs.** It's two actions in every serious tool: compile, then run. The run output is parsed `cargo::` / `cargo:` directives: `rustc-cfg`, `rustc-env`, `rustc-link-lib`, `rustc-link-search`, `rustc-flags`, `rustc-link-arg*`, `rustc-cdylib-link-arg`, `metadata=K=V` (forwarded to dependents as `DEP_<LINKS>_<K>`), and `rerun-if-*`. See `RR/cargo/private/cargo_build_script_runner/lib.rs` ~30-110 and `B/tools/buildscript_run.py` ~200-260 **[V]**. Bazel sets `CARGO_MANIFEST_LINKS` and passes `--links` (`RR/cargo/private/cargo_build_script.bzl` ~583-639) **[V]**. reindeer won't run build scripts by default. Each one needs a `buildscript.run = true` fixup after a human reviews it, which yields `-build-script-build` and `-build-script-run` targets (`reindeer/docs/MANUAL.md` §Build and run build.rs) **[V]**. nixpkgs `buildRustCrate` runs the build script inside the crate's single derivation, with native inputs from `crateOverrides` / `defaultCrateOverrides` **[V excerpt of nixpkgs master default.nix]**.
  - `rerun-if-changed` has no meaning in Nix. The derivation's inputs decide reruns. Treat the whole package source plus declared native inputs as the input set **[I]**.
  - `links` creates a data edge from a *dependency's build-script run* to a *dependent's build-script run* via `DEP_*` env. Model it as an explicit derivation input (C of dep → C of dependent) **[I, consistent with RR/Buck2]**.
  - Hard problems people actually hit: scripts that write outside OUT_DIR break on a read-only store (crane FAQ "sandbox-unfriendly build scripts") **[V]**; build-script-generated code with unremapped paths (`remap-source-paths.md` "External tools") **[V]**; bindgen and pyo3 rebuild churn (crane FAQ entries) **[V titles]**.
- **Proc macros.** They're host dylibs loaded by rustc. Cargo compiles host units (proc macros) with `-C prefer-dynamic` (`prefer_dynamic = (for_host && !is_custom_build) || (contains_dylib && !primary)`, `C/src/compiler/mod.rs` ~1422) **[V]**. Buck2 keeps target-platform config out of a proc macro's `-Cmetadata` so one build serves every target ("host-agnostic", `B/build.bzl` ~1627) **[V]**. nixpkgs added `extraRustcOptsForProcMacro` so sanitizer and coverage flags stay off host dylibs **[V excerpt]**. The impurity risk is proc macros that read files or env and aren't tracked (sccache doc) **[V]**. There's also new nightly `-Zcache-proc-macros`, which caches expansions in incremental and assumes purity (unstable-book) **[V]**. In cross builds the proc macro and its deps, plus build scripts and their deps, are built for the build platform, so the same crate may appear twice (host and target) with different features **[I; standard Cargo behavior]**.
- **Features.** Cargo unifies features across the graph per platform and per host/target, with resolver 2 splitting host from target **[I; not re-read]**. This is the main per-crate caching hazard. Adding one dependency can enable a feature on a shared crate and rebuild everything above it (crane maintainer, discussion #213) **[V]**. crate2nix used to resolve features in Nix at eval time. Its new JSON mode moves resolution into Rust (PR #443) **[V search excerpt]**. It still can't handle target-specific features automatically (crate2nix known restrictions, #129) **[V]**. reindeer resolves features per configured platform to match Cargo's unification (MANUAL §Custom platforms) **[V]**.
- **`-C metadata`** (symbol-mangling disambiguator, `[TRACKED]`, so it's in the SVH) **[V]**. Cargo computes it from package id, features, profile, mode, LTO, compile kind, target name/kind, rustc version and the deps' metadata hashes. It doesn't use source content or RUSTFLAGS (`compute_metadata`, `C/src/compiler/build_runner/compilation_files.rs` ~769-898) **[V]**. rules_rust hashes the crate root path plus the label (`determine_output_hash`, `RR/rust/private/utils.bzl` 214) **[V]**. **Shikumi has to derive `-C metadata` from stable identity, never from the derivation's input hash or store path.** Otherwise every input change alters symbols and SVH even for byte-identical sources, which defeats CA cutoff downstream **[I]**.

### 2.6 Linking

- Final link inputs: the crate's own objects; every transitive rlib (archives of `.o` plus metadata, plus bitcode if `-C embed-bitcode=yes`, which is the default per options.rs) **[V default]**; sysroot std/core/alloc rlibs; native libs and search paths and link args from build scripts; target linker and its flags **[I list]**.
- Tests: `--test` makes rustc build the libtest harness binary. Cargo drops `--crate-type` for test units (`if !test`, mod.rs ~1381) **[V]**. Integration tests are separate crates that link the lib rlib. Unit tests are a full recompile of the lib crate with `--test` (§2.7).
- `-C prefer-dynamic` links std dynamically. Cargo uses it for host proc macros and non-primary dylibs **[V, above]**. A Shikumi dev mode could use dylibs for faster links, but it's not the default anywhere **[I]**.
- Buck2's split: under `advanced_unstable_linking`, bins compile with a fake `-Clinker=extract_link_action.py` that captures the objects rustc would have linked, then the C++ toolchain does the link like any `cxx_binary` (`B/tools/extract_link_action.py` header, `B/build_params.bzl` ~55-70 and ~130-150) **[V]**. It relies on rlibs being `.a`-compatible, which is unstable (rust-lang/rust#73632 per the Buck2 comment) **[V comment]**. That separates **I** from **F** for binaries and lets the link be its own cached action.
- Linker determinism: on MSVC the linker embeds paths in PDBs, and Apple OSO entries need `-oso_prefix` (`remap-source-paths.md`) **[V]**. crane's 0.22.0 changelog disables its remap hook on Darwin because Linux and Darwin Nix behave differently and that "invalidat[ed] build caches" **[V changelog]**.

### 2.7 Determinism knobs (what breaks byte-stability)

- Paths: `--remap-path-prefix` and `--remap-path-scope`. Only scope `all` (the default) remaps paths in **rustc metadata**. Narrower scopes leave local paths in `.rmeta`/`.rlib`/dylibs/proc macros (`remap-source-paths.md` §Metadata impact) **[V]**. `remap_path_prefix` itself is `[TRACKED_NO_CRATE_HASH]`, but `working_dir` (the *remapped* cwd) is `[TRACKED]` (options.rs 443, 485) **[V]**. Remap the sandbox build dir so the SVH doesn't depend on it. Also see `-Zremap-cwd-prefix` (unstable-book) **[V exists]**.
- Tracked flags in the SVH: anything `[TRACKED]`, including `opt-level`, `debuginfo`, `lto`, `metadata`, `target-cpu`, edition, target triple and emit kinds. Untracked: `codegen-units`, `incremental`, `save-temps`, `linker` (options.rs, §2.2) **[V]**. So `opt-level` changes rmeta, and debug/release graphs can't share metadata **[V by construction]**.
- `-C metadata` must be stable (§2.5).
- Incremental is non-reproducible (§2.4).
- Toolchain identity: rustc rejects rmeta from a different rustc (crane FAQ "found invalid metadata files") **[V]**. That's natural in Nix, since the toolchain is an input.
- Build-script and proc-macro impurity: the usual culprits are timestamps, `HashMap` order and env probing **[I]**.
- Parallel frontend (`-Zthreads`) determinism wasn't checked **[U]**.
- Undeclared inputs: dep-info lists source files and writes `# env-dep:NAME=VALUE` lines for env read via `env!`/`option_env!`, plus optional `# checksum:` lines with `-Zchecksum-hash-algorithm` (`R/compiler/rustc_interface/src/passes.rs` ~700-760) **[V]**. Shikumi can use it to *validate* that the declared input set covered everything (e.g. post-build in the derivation, failing on undeclared reads) **[I]**.

### 2.8 cargo-nextest's model

- Two phases. Build every test binary, then run `<bin> --list --format terse` (and `--ignored`) on each to enumerate tests. Then run each test in its own process as `<bin> <name> --nocapture --exact`, in parallel, with retries (`site/src/docs/design/how-it-works.md`, `custom-test-harnesses.md`) **[V]**. The libtest `--exact` binary search came in rust PR #154865 **[V doc]**.
- Partitioning: `--partition slice:m/n` (round-robin after filtering) or `hash:m/n` (`ci-features/partitioning.md`) **[V]**.
- Archives: `cargo nextest archive` bundles binaries, `cargo-metadata.json` and `binaries-metadata.json`, with OUT_DIRs only one level deep and no sources. `--workspace-remap` relocates the workspace. Reuse comes through `--binaries-metadata` / `--cargo-metadata` (`ci-features/archiving.md`) **[V]**. crane's `cargoNextest` already supports `partitions` and an archive step (`crane/docs/API.md` ~727-760) **[V]**.
- Shikumi mapping **[I]**: test exe (I) → listing derivation (J, cheap, deterministic) → result derivations (J′), one per *partition* or per test, keyed by (exe hash, test names, data inputs). An unchanged test exe with unchanged data cuts off for free. Per-test derivations make sense only for slow tests, because derivation overhead (~100 ms+ of sandbox setup, inferred) dwarfs millisecond unit tests. Batch by stable hash partition so membership doesn't churn. A test result has to be a *success-only* derivation, or a derivation that records failure as data. Choose that deliberately, because a failing derivation doesn't cache.

### 2.9 IDE metadata: `rust-project.json`

- Top level: `sysroot`, `sysroot_src`, `sysroot_project`, `cfg_groups`, `crates` (required, includes transitive and sysroot crates), and `runnables` (`program/args/cwd/kind`, where kind is `testOne|run|flycheck`, with `{label}` and `{test_id}` placeholders). Per crate: `root_module`, `edition`, `deps` (`{crate: index, name}`), `cfg`, `env`, `is_proc_macro`, `proc_macro_dylib_path`, `source` (`include_dirs`/`exclude_dirs`), `build` (`label`, `build_file`, `target_kind`), `target`, `display_name`, `version`, `is_workspace_member`, `repository`, `crate_attrs`. Discovery can be a `rust-project.json` at the root, `linkedProjects`, or `rust-analyzer.workspace.discoverConfig` (a command emitting JSONL). Flycheck is off unless a `flycheck` runnable or `check.overrideCommand` is configured, and it must emit JSON diagnostics (https://rust-analyzer.github.io/book/non_cargo_based_projects.html, fetched 2026-11-07) **[V]**.
- The page doesn't mention build scripts or OUT_DIR **[V]**. Shikumi has to run build scripts and pass `OUT_DIR` (and `rustc-env` values) through `env`, `rustc-cfg` through `cfg`, and point `proc_macro_dylib_path` at the built proc-macro store path **[I]**. Buck2 has a `rust-analyzer` subdir in its prelude that does this (`B/rust-analyzer/`, not read) **[U]**.

---

## 3. Prior-art lessons

| Tool | Cache unit | Key lesson |
|---|---|---|
| **crane** | Two derivations: `buildDepsOnly` (all deps, built from `mkDummySrc` stubs), then the workspace. The `target/` dir is passed as a zstd tarball (default `use-zstd`) or symlink-deduped (`use-symlink`) (`docs/API.md` ~1995) **[V]** | It doesn't reimplement Cargo, so it stays correct. Per-crate caching isn't "reliably possible" because of feature unification and lockfile-wide validity (discussion #213) **[V]**. Any Cargo.lock change rebuilds every dep. Toolchain mismatches cause full rebuilds or "invalid metadata" errors (FAQ) **[V]**. |
| **crate2nix** | One derivation per crate via nixpkgs `buildRustCrate` **[V]** | Eval cost dominated: Cargo.nix carried O(n·m) feature and cfg resolution as Nix code. PR #443 moved resolution into Rust and emits JSON. On a 1932-crate / 317-member workspace, `.drv` creation went from 229.7 s to 18.2 s (excerpt) **[V excerpt]**. IFD copied the whole tree (#347) **[V]**. Target-specific features don't work automatically (#129). A crate can only see its own source dir (#17) (known restrictions) **[V]**. |
| **nixpkgs buildRustCrate** | Per crate; re-implements Cargo's rustc invocation in bash. `defaultCodegenUnits ? 1`. Transitive `completeDeps` **[V excerpt]** | Native deps go through a central `defaultCrateOverrides` table. That's a maintenance treadmill. bin targets used to build serially, and nixpkgs PR #503809 (2026-03) added a make jobserver **[V excerpt]**. |
| **cargo2nix** | Per crate, but each derivation runs `cargo build` with a wrapped `RUSTC` that injects prebuilt deps (`overlay/mkcrate.nix` ~37, 108, 265) **[V]** | It reuses Cargo's logic per crate. Last commit was 2025-06, so it's effectively unmaintained **[V date]**. |
| **nix-cargo-integration / dream2nix** | dream2nix modules `rust-cargo-lock`, `rust-cargo-vendor` and `rust-crane`, so crane granularity (`src/implementation.nix` via search) **[V excerpt]** | A UX layer, not a new cut point. |
| **cargo-nix-plugin (Mic92)** | Per crate via `buildRustCrate`. A Nix plugin primop `builtins.resolveCargoWorkspace` resolves from Cargo.lock and the sparse index at eval time **[V README]** | Moves resolution into native code inside the evaluator. The plugin ABI is tied to the Nix version. Doctests and per-bin unit tests aren't built **[V README]**. drowse uses dynamic derivations to dodge IFD (Discourse, 2026-02) **[V excerpt]**. Both are evidence that resolution belongs outside Nix-language eval. |
| **Bazel rules_rust** | Per crate; optional pipelining (metadata action plus full action) **[V]** | Pipelining on stable: same args, rustc killed at the rmeta notification. Build scripts get a separate `cargo_build_script` with directive parsing and `links` env. `-Cmetadata` comes from a path/label hash. Paths are remapped (`--remap-path-prefix=${output_base}=...`, rustc.bzl ~1357) **[V]**. crate_universe generates BUILD files from Cargo (not read) **[U]**. No incremental. |
| **Buck2 prelude + reindeer** | Per crate × emit kind. `metadata-fast` (check), `metadata-full` (a `-Zno-codegen` hollow rlib for pipelining), `rlib`, `link`, plus `llvm-ir`/`mir`/`expand`/`clippy`/`miri` (`B/build_params.bzl`, `B/build.bzl`) **[V]** | The most thorough model. It shows the SVH constraint and the `-Zno-codegen` answer, host-agnostic proc macros, the fake-linker object extraction so linking is a separate C++-toolchain action, local-only uncached incremental, `has_content_based_path` outputs (content-addressed paths, the analogue of CA derivations), and stripping `.dwo` from rlibs. reindeer: build scripts are opt-in per crate via fixups, and features are resolved per platform **[V]**. |
| **sccache** | Per rustc invocation, keyed on compiler, args, cwd, env, externs, static libs, target spec and dep-info source contents **[V]** | Only `link`, `metadata` and `dep-info` emits. Can't cache anything that invokes the linker (bin, dylib, cdylib, proc-macro). Proc macros reading files aren't tracked. Proc macros are identified by crate metadata, so native-code changes are missed. No incremental (`docs/Rust.md`) **[V]**. |

Cross-cutting lessons:
1. Do resolution once, natively, outside Nix eval, and emit plain data for Nix to consume (crate2nix JSON, cargo-nix-plugin, reindeer) **[V]**.
2. Feature unification is graph-global. Per-crate derivations are correct only if each crate's resolved feature set is part of its identity, and Cargo invalidates the same way **[V crane #213; I]**.
3. Build scripts need an escape hatch (fixups or overrides) and a review stance (reindeer) **[V]**.
4. Pipelining needs two rustc processes per crate in any action-based system **[V Buck2/RR]**.
5. Keep incremental out of the shared cache (Buck2, sccache) **[V]**.

---

## 4. Risks

1. **No byte-level early cutoff at the crate interface.** Any edit to crate X rebuilds everything that transitively depends on X, because the SVH flows through every `CrateDep` (§2.2) **[V]**. CA derivations only pay off for identical rebuilds. Expect rebuild cost of roughly "crate plus all its dependents" per edit, the same as Cargo minus incremental.
2. **The SVH algorithm changed recently.** Metadata-byte hashing is the default on this nightly, with `-Zmetadata-crate-hash=no` as a fallback. Stable 1.9x may still use the HIR-based SVH **[U: which stable release first ships it]**. The conclusion holds either way, because both include body and span hashes.
3. **Pipelining doubles frontend CPU** (§2.1). For heavy-frontend crates (big generic code, proc-macro-heavy) that can be a significant fraction of total CPU. The `-Zno-codegen` route needs nightly or `RUSTC_BOOTSTRAP` **[V/I]**.
4. **The CGU level isn't usable** with stable or nightly flags. Non-incremental naming is unstable, local ThinLTO couples CGUs, and no flag hands per-CGU work to another process (§2.3).
5. **Build scripts and proc macros are arbitrary code.** Hermeticity is enforced by the sandbox, but correctness (reading undeclared files or env) only gets detected through dep-info `env-dep` lines and by tests **[I]**.
6. **Eval cost.** A large workspace means thousands of derivations. Generating them in Nix-language eval was the 230 s problem (§3) **[V]**.
7. **Sandbox path drift.** The SVH includes the remapped `working_dir`. If the sandbox build dir path differs across Nix versions or platforms and isn't remapped, the cache splits (cf. crane's Darwin note) **[I]**.
8. **Test results in Nix.** A failing derivation isn't cached, and flaky tests poison CA (§2.8) **[I]**.
9. **Nightly-only features.** `-Zno-codegen`, `-Zembed-metadata` (now stable as `-Cembed-metadata`, options.rs 2282 **[V]**), `thin-link-bitcode`, `-Zno-link`, `-Zremap-cwd-prefix` and `-Zhint-mostly-unused` may change underneath Shikumi.

---

## 5. Smallest spikes to settle what's still open

| Open question | Smallest spike |
|---|---|
| S1. Do private body, comment and whitespace edits change `.rmeta` bytes on *current stable*? (2.2, risk 2) | Two-crate workspace `a ← b`. Build `a` with `--emit=metadata,link` and remapped paths. Edit a private fn body, then just add a comment line. `cmp` the rmeta and `b`'s rlib. Repeat with `-Zmetadata-crate-hash=no` on nightly. |
| S2. Is the rmeta from a killed metadata run byte-identical to the full run's? (2.1, rules_rust route on stable) | One crate. Run (a) `--emit=dep-info,metadata,link` killed at the rmeta artifact JSON, and (b) the same run to completion. Compare rmeta bytes, then build a dependent against (a) and link it against (b)'s rlib. Repeat with `-Zno-codegen --emit=link` vs the normal run on nightly. |
| S3. Are crate outputs reproducible across two Nix sandboxes? | Build the same crate in two derivations with different `name`s (so the build dirs differ), using `--remap-path-prefix=$NIX_BUILD_TOP=/build` and stable `-C metadata`. Use `nix build --rebuild` or `diffoscope` on rlib/rmeta. Add a crate with a build script and one with a proc macro. |
| S4. Can per-CGU bitcode be codegenned out of process identically? (cut point G) | `-Ccodegen-units=4 -Csave-temps -Copt-level=0 -Clto=off`. Take each `*.no-opt.bc`, compile with the `llc`/`opt` from rustc's own LLVM (`rustc -vV` LLVM version) using matching `-mcpu`/reloc and code model flags, then `cmp` against rustc's `.rcgu.o`. If it fails even at O0, drop G. |
| S5. Does distributed ThinLTO work on rustc bitcode? (cut point H) | Binary plus two crates built with `-Clinker-plugin-lto -Clto=thin -Zunstable-options --emit=thin-link-bitcode,link`. Thin-link with `ld.lld --thinlto-index-only`, run per-module backends with `clang -fthinlto-index=` (same LLVM major), final link, and compare behavior and size with in-process `-Clto=thin`. |
| S6. Derivation overhead vs crate size (property 4) | Pick a real workspace (~300 crates). Measure the per-crate rustc wall time distribution (`cargo build --timings`) against a no-op derivation's realisation overhead on a Linux workstation. Use that to set the threshold for merging tiny crates or tiny test partitions. |
| S7. Eval cost of the per-crate graph | Generate the `.drv` set for a ~2k-crate graph from pre-resolved JSON, both with a trivial Nix consumer and with dynamic derivations. Time `nix-instantiate` against the crate2nix #443 numbers (18 s at 1932 crates). |
| S8. Unit-test listing and partition stability | On one test binary, run `--list --format terse` twice and after an unrelated edit, then check hash-partition membership churn. |
| S9. `rust-project.json` with Shikumi outputs | Hand-write one for a 3-crate graph with one build script (OUT_DIR via `env`) and one proc macro (`proc_macro_dylib_path` into the store). Check that rust-analyzer resolves `include!(concat!(env!("OUT_DIR"), ...))` and expands the macro. |

## 6. Recommended shape (inferred, for the design doc)

Per crate × (profile, target, resolved feature set), the derivations are:
- build-script compile and build-script run (if any), with `DEP_*` edges for `links`;
- **meta**: a full-metadata rmeta via `-Zno-codegen` (or kill-on-rmeta on stable);
- **rlib**: the full codegen, depending on upstream **meta** only, plus upstream rlibs only for linking (proc macros need the full dylib);
- **link**: bins and tests, depending on all transitive rlibs;
- **test-list** and partitioned **test-run**;
- a separate **check** graph on `--emit=metadata` for CI linting and IDE flycheck.

Resolution (cut point A) runs natively and emits JSON. `-C metadata` comes from stable identity, paths are remapped with scope `all`, there's no incremental, and CA derivations give whatever cutoff identical rebuilds allow.
