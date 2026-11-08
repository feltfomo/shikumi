# Rust style

How Rust in this repository is written: the lints the gate holds, manifests, errors, contracts,
types, unsafe code, concurrency and tests. Read it with [the writing guide](../writing.md#code-comments),
which owns comments, and [contributing](../contributing.md#dependencies), which owns adding a crate.

Every rule here is enforced by the gate or followed by every crate in the tree. The gate settles
layout and lints; this page covers what it can't check.

## What the gate holds

[The root AGENTS.md's Gates section](../../AGENTS.md#gates) lists the `format`, `clippy` and `nextest`
checks. rustfmt runs with its defaults; the repository has no rustfmt config file. Clippy runs over
tests and binaries as well as libraries with warnings denied, so a lint set to `warn` fails the gate
as surely as one set to `deny`.

The root `Cargo.toml` sets these lints for the whole workspace:

| Lint                            | Level | Why it's on                                                   |
| ------------------------------- | ----- | ------------------------------------------------------------- |
| `unsafe_op_in_unsafe_fn`        | deny  | an unsafe operation needs its own block, even in an unsafe fn |
| `missing_debug_implementations` | warn  | every public type can be printed in a failure                 |
| `unreachable_pub`               | warn  | `pub` means reachable from outside the crate                  |
| `clippy::dbg_macro`             | deny  | no debugging leftovers                                        |
| `clippy::todo`                  | deny  | no unfinished code paths                                      |

Every crate opts in with `[lints] workspace = true`, and a new crate does the same. A finding is
fixed, not silenced: nothing in the tree carries an `#[allow]` or `#[expect]`.

## Workspace and manifests

- One Cargo workspace at the root, with resolver 3, edition 2024 and `rust-version = "1.88"`. A
  crate takes its edition, `rust-version`, licence and repository from `[workspace.package]`.
- Dependencies follow [contributing's rules](../contributing.md#dependencies). A crate never names
  its own version of a shared dependency.
- A crate that isn't for publishing says `publish = false` and has a one-line `description`.
- The gate builds with the toolchain nixpkgs pins, not with 1.88, so the declared minimum isn't
  checked. Don't rely on it as tested.

## Errors

- A library's errors are its own types: an enum or a newtype with a hand-written `Display` and an
  empty `impl std::error::Error`. The tree uses neither `anyhow` nor `thiserror`.
- An error carries the context a reader needs to act, such as the path that failed to open or the
  scenario that broke, in its message.
- A binary's `main` returns `ExitCode` and prints `<binary>: <message>` to stderr on failure.
- `expect` is for an invariant the code already guarantees, and its message states that invariant:
  `"a schema always serializes"`, not `"failed"`. `unwrap` appears outside tests only on writes to a
  `String`, which can't fail.
- A panic message in a test names what broke and how to fix it, when there's a fix to name.

## Contracts

Every machine contract is JSON with a published schema, validated through serde, as
[the philosophy](../philosophy.md#contracts-are-data-with-a-schema) says. In Rust that looks like
this:

- The schema is generated from the Rust types with schemars and tracked beside the code. A test
  compares the tracked file with the generated one, so the two can't drift.
- Every struct serde reads a contract into has `#[serde(deny_unknown_fields)]`. An unknown field is
  an error, not something to ignore.
- A field that doesn't apply is `null`, never an empty string or a zero. Newtypes such as `Text` and
  `Identifier` reject the sentinels a loose `String` would let through, with `try_from` in serde.
- A rule that spans fields lives in a `TryFrom<Raw…>` conversion: serde reads the raw shape, and the
  public type can only be built through the check.
- A contract records its version in the document, as a type that rejects any other version.

## Types and visibility

- Make invalid values unrepresentable where it's cheap: `NonZeroU32` for a count that can't be zero,
  a validated newtype for a constrained string.
- A binary crate's internal items are `pub(crate)`, since `unreachable_pub` flags a `pub` nothing
  outside can reach.
- Every public type derives or implements `Debug`, which the workspace lints hold. Most private
  types derive it too.

## Unsafe code

`unsafe` is allowed where it's the only way to reach the platform, such as `getrusage` through
`libc`. Each `unsafe` block is as small as the call it wraps, and carries a `// SAFETY:` comment that
says why every requirement of that call holds there.

## Concurrency

Independent work runs on every core, as `repo-check` does. Scoped threads from `std::thread::scope`,
sized by `std::thread::available_parallelism`, keep every thread's lifetime inside the call that
started it. Where output is a contract, it stays in input order whatever order the threads finish in.

A measurement is the exception: it runs alone, so it doesn't measure its neighbours. The Cargo
baseline measures from one thread, because its CPU time comes from `RUSAGE_CHILDREN`, which another
thread reaping a child would disturb.

Async code runs on Tokio, one runtime per process, as [the architecture](../explanation/architecture.md#the-core)
says. The store transport's operations are async, and the conformance harness drives them from one
current-thread runtime. Independent CLI invocations and daemon requests run as tasks in a `JoinSet`
with a bounded number in flight, owned by the call that started them. A child process is killed, and
a daemon connection closed, when the future that holds it is dropped; a connection goes back to its
pool only after an operation finished cleanly. Blocking file work in async code goes through
`tokio::fs` or `spawn_blocking`.

## Algorithms

When [contributing](../contributing.md#dependencies) says to write an algorithm yourself rather than
take a crate, say why in the item's doc comment, as `bench/src/rng.rs` does. Measure before doing
anything unusual for speed.

## Tests

- Unit tests live in a `#[cfg(test)] mod tests` at the bottom of the file they test.
- A test's name says the behavior it holds, as a phrase: `parallel_map_keeps_input_order`,
  `a_count_other_than_the_model_predicts_is_rejected`.
- Tests that need a directory use `tempfile`, never a fixed path, so tests can run side by side.
- A test pins a reference value when the value is a contract, such as the generator's output for a
  known seed, and says in a comment what changing it changes.
- `unwrap` is fine in a test; a panic there is the failure report.

## Layout

- Imports come in groups separated by a blank line: `std`, then external crates, then the crate's
  own. rustfmt sorts within a group.
