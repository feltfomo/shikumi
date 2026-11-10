# Shikumi capabilities

Shikumi is a build system and project registry for multi-language repositories, with the Nix store as
its cache. A repository declares its projects once, and Shikumi derives their builds, dev shells,
verbs, hooks, services and formatting from those declarations. Each language's compiler pipeline is
cut where an output can be cached on its own, so an edit rebuilds only the units it changed.

| Area                                                          | What a repository gets                                    | Status          |
| ------------------------------------------------------------- | --------------------------------------------------------- | --------------- |
| [Where it runs](#where-it-runs)                               | CppNix, Determinate Nix and Lix, on Linux (WSL 2 included) and macOS on Apple silicon | Accepted target |
| [Declarations](#declarations)                                 | projects that declare themselves, checked in one pass     | Current         |
| [Registry document](#registry-document)                       | one versioned JSON view of every project                  | Current         |
| [Packages and toolchains](#packages-overrides-and-toolchains) | one catalog, one owner per setting                        | Current         |
| [Lock](#lock)                                                 | one lock for toolchains and third-party artifacts         | Current for toolchains; third-party artifacts are a target |
| [Build graph](#build-graph)                                   | per-stage caching in the store, early cutoff, queries     | Accepted target |
| [Execution mechanism](#execution-mechanism)                   | how each unit actually runs                               | Open            |
| [Languages](#languages)                                       | C, Rust per crate, Kotlin and Java, Python and Nushell    | Accepted target |
| [Rust below the crate](#rust-below-the-crate)                 | caching finer than one crate                              | Open            |
| [IDE project models](#ide-project-models)                     | the model each IDE needs, from the build                  | Accepted target |
| [Steps and verbs](#steps-verbs-and-tasks)                     | derived tasks, cached steps and tests                     | Accepted target |
| [Shells](#shells)                                             | focused and global shells for bash, zsh and fish          | Accepted target; environment declarations current |
| [Services](#services)                                         | per-worktree services under Shikumi's own supervisor      | Accepted target |
| [Hooks](#hooks)                                               | per-project format and lint hooks, per worktree           | Accepted target |
| [Formatting](#formatting)                                     | one formatter per file, cache shared across worktrees     | Accepted target |
| [Formatting mechanism](#formatting-mechanism)                 | treefmt wrapped, or Shikumi's own                         | Open            |
| [Rust tests](#testing-shikumi-itself)                         | Shikumi's own Rust tested under nextest                   | Current         |
| [Store conformance](#store-conformance)                       | paths, transports and cutoff checked against all three    | Current         |
| [Nix tests](#testing-shikumi-itself)                          | nix-unit tests of the Nix side                            | Current         |
| [Benchmark fixtures](#benchmark-fixtures)                     | seeded fixtures, the Cargo baseline, the result record    | Current         |
| [Out of scope](#out-of-scope)                                 | what Shikumi deliberately doesn't do                      | Out of scope    |

## Where it runs

| Target                     | Role                                                    | Status          |
| -------------------------- | ------------------------------------------------------- | --------------- |
| CppNix and Determinate Nix | primary                                                 | Accepted target |
| Lix                        | supported; all three implementations have to work       | Accepted target |
| Linux, any distribution    | target platform                                         | Accepted target |
| macOS on Apple silicon     | target platform, unverified until a test machine exists | Accepted target |
| macOS on Intel             | not a platform                                          | Out of scope    |
| Native Windows             | not a platform; Windows runs Shikumi in WSL 2           | Out of scope    |

Shikumi uses only what the three Nix implementations share, runs as an untrusted daemon user and
never changes the host's Nix. Intel macOS is out because the pinned nixpkgs doesn't build for it,
and Shikumi's Nix side depends on that nixpkgs alone.

Windows users run Shikumi inside WSL 2, which is a real Linux kernel, so Nix and Shikumi run there as
on any Linux. WSL 2 isn't tested on its own, but what Shikumi relies on there, the kernel's
namespaces, the Nix daemon and the store, is what the Linux tests cover. Two things matter: keep the
repository in WSL's own filesystem, never under `/mnt/c`, whose file access is too slow for a build
system, and enable systemd in WSL, which Nix's multi-user daemon needs.

## Declarations

| Capability                                                        | Status          |
| ----------------------------------------------------------------- | --------------- |
| One manifest, one declaration file per project                    | Current         |
| Plain records through the `shikumi.project` factory               | Current         |
| Schema with required fields, closed enumerations, typed paths and globs | Current  |
| Checking in one pass, each finding with a code, path and source position | Current |
| Findings rendered against the user's file, or as JSON             | Current         |
| Evaluation failures as findings at the user's code, by kind       | Current         |
| Each project evaluated on its own, so one failure leaves the rest checked | Current |
| A failure only inside Shikumi's own Nix reported as a Shikumi bug | Current         |
| Extension namespace with the repository's own schema              | Current         |
| Module adapter for NixOS and Home Manager                         | Current         |
| Module adapter for flake-parts                                    | Current; unverified against flake-parts itself |
| Global and focused compositions                                   | Current         |
| The remainder as a scope                                          | Current         |
| The remainder's own format, lint and checks                       | Accepted target |
| Refusal at build, shell, verb, hook, service and query            | Accepted target |
| Typed parameters, sensitive inputs, confirmation, timeout, locks  | Accepted target |
| A generated command CLI with listing, planning and completions    | Accepted target |

A repository lists its projects in one manifest, `shikumi.nix` at its root, and each project has one
declaration, `project.nix`, in its directory. Nothing is discovered by scanning, so joining is a
reviewed change to the manifest. A declaration file returns `shikumi.project` applied to a plain
record, and it's called with whichever of `shikumi` and nixpkgs' `lib` it names.

```nix
# shikumi.nix
{ types }:
{
  projects = {
    core = "libs/core";
    web = "apps/web";
  };
  extensions.team = types.record { owner = types.required types.identifier; };
}
```

```nix
# apps/web/project.nix
{ shikumi, lib }:
let
  services = import ./services.nix;
in
shikumi.project (
  {
    project = {
      summary = "The web front end";
      maintainers = [ "web-team" ];
      dependsOn = [ "core" ];
    };
    steps.bundle = {
      description = "Bundle the site";
      exec = "bundle --release";
    };
    # the local server talks to the development database, so it's declared only beside it.
    commands = lib.optionalAttrs (services.services ? database) {
      serve = {
        description = "Serve the site locally";
        exec = "serve --port 8080";
      };
    };
    extensions.team.owner = "web";
  }
  // services
)
```

- **Declarations are plain records**: a flat set of small records, `project`, `packages`, `env`,
  `toolchains`, `steps`, `commands`, `services` and `extensions`, with a step's `after` as the one
  ordering edge. What a project writes
  is what Shikumi reads, with no merge order and no priorities. A large declaration splits across
  files with plain `import` and `//`, and a condition is `lib.optionalAttrs` or a list of entries.
  Composition across projects is the manifest.
- **Schema.** [The declarations reference](declarations.md) lists every field, type and finding code,
  generated from the vocabulary the checker checks with. A required field with no value is a
  finding, never an empty default, enumerations are closed, and paths and globs can't leave the
  repository. Text the document carries can't refer to a store path or contain the repository's
  location.
- **Checking in one pass.** Shikumi's checker, in Rust, checks every composed project at once against
  the declaration record and the repository's extension schemas, then across projects: a dependency
  names a listed project, a step runs after a step of its own project, neither dependencies nor steps
  loop, a command, service or toolchain name has one owner, every toolchain import names a toolchain
  its project defines, every package picked is in the catalog, no shell gets one variable twice or
  two packages under one name, every listed directory has a declaration, and no listed directory
  lies inside another.
  It returns every finding, each with a stable code, the field's path and its source position.
- **Source positions.** Every attribute keeps the file, line and column that defined it, through
  `import` and `//` too, relative to the repository root. A list element has no position of its own,
  and neither has an attribute that `builtins.mapAttrs` built or one whose position lies outside the
  repository, such as one nixpkgs' `genAttrs` built. Each of those is located at the nearest
  enclosing field that has one.
- **Readable findings.** `shikumi registry check` renders the findings against the user's own files,
  the way rustc does, or prints the document that carries them with `--format json`. Shikumi's Nix
  never throws on a declaration that Nix can still represent: a value of the wrong kind, such as a
  function or a path where text belongs, comes back as a finding.

```text
error[unknown-reference]: `project.dependsOn[1]` names `nowhere`, which isn't a listed project
 --> projects/dangling/project.nix:6:5
  |
6 |     dependsOn = [
  |     ^^^^^^^^^
  = help: list `nowhere` in the manifest, or take it out of `dependsOn`
```

- **Diagnostics as data.** The registry document carries the findings. One that carries any has no
  projects, and every consumer refuses it, naming every finding: the producer and the viewer share
  one refusal, and the table's refusal row lists the entry points that are targets.
- **Extension namespace.** A repository declares a schema per namespace in its manifest, written in
  `shikumi.lib.types`, and projects put metadata under `extensions.<namespace>`. Shikumi checks it
  against that schema and passes it to the document unchanged. A namespace without a schema is
  refused, and so is a schema built from anything but the public vocabulary.
- **The module adapter.** `shikumi.lib.modules` has a module each for NixOS, Home Manager and
  flake-parts whose `shikumi.projects.<name>` option takes a `shikumi.project` result and gives back
  the same value, with no merging and no schema of its own. flake-parts gets the option at the top
  level and per system. The NixOS and Home Manager modules are tested in those module systems. The
  flake-parts module is tested against a copy of flake-parts' own option declarations, not
  flake-parts itself, so it's unverified there.
- **Compositions.** The global composition holds every project plus the remainder. A focused
  composition holds one project, every project it depends on and every project whose toolchain it
  imports, transitively, and never reads any other declaration. Both read the whole catalog.
- **The remainder** is the scope of every source outside the listed directories.
- **Evaluation failures.** A declaration that Nix can't evaluate, because it doesn't parse, uses a
  variable nothing defines, calls something that isn't a function, throws, fails an assertion or
  holds text that isn't valid UTF-8, comes back as that project's finding. The finding names the
  kind, sits at the innermost place in the user's own files the evaluator's trace passed through,
  or at the project's file where the evaluator gives none, and lists the user's other places on
  the way as notes. Shikumi's own frames and nixpkgs' are
  left out. Text that isn't valid UTF-8 is located to its field. A failure Shikumi doesn't recognize
  keeps the evaluator's own message, and one whose frames lie only in Shikumi's Nix is reported as a
  bug in Shikumi, with those frames for the report. Each project is evaluated on its own terms, so
  every other project is still checked in the same run, its findings beside the failure, and a
  focused composition gives each of its projects what the global one gives. A manifest
  that fails to evaluate is the one finding, since every project's directory is its to say. The
  kind, the location and the user's frames are the same on CppNix, Determinate Nix and Lix; the
  evaluator's own wording isn't.

```text
error[thrown]: evaluation threw an error: summary is required
 --> projects/thrown/helpers.nix:2:49
  |
2 |   required = name: value: if value == null then throw "${name} is required" else value;
  |                                                 ^^^^^
note: reached from here
 --> projects/thrown/project.nix:7:15
```

People author declarations in Nix, and the real Nix evaluator runs them. `shikumi.lib` takes the
nixpkgs source tree from whoever imports it, runs on its `lib` and reads its package set for the
catalog, and the `shikumi` command evaluates with the nixpkgs the repository's [lock](#lock) pins.

Every evaluation runs in pure mode from a store copy of exactly the files it may read: the manifest,
and the `.nix` files Git tracks inside a listed project's directory or the catalog's. A declaration
that imports or reads anything else is refused, naming the file:

```text
error[not-keyed]: `app/untracked.nix` is read here, and evaluation may read only the repository's keyed files
 --> app/project.nix:5:15
  |
5 |     summary = import ./untracked.nix;
  |               ^^^^^^
  = help: keep what a declaration reads in a `.nix` file Git tracks, inside a listed project's or the catalog's directory, and not behind a symlink
```

A read that would tie the declarations to the machine or the moment, of a file outside the
repository, the clock, the host's system, a search-path lookup such as `<nixpkgs>` or a URL, is an
`impure` finding. What the evaluation came to is cached, so producing the document again reads
nothing but the stat of each of those files; [the context cache](../explanation/architecture.md#context-cache)
says how.

## Registry document

| Capability                                                    | Status          |
| ------------------------------------------------------------- | --------------- |
| `shikumi.registry` version 1.0, with a published schema       | Current         |
| No store paths, no command lines, identical across worktrees  | Current         |
| No builds or reads of build outputs while producing it        | Current         |
| No state written into the checkout                            | Current         |
| A read-only viewer                                            | Current         |
| Available as a store path to every step, command and service  | Accepted target |

- **The document** is JSON with a published schema,
  [`registry.schema.json`](../../crates/registry/schema/registry.schema.json), and a major and minor
  version. A reader takes only the version it was built for, and refuses an unknown field. The
  serialized declarations the checker reads, failures included, are a contract of their own, with
  the schema [`declarations.schema.json`](../../crates/registry/schema/declarations.schema.json),
  and so is each evaluation's answer, with
  [`evaluation.schema.json`](../../crates/registry/schema/evaluation.schema.json).
- **What it holds.** Each project's directory, summary, maintainers, dependencies, toolchain imports
  with the project each comes from, packages, environment variables, the toolchains it owns, steps,
  commands, services and extension metadata; the catalog's systems, its package names and each
  override's reason and release; the composition's focus, the remainder, and the diagnostics. A
  step's or command's command line and an override's build are declared and never copied in, and
  neither is a package: the document names packages by their catalog name and holds no store path.
- **Identical across worktrees.** Two worktrees of one commit, at any paths, with any untracked files, produce the same bytes,
  on every supported Nix, and every implementation serializes the same declarations and yields the
  same findings for a declaration that doesn't inspect the evaluator. One that asks which evaluator
  it runs on, through `builtins.nixVersion` or a test for a builtin, gets that evaluator's answer, as
  Nix gives it. A string that carries the repository's location or a store path is refused, source
  positions are relative to the repository root, and the evaluation sees no environment variables.
- **No builds.** A declaration that imports, reads, hashes or tests for a build output is refused,
  naming the derivation, whether or not it took the path's string context off, and so is one that
  runs a program through `builtins.exec`, whatever the caller's Nix configuration says.
- **Nothing outside the keyed files.** A declaration can't read a file Git doesn't track, `.git`, a
  file outside the repository, a file behind a symlink, a file that isn't `.nix`, the clock, the
  host's system or the search path. A directory listing of the repository reads the store copy,
  which is the same from every worktree of a commit, and text that holds the copy's location or its
  name is refused as the repository's location. A finding that quotes a store path, from an
  evaluator's error or a package's name, quotes it without its hash.
- **Only what it entered.** Each evaluation opens Shikumi's store under a database of its own that
  registers only its keyed copy, the layer and the locked nixpkgs, so naming another repository's
  copy, or an earlier one of its own, by its NAR hash or its path is an `impure` finding however
  full the store is, and a cached run and a fresh one agree. Every file a declaration's evaluation
  reaches, nixpkgs included and however it's reached, by `import`, `scopedImport`, the module system
  or `callPackage`, is evaluated without any builtin that reaches a fetcher: `getFlake`,
  `fetchTree`, `fetchGit`, `fetchTarball`, `fetchurl`, `fetchMercurial` and `fetchClosure`. On
  Determinate Nix, `getFlake`, and `findFile` with a `flake:` lookup-path entry, read any flake,
  commit or directory on the machine in pure and restricted mode. A lookup-path entry that names a
  scheme, given to `findFile` or bound as `__nixPath` for `<...>`, is refused the same way; the
  lookups nixpkgs makes itself, such as `<nix/fetchurl.nix>`, still resolve. Reaching any of them
  is an `impure` finding.
- **Lix and the copy's location.** Lix reads Shikumi's store at its real location, so a
  declaration's `./.` and its source positions are where the copy lies under this machine's cache
  directory, where CppNix and Determinate Nix give the store path (lix `libexpr/eval.cc`,
  `EvalPaths::toRealPath`). Text holding that location is refused everywhere; on Lix, every
  evaluation that misses the cache also runs again against a second copy of the store at another
  location, and a declaration whose answer differs between the two, because it built a value from
  where the copy lies, such as its length or a hash of it, is a `location` finding. CppNix and
  Determinate Nix evaluate once, and such a value is the same on every machine there, since the
  store path depends only on the copy's content.
- **Limit: a prefix of the cache directory on Lix.** A value built only from a prefix both copies'
  locations share, part of the cache directory's own path, is the same under both and isn't
  refused, so on Lix it can differ between users. Lix upstream owns what's missing: a way to
  evaluate a `local?root=` store at its logical store path, as CppNix and Determinate Nix do. The
  closing item is a request to Lix for that. Until then Shikumi refuses every location value that
  differs between the two copies, and passes one built from their shared prefix. This reopens when
  the request lands or is declined.
- **No state in the checkout.** Producing it writes nothing there.
- **The viewer** prints the project list, one project's detail, the document, its findings, the
  declarations as Shikumi read them, the toolchain context, the schemas and the declarations
  reference, run from the repository root or given `--root`. It refuses a document with a diagnostic the same way every
  consumer does, rendering each finding against the repository's files.

```fish
shikumi registry list
shikumi registry show web
shikumi registry document --focus web
shikumi registry check --format json
shikumi registry declarations
shikumi registry schema declarations
```

## Packages, overrides and toolchains

| Capability                                                                      | Status          |
| ------------------------------------------------------------------------------- | --------------- |
| One catalog naming every package a shell or step may use, picked by name        | Current         |
| A package outside the catalog refused                                           | Current         |
| Two packages under one name on one path, propagated inputs included, refused    | Current         |
| One overlay on the pinned nixpkgs, each entry with its reason and its release   | Current         |
| The catalog read for every system the manifest names                            | Current         |
| The pinned nixpkgs held in Shikumi's own lock                                   | Current         |
| One owning project per toolchain setting, imported by name                      | Current         |
| The catalog's packages resolved to store paths for the toolchain context        | Current         |
| Repository tools built as derivations, so entering a shell never compiles a tool | Accepted target |

A repository names its catalog in the manifest: a directory holding `catalog.nix`, and the systems
its package set is read for. The catalog is called with nixpkgs' `lib` and names each package by its
attribute path in the pinned nixpkgs, such as `jdk21` or `nodePackages.prettier`. Shikumi resolves
each path in every system's package set, with the catalog's own overlay applied.

```nix
# shikumi.nix
{
  projects = {
    core = "libs/core";
    web = "apps/web";
  };
  catalog = {
    directory = "catalog";
    systems = [
      "x86_64-linux"
      "aarch64-darwin"
    ];
  };
}
```

```nix
# catalog/catalog.nix
_: {
  packages = {
    jdk = "jdk21";
    kotlin = "kotlin";
    nodejs = "nodejs";
  };
  overrides.kotlin = {
    reason = "Kotlin runs on the JDK every JVM project builds with";
    release = "nixpkgs' own choice of JDK for Kotlin is the repository's";
    package = final: prev: prev.kotlin.override { jre = final.jdk21; };
  };
}
```

```nix
# libs/core/project.nix: owns the JDK toolchain
toolchains.jdk = {
  description = "The JDK every JVM project builds and runs with";
  packages = [ "jdk" ];
  env.JAVA_HOME.package = "jdk";
};

# apps/web/project.nix: uses it, and picks packages of its own
project.imports.jdk = "core";
packages = [ "kotlin" "nodejs" ];
env.NODE_ENV = "development";
```

- **Catalog.** Every package a shell or step may use is named once, under the catalog's `packages`,
  and projects and toolchains pick packages by that name, for their path or for a variable that
  points into one. A name the catalog doesn't hold is refused, and so is any pick in a repository
  without a catalog. The catalog is read once for each system the manifest names, each against that
  system's own package set, since a package can differ between them: nixpkgs' JDK on Apple silicon
  is a different build from Linux's. An entry whose path names no package on one of them, or a
  package that throws as it's read, as an alias nixpkgs has removed does, is refused at the entry,
  naming the system. A system nixpkgs doesn't know is refused at the manifest. A command line that
  names a store path is refused too, since it would run a package the catalog doesn't hold: a
  command runs what its shell's packages put on the path. A catalog that fails
  to evaluate is its own finding, and nothing is checked against it, so it doesn't turn into every
  project's finding.
- **Conflicts.** A shell's path holds the packages its sources pick, and everything those packages
  propagate. The global composition's shell holds every project and every toolchain; a focused
  one holds its project, the toolchains that project defines and those it imports. Two different
  packages on one path under one name are refused, naming both packages, both catalog entries, the
  propagated input that brought one of them there, the system, and both files that put them there.
  Two packages are different when their `name`, the `pname` and version nixpkgs writes, differs.
  They're under one name when one's `pname` or `meta.mainProgram`, the program it runs as, is the
  other's `pname` or `meta.mainProgram`. A clash between a project and a toolchain it uses is
  reported at the project, and a clash between two of either at the later one in name order. That's the check that catches a JDK 21 against a JDK 25, whichever of them
  arrives through another package's propagated inputs. Every propagation attribute is followed,
  `propagatedBuildInputs`, `propagatedNativeBuildInputs`, the four `deps*Propagated` and
  `propagatedUserEnvPkgs`, so a conflict is found however the shell takes its packages.
- **What the conflict rule can't see.** It reads what evaluation shows of a package, never what the
  package installs, since nothing may be built while declarations are read. So it misses two
  packages that put the same program on the path when neither names it as its `mainProgram` and
  their `pname`s differ, such as a `symlinkJoin` of a JDK under a name of its own; two builds that
  share one `name`, such as an override that keeps the version, where only the first one reached is
  followed; and a package whose build writes extra entries into its `nix-support/propagated-*`
  files beyond its attributes.
- **Overrides.** The catalog's `overrides` is the one overlay on the pinned nixpkgs, an entry per
  attribute it replaces or adds, each with `package = final: prev: …`, as in an overlay, `reason`,
  why it exists, and `release`, what lets it go. An entry missing either text is refused. The catalog
  never sees a package set, so a package that differs from nixpkgs' can only come from an entry: a
  catalog entry is an attribute path, with no `pkgs.extend` or `.override` to reach for. An overlay
  declared anywhere else is refused as a second overlay: an `overlays` field in the catalog, or
  `overrides` in a project's declaration.
- **The pinned nixpkgs** is the one the repository's [lock](#lock) pins. Moving it is
  `shikumi lock pin nixpkgs <tarball URL>`, which fetches the tarball and writes its hash.
- **Single-owner toolchain settings.** A setting such as the JDK or the Rust toolchain is defined
  under one project's `toolchains`, with a description, the packages it puts on the path and the
  variables it sets. Another project uses it through `project.imports`, naming the toolchain and the
  project that defines it, so a focused composition takes that project with it the way it takes a
  dependency. A second definition of one toolchain is refused naming both files, and so is an import
  of a toolchain its project doesn't define. A focused composition reads no declaration outside its
  closure, so it sees a second definition only there; the global composition sees every one.
- **Store paths.** Reading the catalog forces no package's `outPath` or `drvPath`, so it computes no
  store path unless an override's own code does. The document names each package by its catalog
  name and attribute path. The toolchain context resolves every catalog entry's package to its
  output's store path, for each system, without building it or reading anything a build writes, and
  isn't part of the document. A package nixpkgs refuses to evaluate, for its licence or a known
  vulnerability, has no path there. `shikumi registry context` prints it, as the
  [`toolchain-context.schema.json`](../../crates/registry/schema/toolchain-context.schema.json)
  record, version 1, which is unpublished while what its readers need is settled.
- **Repository tools** are built as derivations, so entering a shell never compiles a tool.

## Lock

| Capability                                                                | Status          |
| ------------------------------------------------------------------------- | --------------- |
| One lock per repository, Shikumi's own, at `shikumi.lock/`                | Current         |
| nixpkgs and nix-unit pinned by URL and NAR hash, fetched by hash          | Current         |
| JSON lines with a published schema, sorted, sharded by ecosystem and package | Current      |
| A missing or broken lock refused, naming the file and line                | Current         |
| Third-party artifacts, such as crates and Maven artifacts, in the same lock | Accepted target |
| Native lock data reused where it carries a usable hash                    | Accepted target |

```fish
shikumi lock pin nixpkgs https://github.com/NixOS/nixpkgs/archive/<revision>.tar.gz
git add shikumi.lock
```

```text
shikumi.lock/lock.json
shikumi.lock/toolchain/nixpkgs.jsonl
```

```json
{"ecosystem":"toolchain","name":"nixpkgs","url":"https://github.com/NixOS/nixpkgs/archive/<revision>.tar.gz","narHash":"sha256-…"}
```

- **One lock, sharded.** `lock.json` names the contract and its version, and every other file holds
  the entries of one package of one ecosystem, one JSON object per line, sorted. Two changes to
  different packages touch different files, so they merge without a conflict, and each line is one
  artifact grep finds. Each line has to be written exactly as Shikumi writes it. The schemas are
  [`lock.schema.json`](../../crates/lock/schema/lock.schema.json) for `lock.json` and
  [`lock-entry.schema.json`](../../crates/lock/schema/lock-entry.schema.json) for a line.
- **Toolchain sources.** nixpkgs and nix-unit are pinned as a tarball's URL and its unpacked tree's
  NAR hash, the hash a flake's lock writes as `narHash`, so the same tree pinned either way is the
  same store path. `shikumi lock pin` fetches the tarball with the host's `nix-prefetch-url` to hash
  it, or takes `--hash`. Producing the registry needs nixpkgs; Shikumi takes it from the host's
  store when that tree is there and fetches it by hash otherwise. Shikumi doesn't evaluate through
  flakes, which copy the whole source tree into the store on every evaluation miss.
- **Read as Git tracks it.** The lock is its tracked files, like everything else Shikumi reads, so a
  new shard is part of it once it's added to Git.
- **Third-party artifacts,** such as crates and Maven artifacts, are a target: content-addressed, one
  entry per artifact in shards of their ecosystem, each becoming a fixed-output derivation.
  `Cargo.lock` checksums are the sha256 of the `.crate` file, which an entry reuses. Where an
  ecosystem gives only sha1 or nothing, as Gradle does, Shikumi records sha256 and the URL itself.
- **One lock per repository.** No per-directory copies.

## Build graph

| Capability                                                 | Status          |
| ---------------------------------------------------------- | --------------- |
| Cut points per stage                                       | Accepted target |
| Early cutoff on all three implementations                  | Accepted target |
| Re-addressing, the mechanism early cutoff rests on         | Current         |
| Evaluation outside the per-edit loop                       | Accepted target |
| Third-party dependencies from the lock                     | Accepted target |
| Escape hatches for generated code and build-time plugins   | Accepted target |
| Non-redistributable outputs                                | Accepted target |
| Affected query, dependency queries, "why did this rebuild" | Accepted target |
| Build traces                                               | Accepted target |
| One rooted manifest of live outputs                        | Current         |
| Root generations per worktree, and eviction                | Accepted target |
| Remote cache and builders                                  | Accepted target |
| Provenance                                                 | Accepted target |

- **Cut points per stage.** Each language backend declares the stages of its pipeline where an output
  can be cached on its own, the unit of work at each stage, its declared inputs, and the
  reproducibility flags it needs. Stages stack.
- **Early cutoff.** A rebuilt unit whose output is byte-identical stops the rebuild there, on CppNix,
  Determinate Nix and Lix alike. Shikumi does this itself by re-adding outputs as content-addressed
  store paths, without content-addressed derivations.
- **Re-addressing** is current: given built outputs, Shikumi computes each one's content-addressed
  path from the NAR hash and references the daemon records, and sends bytes only for a path that
  isn't valid yet. An output that refers to its own store path is refused, naming the output. The
  conformance checks prove the cutoff end to end on every implementation; the scheduler that will
  drive it isn't built.
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
  eviction, since Nix GC has no notion of least recently used. One manifest and its indirect root are
  current, through the local daemon only: a root is a link on the daemon's machine, so over `ssh-ng`
  it's refused. Generations, the `.shikumi/roots/` layout and eviction are targets.
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

| Capability                                                                | Status          |
| ------------------------------------------------------------------------- | --------------- |
| Environment variables declared on projects and toolchains, each once      | Current         |
| Those variables reaching shells and steps                                 | Accepted target |
| Everything else below                                                     | Accepted target |

- **Activation replaces the path.** The caller's PATH doesn't leak in, and nothing is appended.
- **Environment variables** are declared under `env` on projects and toolchains: text, or where in
  a catalog package a variable points, `{ package = "jdk"; path = "lib"; }`, which the toolchain
  context resolves. A shell gets each variable from one place, so a second definition among a
  shell's sources is refused with both files named. A project's variable and a toolchain's variable
  of the same name are two definitions like any other: there's no precedence between them. The
  global composition's shell holds every source, so there each variable has one definition across
  the repository. `PATH` is refused: the shell sets it from its packages. The registry document
  carries every variable's name and value. They reach shells and steps alike,
  which is a target.
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
| Nix evaluation and module behavior tested in pure Nix with nix-unit and flake checks | Current         |
| Rust behavior tested with nextest                                                    | Current         |
| Store paths, transports and cutoff conformance-tested against all three, on Linux    | Current         |
| Benchmark fixtures                                                                   | Current         |

The gate's `nextest` check runs every workspace test, the checker's included. Its `declarations`
check runs the declaration layer's nix-unit tests against fixture repositories and in each module
system. The `registry-no-builds-*` checks, one per Nix implementation, hold the declarations each
implementation serializes from them to one tracked reference, and the checker's tests hold what
those declarations give: the document and the rendered findings. For a fixture whose declarations
fail to evaluate, each implementation's failures have to classify as one tracked reference, every
project evaluated beside the others on one core has to match it evaluated alone, and a fault
planted in a copy of the layer has to come back as a Shikumi bug. `registry-cli` and
`registry-worktrees` run the packaged `shikumi` on them, and `declarations-reference` holds the
generated reference page to the vocabulary, as [the root AGENTS.md](../../AGENTS.md#gates) lists.

### Store conformance

Status: current on Linux; unverified on macOS.

Every store path Shikumi computes has to equal, bit for bit, what each Nix implementation computes,
and both store transports, early cutoff and roots have to work against each one as a user the daemon
doesn't trust. The gate checks that against four implementations, each in its own NixOS VM:

| Implementation  | Version | Built from                                  | Worker protocol |
| --------------- | ------- | ------------------------------------------- | --------------- |
| CppNix          | 2.35.3  | the pinned nixpkgs                          | 1.37            |
| Determinate Nix | 3.24.2  | its source tag, with nixpkgs' Nix packaging | 1.37            |
| Lix             | 2.95.4  | the pinned nixpkgs                          | 1.35            |
| Lix             | 2.96.0  | the pinned nixpkgs                          | 1.35            |

In each VM, a user the daemon doesn't trust runs `shikumi-conformance` three times: through the Nix
CLI, through the worker protocol on the daemon's socket, and through `ssh-ng` to the same machine as
the same user. Each run computes paths with the derivation crate, sends them through its transport
and records its value beside the daemon's for every check:

- **`.drv` paths and the file's exact bytes**, for derivations with no inputs, input sources, input
  derivations two levels deep, several outputs, flat and recursive fixed outputs, a fixed output with
  inputs, inputs that share a hash modulo, and every string escape the `.drv` format has.
- **Output paths**, as a real build of each derivation reports them, and each output's NAR hash. One
  derivation is built alike in all three runs, and the VM test checks that their reports agree on it.
- **Content-addressed source paths**, with and without references, as the daemon registers them. A
  path computed wrong is refused, and an output that refers to itself is refused as a source.
- **NAR reads**: a built output's NAR, streamed back, hashes as the bytes its build wrote.
- **Early cutoff**: a unit rebuilt from a changed derivation with the same bytes re-addresses to the
  path it had, sending no NAR bytes, so its dependent's `.drv` is the one already there and its output
  is valid with nothing built. A changed output gives a new address and a dependent that has to build.
  An output that refers to itself is refused, naming it, and nothing is registered for it.
- **Cancellation**: a build whose future is dropped stops its builder, and its output never appears.
- **Roots**: a manifest rooted through an indirect root keeps itself and everything it names through
  a collection that takes an unrooted control added the same way. Over `ssh-ng` the root is refused.
- **Trust**: the daemon reports the user as untrusted, on the socket and on the far side of `ssh-ng`.

What each daemon answers a user it doesn't trust, from the runs on the socket and over `ssh-ng`:

| Operation                                                   | CppNix 2.35.3 and Determinate Nix 3.24.2 | Lix 2.95.4 and 2.96.0 |
| ----------------------------------------------------------- | ---------------------------------------- | --------------------- |
| Add a content-addressed path, with references or without   | accepted when the address reproduces     | the same              |
| Add a `.drv`, then build it                                 | accepted                                 | the same              |
| Build an input-addressed derivation it's sent whole         | refused, whether or not it's in the store | the same             |
| Register an indirect root                                   | accepted                                 | the same              |
| Collect garbage                                             | accepted                                 | the same              |
| Make a permanent root on the daemon's side                  | refused on the socket, accepted over `ssh-ng` | no such operation |

Shikumi relies on the first five, which are checks; the last two rows are notes. A permanent root
goes through over `ssh-ng` on CppNix and Determinate Nix because their `nix-daemon --stdio` runs
operations itself, as trusted, against the daemon behind it, which still judges each path it's sent.
Shikumi makes the link and registers an indirect root instead, the same on every implementation.

The harness exits non-zero when any value differs. `--perturb <case>` alters one case's computed
values, and `--remove readdressing` or `--remove root` leaves that step out, to show the checks fail.
The checks need KVM, so they don't run on macOS. The Determinate Nix build leaves out its wasm
builtins, Sentry and OpenTelemetry, none of which touch the store.

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
- **The Cargo baseline.** Plain Cargo builds a generated workspace in place, each build reusing the
  session the previous one left, through five scenarios: cold, no-op, a private function edit, a
  public signature edit, and an edit to a file in the hub's crate directory that rustc never reads,
  whose rebuilt output is byte-identical. It runs with incremental compilation on and off.
- **The result record.** One JSON line per measured build, in a published schema at version 2, so
  comparing a candidate with the baseline is a join on the record's fields. Each record carries its
  wall and CPU time, a bucket per kind of step the build paid for, and the outcome its scenario
  claims: units built and skipped against the fixture's model, bytes sent, the outputs' identity
  against a reference, and, for incremental units, how many started from a session and how much of
  it rustc reused. Scenarios cover the Rust fixture, the overhead fixture's per-unit overhead and
  latency floor, substitution, garbage collection and path registration.
- **Self-checking records.** A record doesn't parse unless its buckets cover every step its
  mechanism pays in its scenario, inside its timer, and its outcome matches the fixture's model.
  `validate` also regenerates each fixture from its seed and parameters and refuses a digest that
  doesn't match.

The record's schema and the overhead fixture's unit graph schema are tracked in `bench/schema/`.
[Contributing](../contributing.md#the-benchmark-harness) has the commands.

## Out of scope

| Excluded | Why |
| --- | --- |
| Shipping a patched compiler | Shikumi works on unmodified toolchains, using their internals only from outside |
| Reimplementing the Nix evaluator | the host's evaluator is the only one that reads a repository's Nix |
| Content-addressed derivations, dynamic derivations, recursive Nix, import-from-derivation | not shared by all three Nix implementations |
| Native Windows | Windows runs Shikumi inside WSL 2, as Linux |
| Repository-specific tools: doc tools, docs layouts, scratch policies, licence-header rules, leases | they're declarations a repository makes |
| Depending on an external run queue | a lease, if Shikumi needs one, is its own concept |
