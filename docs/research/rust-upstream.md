# Rust upstream: interface-level cutoff, metadata, and codegen granularity

Research date: 2026-11-07. Sources are rust-lang GitHub (read via the GitHub API in this session), goals.rust-lang.org, compiler-team MCPs, and the Rust blog. Zulip threads are cited only where a GitHub page links them; I didn't read Zulip directly.

Tags:

- **[V]** means I read it in a primary source during this session.
- **[U]** means it's inferred or unconfirmed. The reason is given each time.

Usability tags, per the constraint that Shikumi doesn't ship a patched rustc:

- **STABLE**: usable on an unmodified stable toolchain.
- **NIGHTLY**: usable on an unmodified, pinned nightly.
- **PATCH**: needs unmerged compiler code today. Shikumi can only get it by helping it land upstream.

## TL;DR

- Relink Don't Rebuild (RDR) is real and active, but none of its hashing has merged into rustc. The working implementation is a draft PR (rust-lang/rust#156940) behind `-Zpublic-api-hash`. A reviewer, petrochenkov, was assigned on 2026-11-06. The 2026 goal targets "nightly, with limitations" in 2026 and stabilization prep in 2027. **PATCH** today.
- The RDR output is a **public API hash stored in the crate header of `.rmeta`**, not a byte-stable `.rmeta`. The `.rmeta` file still changes on private edits, because it carries a separate private hash and other data. A content-addressed system can use it for early cutoff, but only by keying on the extracted hash, not on the file bytes. The tracking issue plans `-Zls=public_hash` for reading it.
- Something useful has already landed on nightly. rust-lang/rust#154724 (merged 2026-10-01) makes the SVH/crate hash a hash of the encoded metadata bytes instead of HIR. A run of determinism fixes landed in Sept–Oct 2026 (#162809, #164507). Together they move `.rmeta` toward "same encoded content means same bytes". It doesn't make `.rmeta` insensitive to spans or source-file changes. **NIGHTLY** now, reaching stable over the next one or two releases **[U]**.
- Cargo-side early cutoff is only at the design stage (cargo#17439, cargo#17626, both opened Sept–Oct 2026). `-Zchecksum-freshness` covers source inputs only and doesn't help a Nix-based system.
- No active proposal exists for out-of-process or distributed codegen inside rustc. The multi-process idea (rust#47518) was closed in 2023 in favour of the parallel frontend.

---

## 1. Relink Don't Rebuild

### History and owners

| Date | Item | State |
|---|---|---|
| 2024-09-26 | cargo#14604 "Downstream dependencies of a crate are rebuilt despite the changes not being public-facing" (osiewicz, Zed). Has a prototype that emits a public API hash in the "metadata finished" notification. | open, labelled blocked on compiler [V] |
| 2024-09-30 | compiler-team MCP #790 "Relink, don't rebuild" (osiewicz) | `major-change-accepted`, closed 2025-06-12 [V] |
| 2025-06-30 | rust#143249 "RDR: avoid rebuilding dependent crates after comment changes" (strips span/source-map data from rmeta, breaks diagnostics) | open, stale since 2025-12 [V] |
| 2025-07 to 2025-09 | 2025H2 flagship goal (contact Jane Lusby; owners @dropbear32, @osiewicz; champions Weihang Lo for cargo, Oli Scherer for compiler), rust-lang/goals#400 | closed; the 2026-05-18 blog lists it as "Not completed" [V] |
| 2025-11-21 | yaahc links a Zulip goal review explaining the lack of progress | [V] |
| 2026-01-17 | rust#151265 (davidbarsky): `-Zstable-crate-hash`, spans split into a separate `.spans` file, SVH limited to items reachable from the public API, sorted by DefPathHash | draft, open, idle since 2026-04-02 [V] |
| 2026-04-27 | rust#155871 (susitsm) "baseline, sound implementation" | closed 2026-09-16, superseded [V] |
| 2026-05-25 | **rust#156940** (susitsm) "improve public api hashing using an rmeta reachability graph", +3870/−721, 71 files | **draft, open, S-waiting-on-author; petrochenkov assigned 2026-11-06** [V] |
| 2026-07-06 | **rust#158844** tracking issue. Unstable option is `public_api_hash`. | open [V] |
| 2026-10-30 | rust-lang/goals#722 merged: 2026 goal "Relink Don't Rebuild" (contact @susitsm / Matyas Susits; compiler champion Vadim Petrochenkov; cargo support level "Small") | merged [V] |
| 2026-11-05 | rust-lang/goals#843 goal tracking issue | open [V] |

The Fast Builds roadmap lists two goals: "RDR – Land in nightly, with limitations (2026)" and "RDR – Prepare for stabilization (2027)". The funding ask is $70k, and the roadmap shows the funding status in red [V].

### Design (rust#156940, rust#158844)

- **The public hash is derived from rmeta itself.** As metadata is encoded, each `id -> data` table write records edges from `id` to every id inside `data`. That builds a reachability graph from root nodes, the crate root plus a few global tables. The graph's SCCs are computed, and each SCC's public hash is the local hash of its contents combined with the hashes of the SCCs it depends on. Anything unreachable from the roots counts as private. The design is meant to be future-proof: new metadata fields join the hash automatically, which answers the maintainability objection in the MCP [V].
- **Per-item public hashes are stored in rmeta.** Each reachable `LocalDefId` gets one. A crate's public hash then depends only on the *reachable* parts of its dependencies' APIs, plus their global parts such as lang items. This gives **transitive cutoff**: a change to a public item of `A` that `B` doesn't re-export doesn't change `B`'s public hash, so recompilation stops two crates downstream [V].
- **The crate header carries both a public and a private hash.** Dependency loading was the soundness problem. susitsm wrote on 2026-05-04 that "the resolver uses the crate hash of the dependencies saved inside the metadata to load transitive dependencies. Leaving something out of this hash is unsound". The fix is to look up dependencies by their public hash, and to error out if two candidates share a public hash but have different private hashes [V].
- **Spans stay in the public hash.** Editing the body of a public function moves its span end, and that changes the public hash. Private items added after the public ones, or in separate files, don't. The goal explicitly accepts that this rewards a "header style" where public functions are thin wrappers around private bodies. Span insensitivity is still an open question. One idea is an option that ignores diagnostic spans when debuginfo is off, which would make `cargo check` span-insensitive [V].
- **Known limitations of the current implementation** [V]:
  - Private data of dependencies is *removed* from rmeta when RDR is on, so diagnostics get worse.
  - Upstream monomorphizations (share-generics) are disabled.
  - v0 symbol mangling panics if any dependency uses `public_api_hash`, because the disambiguator is a per-crate global index.
  - Overhead is about 3% on a non-incremental build and about 25% on a no-op incremental rebuild.
  - The `impl PublicTrait for PrivateStruct` case is still hashed.
  - About 20 metadata fields haven't been reviewed.
  - Cargo integration isn't done.

### What has landed in rustc

None of the RDR hashing has landed [V]. The groundwork that has merged, all by susitsm unless noted [V]:

- rust#156092 (merged 2026-05-05) renames `needs_crate_hash` to `needs_hir_hash`.
- rust#164507 (merged 2026-10-21) encodes HashMap/HashSet in a deterministic order wherever the encoder requires determinism.
- rust#164636 (merged 2026-10-23) removes the `record_*` macros from the metadata encoder. This is a refactor toward the reachability graph's encoder callbacks.
- rust#165602 (opened 2026-11-07, open) skips the `reachable_set` calculation during metadata encoding when it isn't used.

### Would it give Shikumi an early-cutoff key?

Yes, but as a **hash, not as a byte-stable artifact** [V for the design; U for the exact extraction interface]. When the private hash changes, the rmeta bytes still change. The `public_api_hash` lives in the crate header. The planned interface for reading it is `-Zls=public_hash`, a listed task that hasn't been implemented. Today `rustc -Zls=root` already prints the crate hash, as used in rust#164379.

For Nix this matters. A content-addressed (CA) derivation cuts off on output bytes. If Shikumi feeds `foo.rmeta` into the derivation that compiles `bar`, every private edit to `foo` still re-runs `bar`. To get RDR cutoff, Shikumi has to:

1. Make an **interface stamp** (the public hash, extracted after compiling `foo`) the thing that keys `bar`'s compile.
2. Treat the actual `foo.rmeta` as a build-time input that doesn't change the key, the same role as Bazel's unused-inputs list.
3. Link the final binary against the *latest* `foo.rlib`.

RDR's soundness argument covers this. The downstream artifact stays valid against any upstream rmeta with the same public hash, because the resolver looks dependencies up by public hash. Plain Nix input-addressing can't express "an input that doesn't affect the key", so this needs Shikumi's own memo table (inputs plus upstream public hashes mapped to a store path) above Nix [U, design inference].

Cargo-internal or general? The MCP says it's for "cargo (and other build systems)". Nothing in the design is cargo-specific, apart from the stated reliance on `-Cmetadata` being unique per crate instance [V].

Usability: **PATCH** today. **NIGHTLY** once the split PRs land, a 2026 target the goal's own red status puts at risk. **STABLE** no earlier than 2027 [V for the stated plan; U for whether it's met].

---

## 2. Related efforts

| Effort | Primary source | State and date | What it gives a CA build | Shikumi use |
|---|---|---|---|---|
| **Crate hash computed from metadata bytes** | rust#154724 (Daniel-B-Smith, Google), implements rust#94878 | merged 2026-10-01. The flag `-Zmetadata-crate-hash=no` reverts it. A trimmed HIR hash is kept for bins and dylibs without metadata. [V] | The SVH no longer covers HIR bodies directly. If an edit doesn't change any encoded metadata, the rmeta can come out byte-identical. **Caveat:** encoded spans are byte positions, and rmeta encodes source-file info, so most edits to a file still change it. I haven't measured this. Probe it before relying on it. [U] | **NIGHTLY** now. The reporter of rust#164379 says it's in 1.100, and the 1.100 beta showed it. Stable 1.100 is around mid-Nov 2026 by the six-week cadence [U]. |
| **rmeta determinism fixes** | rust#162809 "Remap local def ids for deterministic metadata encoding" (merged 2026-10-17); rust#164507 (merged 2026-10-21). Fixed issues: #162202 (parallel frontend async fns), #163878 (generics_of under `-Zthreads`), #164379 ("Crate metadata and crate hash depend on the host CPU", x86_64 vs aarch64 host, P-medium). Earlier reports: #151868 and #152150 (Bazel remote-cache misses from rmeta ordering, Jan–Feb 2026). | merged [V] | Cross-machine, cross-host-arch rmeta reproducibility, which a shared Nix binary cache needs. After #154724, host-dependent rmeta also meant a different crate hash, which cascaded. | **NIGHTLY** now. Stable in about 1.101 [U, cadence]. |
| **`-Zembed-metadata=no`** (renamed from `-Zno-embed-metadata`); MCP compiler-team#851 `-Zsplit-metadata` (Kobzol) | MCP accepted 2025-03-28; cargo tracking cargo#15495; cargo#17267 "Enable `-Zembed-metadata=no` by default on nightly Cargo" merged 2026-08-17; std dogfoods it (rust#145343, merged 2026-01-13) | nightly [V] | The rlib holds only a metadata stub, and `.rmeta` becomes the single source of metadata. Downstream links need both, passed through `--extern`. That gives two separately addressed outputs with no duplication. Open question: is this a breaking change (cargo#17359)? | **NIGHTLY**. |
| **`-Zchecksum-freshness`** (cargo) plus `-Zchecksum-hash-algorithm` (rustc, rust#126930) | cargo#14136, opened 2024-06-25, open. Unresolved: build-script inputs, how to stabilize. | nightly [V] | Cargo uses source content hashes instead of mtimes. It covers *source inputs only*, not outputs, and it doesn't propagate cutoff. Nix already content-addresses sources, so it adds nothing for Shikumi. The 2025H2 RDR goal paired it with a less over-sensitive rmeta. | Not needed. |
| **Cargo output checksums / early cutoff** | cargo#17626 "Build output artifact checksum freshness" (ranger-ross, 2026-10-23); cargo#17439 "Support for early cutoff" (2026-09-04) | design discussion [V] | epage notes that early cutoff relies on rustc being bit-for-bit reproducible, which proc macros and build scripts can break. ranger-ross worries it conflicts with pipelining. bjorn3 replied that pipelined dependents need only the rmeta checksum. | Cargo-only. It's useful evidence that upstream is aligning on output-based cutoff. |
| **Don't track `--emit` in the SVH** | compiler-team MCP #769 (chbaker0, Chromium) | accepted 2024-09-16 [V]; whether the implementing PR merged is unverified [U] | The rmeta from `--emit=metadata,link` and from a later `--emit=link` invocation can match. MCP #769 also notes that `--emit=metadata` alone omits MIR, so that rmeta can't build rlib dependents. | Probably **STABLE** if it landed [U]. |
| **`-Zno-codegen` "hollow rlib"** | jsgf (Meta, Buck2) in bazelbuild/rules_rust#228: Buck2 pipelining uses `--emit link -Zno-codegen` so the metadata-only artifact has the same crate hash as the full build. He asks for stabilization. | still nightly; compiletest PR rust#154993 (2026-04) still exercises it; ICE rust#149156 open [V] | A metadata-producing action that's separate from codegen and hash-compatible with it. That's the Bazel/Buck way to pipeline without cargo's in-process JSON notification. | **NIGHTLY**. |
| **`-Zmetadata-link`** | rust#85793 (2021) | historical; not checked further [U] | Same goal as `-Zno-codegen`. | — |
| **`-Zno-link` / `-Zlink-only`** | rust#141706 "-Zno-link is broken (take 2)", open since 2025-05-28; compiler-team MCP #908 (bjorn3) moves LTO to `link_binary` to fix it, accepted 2025-09-15 | broken on nightly [V] | Would let codegen and link be separate actions, with link the only step needing all rlibs. | **NIGHTLY**, once fixed. |
| **Dependency formats in metadata** | rust#96708 (bjorn3, 2022): remove them from crate metadata, needed for bazelbuild/rules_rust#228 | open [V] | Removes a source of rmeta difference between compiles. | Upstream contribution candidate. |
| **"Late metadata"/digest file in rlib** | compiler-team MCP #853 (petrochenkov) | accepted 2025-03 [V]; implementation not checked [U] | Moves link-time-only data such as native libs out of early metadata, which makes rmeta less sensitive. | Watch. |
| **MIR-only rlibs** | compiler-team MCP #713 (accepted 2024-02); rust#119017 | PR closed unmerged 2025-01-17 [V] | Would put all codegen into the final crate. That's the opposite of per-CGU caching, but it's interface-friendly. | **PATCH**. |
| **Path normalization** | `--remap-path-scope` stable in rustc 1.95 (rust#147611, quoted in cargo#17488); cargo `profile.trim-paths` stabilized by cargo#17488 (merged 2026-10-06); rustdoc part rust#163895 open; `{cwd}` placeholder MCP #998 closed 2026-11-05 for lack of traction | [V] | Path-independent outputs. Nix's fixed `/build` sandbox path mostly removes the need. | **STABLE**. |
| **Proc-macro sandboxing / wasm proc macros** | MCP #475 build-time sandboxing (accepted 2022); MCP #876 (has-concerns, closed 2026-08-06); MCP #1017 "Wasm proc macro support" (bjorn3, accepted 2026-08-02); rust#160981 partial support (merged 2026-08-21); rust#157709 wasmi implementation (open). MCP #1017 mentions `-Zcache-proc-macros`. | [V] | Deterministic, cacheable macro expansion, which output-based cutoff needs. Watt (dtolnay) is the out-of-tree precedent, not checked in this session [U]. | **NIGHTLY**, partial. |
| **Distributed ThinLTO** | MCP #735 `--emit=thin-link-bitcode` (Google, accepted 2024-05); follow-up PRs rust#164038 and #164174 open (Oct 2026) | [V] | Lets Bazel/Buck2 distribute the ThinLTO backend the way they do for C++. It's the nearest thing to distributed codegen that upstream has accepted. | **NIGHTLY** (`-Z`/unstable). |
| **Parallel/scoped fat LTO** | MCP #1023 | withdrawn the same day, 2026-07-23 [V] | — | — |
| **Incremental Systems Rethought** | 2026 goal, rust-lang/goals#641 | accepted [V] | Shares incremental state between check, build and test. It's still per-crate, and incremental state is an impure cache. | Not applicable to hermetic Nix actions. |
| **Cargo cross-workspace cache** | 2026 goal, rust-lang/goals#626 (Ross Sullivan) | accepted [V] | Cargo-side unit cache. | Cargo-only. |
| **build-std** | 2026 goal, rust-lang/goals#274; RFC 3873 accepted, RFCs 3874 and 3875 pending | [V] | Relevant if Shikumi wants std built as ordinary Nix derivations with custom flags. Still cargo-nightly (`-Zbuild-std`). | **NIGHTLY**. Shikumi can drive rustc on `rust-src` itself without cargo [U]. |
| **Crate slicing** | Fast Builds roadmap "Future"; goals PR rust-lang/goals#438 "Slicing Crates for Faster Fresh Build" (closed 2026-01-30) | [V] | Would compile only the used part of dependencies. The roadmap says it "would require rearchitecting the compiler". | — |

**Deterministic CGU partitioning or exposing codegen work products:** I found **no MCP, issue or goal** proposing either, after searching compiler-team and rust-lang/rust titles for partitioning, codegen units and work products [V for the absence in search results; the search could have missed something]. CGU partitioning and incremental work products are internal to rustc's incremental cache, keyed by session state. The only flag-level relevant items are `-Ccodegen-units` (stable) and the 2020 CGU partitioning meeting (compiler-team#281, closed 2023).

---

## 3. Out-of-process, distributed or server-mode rustc

- **rust#47518** "Parallelize rustc via multi-process approach" (michaelwoerister, 2018). It proposed a deterministic partition of a crate into "vertical slices" compiled by separate rustc processes and then stitched together. Closed 2023-11-08 by nnethercote with the note "With the parallel front-end just being shipped to nightly, I think we can close this." **No successor proposal** [V].
- **Parallel frontend.** MCP #1005 sets out the stabilization strategy (accepted 2026-08-17). It's threads in one process, not separate processes [V].
- **No rustc server or daemon proposal** in rust-lang. The pipelining working group explicitly picked in-process JSON artifact notification over running rustc twice, and noted that this is "significantly more difficult for other build systems" (pipelining WG notes) [V]. RFC 1317 (RLS) described a long-running compiler daemon and called keeping the compiler in memory a "significant refactoring". It's historical [V].
- **Build-system workarounds:** bazelbuild/rules_rust#412 (persistent worker that reuses `-Cincremental` dirs) and nikhilm/rustc-worker. Both state that rustc has no "service mode" [V].
- **Third-party:** DioxusLabs `rustd`, a compiler daemon that keeps rust-analyzer analysis in memory, compiles changed functions and links them into a rustc-built program, with function-level content-hash caching and S3 sharing. It says it's pinned to 1.99.0, a prototype, and that "a fuzzer found twenty-one problems, most of them miscompiles" [V, from its own page; not a rust-lang source]. It's the closest existing precedent to sub-crate CAS caching. It works outside rustc and doesn't need a patched toolchain, but it isn't trustworthy for correctness yet.

---

## 4. How Shikumi can help, and the risk of depending on nightly

### Contribution targets, in priority order

1. **rust#158844 split-PR series.** The tracking issue lists about 19 PRs to carve out of #156940. Some are marked as small and reviewable without RDR knowledge: span hashing without parents, missing `StableHash` derives, encoding DefIndex as DefPathHash under the flag, `SyntaxContext` in `HygieneEncodeContext`, removing stripped cfg items, the sanity checks, and `-Zls=public_hash`. The blocker is **reviewer bandwidth**. susitsm wrote on 2026-08-12 that it's "currently blocked on reviewer bandwidth" [V]. Petrochenkov is now the dedicated reviewer. Useful contributions from Shikumi:
   - Test cases using the new `rustc_public_hash_changed` / `rustc_public_hash_unchanged` attributes.
   - An out-of-tree validation harness that rebuilds real workspaces with and without the flag and diffs the outputs, which would show miscompiles if any happen.
   - Reviewing the unreviewed metadata-field list.
   - The `-Zls=public_hash` printer, which is exactly the extraction interface Shikumi needs.
2. **v0 mangling disambiguator rework.** It's an open, unowned prerequisite (rust#158844, "Unresolved questions"). v0 became the nightly default per the 2025-11-20 blog post "Switching to Rust's own mangling scheme on nightly", which I didn't open [U]. RDR can't ship until this is solved.
3. **Span insensitivity for check/no-debuginfo builds.** This is an open design question. Solving it is what would make a private body edit inside a file with public items cut off.
4. **rmeta reproducibility.** Shikumi can find bugs cheaply: build each crate twice, on different hosts and with `-Zthreads>1`, compare rmeta bytes, and file the differences the way rust#164379 and #163878 were filed. Each fix lands in unmodified rustc.
5. **rust#141706 (`-Zno-link` / `-Zlink-only`)** and stabilizing `-Zno-codegen`. These give separate metadata, codegen and link actions with matching hashes.
6. **cargo#17439 / #17626.** These aren't needed by Shikumi, but a non-cargo consumer of the public hash would strengthen the argument for a stable, build-system-neutral interface.

### Timeline and stability risk

- Plan: nightly with limitations in 2026, stabilization prep in 2027 [V]. Realistically, the flag probably reaches nightly in 2027. The 2025H2 goal produced nothing that merged, 2026 started with a 3.8k-line draft, and the funding status is red [U, judgement].
- Even on nightly, the flag changes rmeta semantics. It removes private items, disables share-generics, and is incompatible with v0 mangling. Its hash definition will keep changing as edges are pruned. Shikumi should treat the public hash as **valid only for one exact toolchain hash**, and fold the toolchain store path into every key. Nix input addressing does that already [U, recommendation].
- `RUSTC_BOOTSTRAP=1` makes `-Z` flags work on a stable toolchain (rust#164379 uses it with `-Zls`). The toolchain is unmodified, but it's unsupported. Using it for a production mode would be a policy decision, not a technical one [V for the usage; U for advice].

### Is a pinned nightly via rust-overlay or fenix reasonable?

Yes, for an opt-in mode. Both projects are maintained: their latest pushes were on 2026-11-07, with about 1.6k and 1.1k stars [V].

- **rust-overlay** exposes dated nightlies (`rust-bin.nightly."YYYY-MM-DD".default`) and `fromRustupToolchainFile`. It warns against `nightly.latest` because components go missing on some days, and recommends `selectLatestNightlyWith` [V].
- **fenix** offers `toolchainOf { date; sha256 }` and `fromToolchainFile`, and needs a manifest sha256 in pure evaluation [V].

Both pin the exact upstream binaries, so they count as unmodified toolchains. Recommendations [U]:

- The default mode stays on stable. It uses whole-rmeta content addressing, which improves as the #154724 and determinism fixes reach stable.
- An "interface cutoff" mode requires a pinned nightly date, which Shikumi bumps deliberately, with a probe suite that checks the hash's behaviour on each bump.
- The RDR mode stays gated until #156940's pieces merge. Until then it would need a patched rustc, which is ruled out.

### Summary by effort

| Effort | Stable | Unmodified nightly | Patch only |
|---|---|---|---|
| Public API hash (RDR) | — | 2026–27 target | **yes, today** |
| Metadata-derived crate hash (#154724) | ~1.100 [U] | **yes** | — |
| rmeta determinism fixes | ~1.101 [U] | **yes** | — |
| `-Zembed-metadata=no` | — | **yes** | — |
| `-Zno-codegen` hollow rlib | — | **yes** | — |
| `-Zno-link` / `-Zlink-only` | — | broken (#141706) | — |
| checksum-freshness | — | yes (irrelevant to Nix) | — |
| `--remap-path-scope` / trim-paths | **yes** | yes | — |
| thin-link-bitcode | — | yes | — |
| Wasm proc macros | — | partial | — |
| MIR-only rlibs | — | — | yes (PR closed) |
| Per-CGU artifacts / out-of-process codegen | — | — | no proposal exists |
