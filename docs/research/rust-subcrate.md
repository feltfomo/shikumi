# Shikumi research: Rust granularity below the crate

Scope: what actually lets a one-function edit in a large crate redo only the work that edit affects, with the results living in the Nix store. Research only. Nothing was built, and no repository was edited.

Constraint from the user (applied throughout): **no patched rustc.** Avenues that work from outside rustc rank first: a `rustc_driver` tool, a `-Zcodegen-backend` dylib, wrapper flags, or Shikumi-side caching around an unmodified toolchain. Each avenue is labelled with one toolchain class:

- **STABLE**: works on an unmodified stable toolchain with stable flags only.
- **STABLE+BOOTSTRAP**: an unmodified stable toolchain, but Shikumi builds a tool against its `rustc-dev` component with `RUSTC_BOOTSTRAP=1` (needed for `#![feature(rustc_private)]`). The shipped compiler binaries aren't touched.
- **NIGHTLY**: an unmodified pinned nightly (or `-Z` flags on stable via `RUSTC_BOOTSTRAP=1`, which amounts to the same).
- **PATCH**: needs a change to rustc. Last resort only, and each one names the closest outside alternative.

Tags: **[V]** verified in source, docs or the release manifest at the versions below. **[I]** inferred from verified facts and not run. **[U]** unverified, and a spike settles it.

## Sources

| Source | Version | Notes |
|---|---|---|
| rust-lang/rust | `e30d778f3029b78bb9c2e0dbe11a3b7389b652c4` (2026-11-07, 1.102 nightly) | The same clone the earlier pass used, with these sparse paths added: `rustc_incremental`, `rustc_middle/src` (dep_graph, mono), `rustc_codegen_llvm`, `rustc_codegen_cranelift/src`, `rustc_codegen_gcc/src`, `rustc_llvm/llvm-wrapper`, `rustc_span`. Paths below are relative to `compiler/`. |
| Stable channel manifest | `channel-rust-stable.toml`, date 2026-10-15, rustc 1.99.1 (`58cf8f9c3`) | Fetched from static.rust-lang.org. |
| `rustc-dev-1.99.1-x86_64-unknown-linux-gnu.tar.xz` | 1.99.1 | Only the file listing was read. |
| Web | fetched 2026-11-07 | Kobzol "What part of Rust compilation is the bottleneck?" (2024-03), Lattimore "Speeding up the Rust edit-build-run cycle" (2024-02), "Designing Wild's incremental linking" (2024-11), Wild 0.6.0 update (2025-09), Wild 0.10 coverage (2026-08), Nethercote "Back-end parallelism" (2023-07), Rust compiler performance survey 2025 results, project goals 2026 (Fast Builds roadmap, Parallel Front End, Relink Don't Rebuild, Incremental Systems Rethought), rust#158844, rust PRs #156940/#155871/#151265, compiler-team MCP #790, RFC 3881, rust#137560, Dioxus subsecond docs and PR #3797, the ipbp README, the kache issue, howardjohn's shared-cache post, project goal #626 (Cargo cross-workspace cache). Most were read as search excerpts. Those count as [V] only for the excerpted text. |

---

## 0. Verdict on the earlier pass's findings

| Earlier claim | Status now |
|---|---|
| The crate hash covers all bodies and spans | Still true on this nightly **[V earlier pass]**. Relink Don't Rebuild (RDR) is changing it: tracking issue rust#158844 adds an unstable `public_api_hash` option, and the draft PR #156940 is under review **[V excerpt]**. That's a cross-crate cutoff, not a sub-crate one. |
| Non-incremental CGUs are size-merged and renamed per build | **[V]** `rustc_monomorphize/src/partitioning.rs` 471-507: sorted by size and named `<crate>.<hash>-cgu.N`. Correction: the "merge anything under 1800" loop runs **only when the CGU count is the default** (`CodegenUnits::Default`, 400-403), and the merge-to-max loop runs only while `len > max` (345). **With an explicit large `-Ccodegen-units`, a non-incremental build keeps one CGU per source module, but the names are still size-ordered indices.** `-Ccodegen-units` is untracked, so it doesn't affect the SVH **[V earlier]**. |
| `--emit=llvm-bc` without `-Ccodegen-units` forces one CGU | Not re-read. Carried over **[V earlier]**. |
| `-Zno-codegen` gives a hollow rlib with the same SVH | Not re-read. Carried over **[V earlier]**. |
| Incremental uses random temp suffixes, so rlibs aren't byte-reproducible | **Mechanism now verified end to end.** `rustc_interface/src/util.rs` 674-678 draws a random `invocation_temp` only when `-Cincremental` is set. `rustc_session/src/config.rs` 1337-1341 appends it to every per-CGU temp path. The LLVM backend archives through `ArArchiveBuilder` (`rustc_codegen_llvm/src/back/archive.rs`), whose `add_file` uses the **file name as the archive member name** (`rustc_codegen_ssa/src/back/archive.rs` 550-554) **[V]**. So the only rlib difference the source shows is the member names. Whether object *bytes* also differ is **[U]**, and spike A1 checks it. |
| "No flag hands per-CGU work to another process" | Still true for flags. But a **driver can get between the partitioner and the backend**, which flags can't (§1). |

---

## 1. Ranked avenues

Ranked by feasibility × payoff under the no-patch constraint.

| Rank | Avenue | Toolchain | What gets reused below the crate | Payoff | Feasibility |
|---|---|---|---|---|---|
| **1** | **A. Store-seeded incremental as an accelerator.** Shikumi runs plain `rustc -Cincremental=<scratch>` and seeds the scratch dir from a snapshot it keeps in the store, built from content-addressed parts. Outputs are re-archived canonically and added to the store by content. | **STABLE** | Frontend queries (typeck, borrowck, MIR of unchanged items) and per-CGU objects | High for debug and dev loops, where the frontend and LLVM both matter | High. It's just files and flags. |
| **2** | **B. Shikumi codegen driver: stable CGU names plus a per-CGU object cache keyed by unoptimized bitcode.** | **STABLE+BOOTSTRAP**, pinned per toolchain | LLVM optimization and machine codegen per CGU, shared across machines and branches with no dep graph | High for release and opt builds, where LLVM dominates. Medium for debug, since frontend and lowering still run in full. | Medium. It uses internal APIs and has a porting burden. |
| **3** | **C. Release: `-Clinker-plugin-lto` with rust-lld's ThinLTO cache pointed at a Shikumi CAS dir** | **STABLE** (the cache flag is an lld option) | ThinLTO backend (opt plus codegen) per module, at link time, keyed by LLVM's own cache key | High for LTO release binaries | Medium-high. **[U]** whether rustc bitcode and rust-lld line up (spike C1). |
| 4 | **A + B combined.** B fills A's misses: when the local incremental state lacks a CGU, B's store cache may have it from another machine or branch. | STABLE+BOOTSTRAP | both | Highest | Medium |
| 5 | **D. Out-of-process per-CGU codegen** (distribute CGUs to other machines) | STABLE+BOOTSTRAP for emission. Parity of a separate `llc` is **[U]** | CPU fan-out, not reuse | Medium. It's parallelism, not incrementality. | Low-medium (spike S4 from the earlier report) |
| 6 | **E. Linking:** fast linker now, and prelinked dependency dylibs for dev | STABLE | n/a | Removes the fixed cost after every edit | High (lld/mold/wild now). Incremental linking doesn't exist yet. |
| 7 | **F. Cross-crate cutoff (RDR)** so an edit doesn't rebuild dependents | NIGHTLY (once it lands) | not sub-crate, but multiplies everything above | Very high in multi-crate graphs | Pending upstream |
| — | **P. Patches** (§6) | PATCH | — | — | Last resort only |

The core asymmetry, which the rest of the report backs up:

- **Codegen can be made sub-crate from outside without incremental mode**, because CGU membership is computable and the LLVM work per CGU is a pure function of its unoptimized bitcode plus config (avenue B).
- **The frontend can't be made sub-crate without rustc's incremental query system.** Outside incremental there's no per-item reuse at all: `collect_and_partition_mono_items` walks every reachable item, which forces typeck, borrowck and MIR optimization for the whole crate (`partitioning.rs` 1169-1183) **[V]**. So frontend reuse means using incremental (avenue A) or patching rustc. There's no third route.

---

## 2. Avenue A: incremental as a store-seeded accelerator (STABLE)

### 2.1 How reuse actually works (what a store-backed cache would have to replay)

- CGU reuse is decided by `determine_cgu_reuse` (`rustc_codegen_ssa/src/base.rs` 1253-1296) **[V]**. It returns `No` unless the dep graph is fully enabled, and unless a **previous work product with the same CGU name** exists. It then calls `tcx.dep_graph.try_mark_green(CompileCodegenUnit dep node)`. **There's no content fingerprint of a CGU.** Reuse is "every query this CGU's codegen read last time is green". Green CGUs give `PostLto` (no LTO, so the `.o` is reused) or `PreLto` (the pre-LTO bitcode is reused) (1283-1295) **[V]**.
- Codegen of a CGU runs inside `dep_graph.with_task(codegen_dep_node, …)` in the LLVM backend (`rustc_codegen_llvm/src/base.rs` 70-76) and in cg_clif (`rustc_codegen_cranelift/src/driver/aot.rs` ~294) **[V]**. That's how those reads get recorded.
- Work products are copied into the session as `{cgu_name}.{ext}`, so they have **deterministic names** (`rustc_incremental/src/persist/work_product.rs` 20-50) **[V]**. They're hard-linked from the old session when unchanged (`link_or_copy`) **[V]**.
- On load, rustc discards the whole previous session if `dep_tracking_hash(false)` differs. That hash covers every `[TRACKED]` flag, including the remapped working dir. It also discards it if the header's rustc version differs, or if any listed work-product file is missing (`persist/load.rs` 35-106, `file_format.rs` 188-200) **[V]**.
- ThinLTO under incremental: rustc stores `thin-lto-past-keys.bin` and reuses a post-LTO module only if the module is green **and** LLVM's ThinLTO cache key matches (`rustc_codegen_llvm/src/back/lto.rs` 36-38, 464-528). The key comes from `LLVMRustComputeLTOCacheKey`, which wraps LLVM's `computeLTOCacheKey` with import and export lists, resolved ODR and defined globals, but builds it with a **default `lto::Config`** (`rustc_llvm/llvm-wrapper/PassWrapper.cpp` 1631-1656) **[V]**. So it doesn't encode opt level or target options. That's fine inside one session dir, but a cross-machine cache would have to add them (relevant to B and C).

### 2.2 Exactly what makes the session dir and its outputs non-reproducible

| Source | Where | Fundamental? | Outside fix |
|---|---|---|---|
| Session dir name `s-{timestamp}-{random u32}-working`, renamed to `s-{ts}-{rand}-{SVH}` on finalize | `persist/fs.rs` 379-389 (`SystemTime::now()`, `rng().next_u32()`), 277-312 **[V]** | No, it's naming only. rustc picks the newest finalized dir by the timestamp parsed from the name (`find_source_directory_in_iter` ~466-499) **[V]**. | Shikumi stores the *contents*, not the name. On import it writes any valid finalized name, such as `s-<fixed ts>-<fixed>-<svh>`. |
| Lock files next to each session dir. Shared lock on the old dir, exclusive on the new one. | `fs.rs` 408-447 **[V]** | No | Import into a writable scratch crate dir. Never point rustc at a store path directly, because `create_dir_all` and lock creation would fail **[I]**. |
| GC deletes the old session dir and other stale dirs after finalize | `fs.rs` 349-351, 555-770 **[V]**. Failures are only warnings (`delete_old`, 773-781) **[V]**. | No | Seed with a reflink or copy. The GC then deletes the scratch copy, never the store. |
| Hard links from old to new session (`link_or_copy`) | `work_product.rs` 33 **[V]** | No | Same filesystem as the scratch dir gets cheap links. Otherwise it copies. Reflinks (btrfs/xfs) give real copy-on-write **[I]**. |
| Random `invocation_temp` in temp CGU file names, which become rlib member names | `rustc_interface/src/util.rs` 674-678, `rustc_session/src/config.rs` 1337-1341, `rustc_codegen_ssa/src/back/archive.rs` 550-554 **[V]** | No. It exists to stop concurrent invocations clobbering each other (rust PR #139453). | Shikumi re-archives: either take the objects and build the rlib itself (members `lib.rmeta` plus objects under canonical names), or skip rlibs for objects and link directly (Buck2's fake-linker trick) **[I]**. |
| Dep-graph file node order. "Dep-graph indices are bulk allocated to threads", and nodes are emitted on demand in execution order. | `rustc_middle/src/dep_graph/serialized.rs` header doc, lines 7-8 and 38-40 **[V]** | **Yes, by design.** The file depends on history (which nodes were promoted green rather than re-executed) and on thread scheduling under `-Zthreads` **[I]**. | Don't need it reproducible. Treat the snapshot as an *untrusted accelerator*, stored by content but never a derivation whose bytes anyone depends on. A canonical re-serialization would be a patch (§6). |
| Query result cache (`query-cache.bin`) order | written in execution order **[I, not read in detail]** | Same as above | Same |
| Absolute paths | `StableSourceFileId` hashes `FileName`. For `RealFileName`, the **local path is excluded when the name is fully remapped** (`rustc_span/src/lib.rs` 331-343, the #70924 fix) **[V]**. The remapped working dir is `[TRACKED]`, so it's part of `dep_tracking_hash` **[V earlier]**. | No | Remap the source root and cwd to a fixed prefix with scope `all`, or run rustc under a fixed path (as the Nix sandbox's `/build` already is). A snapshot made under one checkout path is then valid under another **[I, spike A2]**. |
| Crate dir name `{crate}-{StableCrateId base36}` | `fs.rs` 542-547 **[V]** | No | `StableCrateId` comes from the crate name and `-C metadata`, which Shikumi already derives from stable identity (earlier report §2.5). |
| rustc version in every file header | `file_format.rs` 188-200 **[V]** | Correct behavior | Snapshots are keyed by toolchain. |

**Conclusion [I]:** none of the non-reproducibility makes incremental unusable from the store. It makes the *snapshot* history-dependent, which is fine for a cache hint. It also makes rlib member names random, and Shikumi can canonicalize those after the fact. What has to hold is rustc's own invariant: *incremental output is a function of the sources and tracked flags, whatever base it started from*. `-Zincremental-verify-ich` (UNTRACKED, rehashes green query results, `rustc_session/src/options.rs` 2616) exists to check exactly that **[V]**. Rust 1.52.1 shows the invariant has been violated before (incremental was disabled after verification ICEs) **[I, from memory, not re-read]**.

### 2.3 Design sketch

1. **Key the snapshot** by (toolchain, target, `dep_tracking_hash` inputs = tracked flags and remap, crate identity/`-C metadata`). The source revision is *not* in the key, because any snapshot of the same crate identity is a valid base and only the hit rate changes.
2. **Store the snapshot as a manifest plus CAS blobs**: `dep-graph.bin`, `query-cache.bin`, `work-products.bin`, and each `{cgu}.o`/`.bc` as its own store path or CAS object. Per-CGU objects dedup across snapshots and branches. This is the same "entry with pointers into CAS" design Cargo's cross-workspace cache goal settled on (rust-project-goals #626, cargo#5931) **[V excerpt]**.
3. **Pick a base**: the snapshot from the nearest ancestor commit, the last CI build, or the user's last local build. Shikumi materializes it into `<scratch>/incr/<crate>-<scid>/s-…-<svh>/` by reflink, hardlink or copy. All work-product files must exist, or rustc drops the whole base (`load.rs` 64-78) **[V]**.
4. **Run unmodified rustc** with `-Cincremental=<scratch>/incr`, a large explicit `-Ccodegen-units` so CGUs map 1:1 to modules and there's no merge churn (`partitioning.rs` 345, 400-403) **[V]**, and full path remapping.
5. **Collect outputs**: re-archive the rlib canonically, or keep the objects, then `nix store add` by content. Export the new snapshot (step 2).
6. **Trust**: CI and release builds stay non-incremental and fully hermetic. The incremental path covers the dev loop and the "build my branch fast" case. Optionally spot-check by occasionally rebuilding from scratch and comparing canonicalized outputs.

Granularity this buys: frontend reuse per query and per item (red-green), and codegen reuse per CGU (module, split into stable and volatile halves, `partitioning.rs` 229-231) **[V]**.

### 2.4 Smallest spikes for A

- **A1 (settles: is the output a function of the sources only?)** One medium crate (~30 modules) built three ways with `-Cincremental`: (a) a fresh empty dir; (b) seeded from a snapshot taken *before* a one-function edit; (c) seeded from a snapshot taken after an unrelated edit elsewhere. Compare each `.o`, and the rlib after canonical re-archiving. The expectation is that they're identical apart from member names **[U]**.
- **A2 (settles: is it relocatable?)** Build in `/tmp/x/src` and seed a build in `/tmp/y/src` with `--remap-path-prefix=<root>=/src` and `--remap-path-scope=all`. Check reuse with `-Zincremental-info` on nightly, or with `-Zself-profile` counts (`codegen_copy_artifacts_from_incr_cache`). On stable, time the build and count CGU work-product hard links.
- **A3 (settles: is the payoff real?)** On a real big crate, compare a one-function edit seeded from the parent-commit snapshot against a cold non-incremental build and against warm local `cargo build`.

---

## 3. Avenue B: a Shikumi codegen driver (STABLE+BOOTSTRAP, pinned)

### 3.1 What the extension points allow (all **[V]** at this commit)

- `rustc_interface::Config` exposes `override_queries: Option<fn(&Session, &mut Providers)>` and `make_codegen_backend: Option<Box<dyn FnOnce(&EarlySession) -> Box<dyn CodegenBackend>>>`. Its doc says the latter "is used by bjorn3 for the hotswapping branch of cg_clif" (`rustc_interface/src/interface.rs` 340-375). A driver sets both in `Callbacks::config(&mut Config)` (`rustc_driver_impl/src/lib.rs` ~122).
- The partitioner is a query provider: `providers.queries.collect_and_partition_mono_items` and `providers.queries.codegen_unit` (`partitioning.rs` 1353-1365). An override can keep the default function pointer in a static, call it, then post-process the result.
- `CodegenUnit` has public `set_name`, `set_symbol_name`, `items()` and `items_mut()` (`rustc_middle/src/mono.rs` 400-460). The symbol name is the prefix for backend-generated globals (`generate_global_symbol_name`, `rustc_codegen_llvm/src/context.rs` ~1181-1191, and the same in cg_gcc). In non-incremental mode it's derived from the size **index** (`partitioning.rs` 502-506), which is why non-incremental objects churn.
- `CodegenBackend::codegen_crate`, `join_codegen` and `link` (`rustc_codegen_ssa/src/traits/backend.rs` 37-140). Out-of-tree backends drive codegen through the shared `rustc_codegen_ssa::base::codegen_crate(backend, tcx)` by implementing `ExtraBackendMethods` and `WriteBackendMethods`. cg_clif does exactly this now (`rustc_codegen_cranelift/src/driver/aot.rs` 270-330).
- LLVM's `WriteBackendMethods::{optimize, codegen, optimize_and_codegen_thin, serialize_module}` are associated functions **without `self`** (`rustc_codegen_llvm/src/back/llvm_backend.rs` 83-176). `LlvmCodegenBackend` is `pub` and `Clone`, but its field is private, so only `new() -> Box<dyn CodegenBackend>` constructs it (40, 178-181).
- The **stable** 1.99.1 `rustc-dev` tarball ships `librustc_codegen_llvm-*.rmeta`, `librustc_codegen_ssa`, `librustc_monomorphize`, `librustc_incremental`, `librustc_interface` and `librustc_driver-*.so` (tarball listing) **[V]**. So a driver can name the LLVM backend's types on a stable toolchain.

### 3.2 Design

1. **Stable CGU identity without incremental.** Run with an explicit huge `-Ccodegen-units` (one CGU per module, no merges), then use `override_queries` to wrap `collect_and_partition_mono_items`. Rename each CGU from **its own contents only**, for example the module path of its first non-inlined root item, plus a `-generic` tag for volatile-style splits if the driver also splits generics. Set `symbol_name` the same way and re-sort by name, since the partitioner asserts name order (`partitioning.rs` 189-196) **[V]**. Nothing about another CGU's size then leaks into a CGU's bytes **[I]**. This replaces the `-Zhuman-readable-cgu-names` idea: that flag only affects names of **merged incremental** CGUs (`partitioning.rs` 448-457) **[V]**, so it doesn't help non-incremental builds.
2. **Per-CGU cache key = hash of the unoptimized LLVM module plus the full codegen config.** A wrapper type `Shikumi` implements `ExtraBackendMethods` (delegating `compile_codegen_unit` to the LLVM backend, i.e. the MIR-to-LLVM-IR lowering) and `WriteBackendMethods`. Its `optimize`/`codegen` serialize the module (`serialize_module`), hash it together with target, CPU and features, opt level, `ModuleConfig` and the LLVM version, and look the hash up in the store. On a hit it writes the cached `.o` where `CompiledModule` expects it and skips LLVM. On a miss it calls `<LlvmCodegenBackend as WriteBackendMethods>::codegen` and adds the result by content. `codegen_crate` becomes `rustc_codegen_ssa::base::codegen_crate(Shikumi, tcx)`.
   - The key is **sound by construction**. Whatever rustc lowered, including linkage and visibility after internalization, inlined `LocalCopy` items, debuginfo spans and anonymous global names, is all in the bitcode. A hand-written semantic key (instance MIR plus layouts plus callee symbols) would have to re-derive the dep graph, and a missed input is a silent miscompile. Don't hand-roll it.
   - Cost: the frontend and lowering run every time. Only LLVM opt and machine codegen are skipped. Under ThinLocal LTO (opt > 0 with more than one CGU, earlier report §2.3), cache at `optimize_and_codegen_thin` with LLVM's `computeLTOCacheKey` *plus* the config, as rustc's own incremental does (§2.1).
   - Delegating `compile_codegen_unit` needs a `&LlvmCodegenBackend`, and its field is private. It's a zero-sized `pub struct LlvmCodegenBackend(())`, so it can be conjured unsafely. That's a hack, and the clean fix is a one-line upstream `pub fn` (§6) **[I]**.
3. **Driver form:** a `shikumi-rustc` binary using `rustc_driver::run_compiler` with Callbacks, built like clippy-driver against the toolchain's `rustc-dev`. It needs no `-Z` flags at runtime, because options are set on `Config` after parsing and so skip the nightly-flag check **[I: the check runs in option parsing, not verified for every field]**. The `-Zcodegen-backend=<dylib>` form also works, but it needs a `-Z` flag (NIGHTLY or bootstrap) and only gets the backend hook. `provide()` on the backend also lets it override queries (`traits/backend.rs` 100-103, "rustc_codegen_spirv and rustc_codegen_nvvm use it") **[V]**.

### 3.3 Stability burden

- `rustc_private` APIs change with every nightly. A driver is compiled for **exactly one** toolchain and dynamically links that toolchain's `librustc_driver-<hash>.so` **[V for the .so in rustc-dev; I for the consequence]**. clippy and miri escape the burden by living in-tree, while Kani, Prusti, Creusot, rust-gpu and others pin a nightly and port periodically **[I, general knowledge, not re-read]**.
- Supporting stable means building one driver per stable release (every 6 weeks) from that release's `rustc-dev` with `RUSTC_BOOTSTRAP=1` **[V that rustc-dev exists for stable 1.99.1; I for the workflow]**. In Nix that's one derivation per toolchain, so the cost is porting effort, not distribution.
- The surface B touches: `Callbacks`, `Config::{override_queries, make_codegen_backend}`, `Providers.queries.*` field names, `CodegenUnit` setters, `ExtraBackendMethods`/`WriteBackendMethods` signatures, `ModuleConfig` and `CompiledModule`. The write-backend traits are the churniest: their current signatures carry `SharedEmitter`/`SelfProfilerRef` parameters (`llvm_backend.rs` 143-176) **[V]**. A partition-only driver (step 1 alone) has a much smaller surface.

### 3.4 Smallest spikes for B

- **B1 (settles: stable names and bitcode determinism)** A driver that only overrides the partition rename and dumps `sha256(serialize_module)` per CGU. Run it twice on the same crate (same bytes?), from two checkout paths (same bytes with remap?), and after a one-function edit (how many CGUs change?). This is the cheapest proof that the foundation holds.
- **B2 (settles: hit path correctness)** Add the write-hook cache at `-Copt-level=0` with no LTO. Do a cold build, then an edit, then a rebuild. Link the result and run the test suite. Compare against a plain rustc build with the same partition.
- **B3 (settles: payoff split)** On a big crate, use `-Ztime-passes` or self-profile to split frontend, `codegen_to_LLVM_IR` (lowering) and LLVM time at opt 0 and opt 3. B saves only the LLVM share.

---

## 4. Avenue C: release builds through linker-plugin LTO and the ThinLTO cache (STABLE)

- `-Clinker-plugin-lto` is stable and makes rlib objects LLVM bitcode, so LTO and codegen run in the linker **[V earlier, linker-plugin-lto.md]**. lld's ThinLTO backend caches each module's result under the LLVM ThinLTO cache key when given a cache dir (`--thinlto-cache-dir`, `-Wl,--thinlto-cache-dir=` via `-Clink-arg`) **[I: lld option known, not re-read at a pinned version]**. rust-lld matches rustc's LLVM by construction **[I]**.
- So with no driver, the expensive release backend is cached per module, with imports included in the key. Shikumi points the cache dir at a scratch dir and syncs entries to the store by content.
- The limit: pre-link optimization and lowering still run per crate in rustc, and the final ThinLTO thin-link step runs every time.
- **C1 spike:** a binary with two crates, `-Clinker-plugin-lto -Clto=thin -Clinker=clang -Clink-arg=-fuse-ld=lld -Clink-arg=-Wl,--thinlto-cache-dir=…`, or rust-lld directly. Build, edit one function, rebuild, and count the new cache entries (expect a handful). Check that the binary behaves the same as a plain `-Clto=thin` build.

---

## 5. Frontend, codegen and link: where a one-function edit's time goes

- **Does red-green skip type-checking unchanged items?** Yes, per query. Typeck, borrowck and `optimized_mir` are per-item queries, and unchanged items are marked green and loaded from the cache. Kobzol's ripgrep incremental debug rebuild shows borrow checking "disappeared" from the profile **[V excerpt]**. Parsing, expansion, name resolution and HIR lowering still run every time, and metadata, the dep graph and the query cache are fully re-encoded each session (internals thread 13500, and RFC 3881 discussion) **[V excerpt]**. `-Zincremental-ignore-spans` exists but is `[TRACKED]` and documented as "used for testing" (`options.rs` 2611) **[V]**. Don't build on it.
- **Measured splits** (published, not on Shikumi's workloads):
  - Kobzol (2024): after a small edit to the ripgrep binary in debug incremental mode, "the linker suddenly becomes a much larger bottleneck". For ~90 libraries, the frontend takes the larger fraction, and for `check` it's mostly frontend **[V excerpt]**.
  - Lattimore (2024), a small binary after switching to a fast linker and dropping debug info: 939 ms total = 454 ms link + 411 ms `codegen_crate` + 74 ms other **[V excerpt]**.
  - Nethercote (2023), rust#111666: dropping incremental CGUs from 256 to 64-128 made small-change rebuilds **4-10× slower** in some cases. So CGU granularity directly drives edit cost, which argues for one CGU per module (or finer) in A and B **[V excerpt]**.
  - 2025 compiler performance survey: 55% of respondents wait more than 10 s per rebuild. The named bottlenecks are workspace cascades, linking, and single-crate incremental speed **[V excerpt]**.
  - **[U] for Shikumi's real crates.** Measure with `cargo build --timings` (stable, splits frontend from codegen per unit) and `-Zself-profile` (nightly).
- **`-Zthreads`:** the 2026 goal is stabilization (Petrochenkov). Per a 2026-10 report, it's being switched on by default on nightly (rust#162848) **[V excerpt]**. A parallel frontend makes the dep-graph file order scheduling-dependent (serialized.rs) **[V]**. That's harmless under avenue A's "accelerator" stance. Its design axiom is output "consistent with single thread" **[V excerpt]**. Codegen output determinism under `-Zthreads` is **[U]**, so add `-Zthreads=8` to spike B1.

---

## 6. Linking (the fixed cost after any edit)

- **lld** is the default on x86_64-linux now, or about to be (survey post) **[V excerpt]**. **mold**'s author doesn't intend to add incremental linking **[V excerpt]**. **wild** is the fastest non-incremental linker and has an incremental design (2024-11: diff objects, then apply minimal edits with padded sections, falling back on strict-order sections and archive semantics). As of 0.6.0 (2025-09) incremental is postponed ("won't promise when"), and 0.10 (2026-08) still has none. Its README says incremental "isn't yet implemented" **[V excerpt]**. "Incremental, efficient linking with Wild" is on the 2026 Fast Builds roadmap **[V excerpt]**.
- **Things Shikumi can do now (STABLE):** use lld or wild, skip linking debug info in the dev loop (Lattimore's 7.5 s → 1.6 s) **[V excerpt]**, and **prelink the dependency closure as a dylib**, so the per-edit link only covers the workspace's own objects. Subsecond's "ThinLink" does this ("automatic dynamic linking to dependencies", under 500 ms incremental builds) **[V docs excerpt]**. In Shikumi, the dep dylib is a store path that doesn't change across edits **[I]**.
- Buck2's fake-linker extraction (earlier report §2.6) gives Shikumi the object list, so the link is its own derivation and can be swapped for an incremental linker later.

---

## 7. Prior art for sub-crate or shared Rust caching

| Project | What it does | Sub-crate? |
|---|---|---|
| rustc incremental | Query red-green plus per-CGU work products, in one local dir | Yes, locally only |
| Buck2 | `incremental_enabled` actions are local-only, never uploaded to the cache **[V earlier]** | Local only |
| sccache | Refuses incremental, per-invocation cache **[V earlier]**. The cache key includes the absolute cwd, so `--remap-path-prefix` breaks hits (howardjohn 2026) **[V excerpt]** | No |
| Cargo cross-workspace cache (goal #626, cargo#5931, #17453) | CAS store with per-unit entry pointers, prototyped 2026-09 **[V excerpt]** | No, per unit (crate) |
| kache | CAS per-crate artifacts restored by hardlink across worktrees **[V excerpt]** | No |
| **Dioxus subsecond / ThinLink, ipbp** | A `RUSTC_WRAPPER` captures args, `dx` is the linker proxy, it diffs object files to find changed functions, and it hot-patches through a jump table. Runs incremental rustc without a final link. It mostly patches the "tip" crate **[V docs and deepwiki excerpt]** | **Yes, per function at link and patch level.** It's the closest existing sub-crate tool, but the focus is the dev loop, not caching. |
| rustc_codegen_cranelift | Uses the shared ssa driver with dep-graph tasks, so it gets incremental CGU reuse. JIT mode compiles functions in memory (`driver/jit.rs`) **[V]**. The component is listed but `available = false` for x86_64-linux on stable 1.99.1 (manifest) **[V]**, so it's nightly only | CGU-level, same as LLVM |
| RDR (rust#158844, PR #156940 draft, MCP #790) | Public API hash so dependents relink instead of rebuilding. Spans move out of the hash, and private, non-inline, non-generic bodies are excluded **[V excerpt]** | Cross-crate cutoff. It's what makes A and B pay off in multi-crate graphs |
| Incremental Systems Rethought (goal #641, RFC 3881) | Shares incremental bases between `check`, `build` and `clippy`. bjorn3 notes the dir contents are "entirely unstable" **[V excerpt]** | Local; no distributed design |
| cargo-hakari | Workspace-hack crate that pins feature unification | No. It only stabilizes per-crate identity |
| Meta, Google, Microsoft | Nothing public found that does distributed sub-crate Rust caching **[U, absence]** | — |

Nobody publicly ships "remote incremental", meaning shared, seeded incremental state for Rust. The pieces that prove it's coherent are rustc's own session protocol (copy from the newest finalized dir, every artifact validated) and the Cargo CAS design **[I]**.

---

## 8. If outside isn't enough: minimal rustc patches (last resort)

Each row names the smallest outside alternative first.

| Need | Outside alternative (toolchain) | Minimal patch, if the alternative proves insufficient |
|---|---|---|
| Deterministic rlib members under incremental | Re-archive canonically in Shikumi (STABLE) | Use the work-product name (`{cgu}.{ext}`) as the archive member name, keeping the random suffix only on the temp path. Touches `ArArchiveBuilder::add_file` callers in `back/link.rs`. Small. |
| Stable CGU names without incremental | Driver override rename (STABLE+BOOTSTRAP) | Let `-Zhuman-readable-cgu-names` (or a new flag) keep module-derived names in the non-incremental branch of `merge_codegen_units` (`partitioning.rs` 471-507). Small. |
| Per-CGU content key without lowering | Hash the unoptimized bitcode in a wrapper backend (STABLE+BOOTSTRAP) | Emit each CGU's `CompileCodegenUnit` dep-node fingerprint, the combined hash of its recorded reads, as a JSON artifact. Needs incremental's dep tracking enabled; medium. |
| Read-only base session | Reflink or copy the base into scratch (STABLE) | `-Zincremental-base=<dir>`: read old artifacts from a dir with no lock and no GC, write the new session elsewhere. Touches `prepare_session_directory`, `find_source_directory`, GC; small to medium. |
| Reproducible snapshots | Not needed under the accelerator stance | Canonicalize the dep graph and query cache on save (sort by fingerprint). Medium, with a perf cost. |
| Clean wrapping of the LLVM backend | Unsafe ZST construction (STABLE+BOOTSTRAP) | `impl LlvmCodegenBackend { pub fn new_concrete() -> Self }`. One line. |
| Frontend reuse without incremental | **None.** The outside alternative is avenue A. | Not a small patch. It would be a second incremental system. |

---

## 9. Recommended order of work

1. **B1** (driver partition rename plus bitcode-hash dump). It's a day-sized proof of the foundation. If CGU bitcode is stable across runs and paths, and a one-function edit touches 1-2 CGU hashes, then sub-crate codegen caching from outside rustc is real.
2. **A1 + A2** (seeded incremental is equivalent to fresh, and relocatable). That proves frontend reuse can come from the store on an unmodified stable rustc.
3. **B3 / A3** on a representative large crate, to put numbers on what each saves (frontend vs lowering vs LLVM vs link).
4. **C1** if release LTO builds matter.
5. Linking: lld or wild now, plus a prelinked dep dylib for the dev loop. Track wild's incremental work and RDR.

Expected shape **[I]**: in a debug edit loop, A skips frontend work for unchanged items and LLVM work for unchanged CGUs, and the per-edit floor becomes parse, expand and resolve, plus metadata re-encode and link. In release, B or C caches each unchanged module's LLVM work across machines. Neither needs a patched compiler, and only B needs a per-toolchain driver build.
