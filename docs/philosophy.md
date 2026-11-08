# Shikumi design philosophy

These are Shikumi's laws. They decide what Shikumi may depend on, which Nix it runs on, where a
concern's code goes and what counts as proof that it works. The root AGENTS.md turns them into rules
for each change, and the [architecture](explanation/architecture.md) says how the system keeps them.
A rule that contradicts a law is wrong, or the law is, and either one changes only on purpose.

The goal behind all of them is a build system other repositories can adopt without trusting it
blindly: it runs on the Nix they already have, its cache never hands out an output it can't account
for, and it carries none of its first consumer's habits.

## Run on the Nix everyone has

Shikumi runs on CppNix, Determinate Nix and Lix. CppNix and Determinate Nix are primary, Lix is
supported, and all three have to work. Shikumi uses only what the three share: input-addressed and
fixed-output derivations, content-addressed store paths and the daemon worker protocol.

So no content-addressed derivations, dynamic derivations, recursive Nix or import-from-derivation.
Lix removed content-addressed derivations and the other two still mark them experimental, so a
feature built on them would hold for one implementation at best. Where Shikumi needs what they'd
give, such as early cutoff, it does the work itself on top of the shared subset.

The host's Nix is never changed to make Shikumi work. Shikumi runs as an untrusted daemon user, and
the two settings that need a machine's admin, trusting a Shikumi cache and trusting its signing key
on a remote builder, are detected and reported rather than assumed. Other implementations are tested
in VMs or separate stores, never by swapping the one on the host.

## The store is the cache, finer than the compiler's unit

Every cached output is a store path. Each language backend cuts its compiler's pipeline at the
stages where an output can stand on its own, and an edit rebuilds only the units it changed. The aim
is granularity below the compiler's coarsest unit: per crate is a starting point for Rust, not the
end state.

Shikumi never ships a patched compiler and never reimplements the Nix evaluator. It may use a
compiler's internals from outside, through an interface an unmodified toolchain offers, such as
`rustc_driver` or a codegen backend. The host's evaluator is the only one that reads a repository's
Nix.

## Nix declares, Rust scales

People write declarations in Nix, and the real Nix evaluator runs them. Anything that grows with the
size of the repository runs in Rust: discovering units, writing per-unit `.drv` files, scheduling,
cutoff and caching. Nix evaluation and the module system handle hundreds of project records well and
millions of compile units badly, so nothing per unit goes through them.

## Few dependencies, all of them owned

The Nix side depends on nixpkgs and nix-unit and nothing else: no flake frameworks, no effect
libraries, no other Nix library. The Rust side may use crates under the
[dependency rules](contributing.md#dependencies). Shikumi depends on no external run queue or lease
service. If it needs a lease, the concept is its own, and a repository plugs its provider in.

## Contracts are data with a schema

Every machine contract is JSON with a published schema, validated in Rust through serde. People
author declarations in Nix; everything a program hands another program is a versioned document a
third program can check. A contract changes by a version bump its schema records, never by a reader
learning to tolerate a new shape.

## One owner per concern

Each concern has one owning component. When a new component takes a concern over, the superseded
path is removed in the same change, so there's never a second authority to drift. The registry
document is the only source of project facts: shells, verbs, hooks, services and formatting each
read it rather than keeping their own copy.

When composition finds a conflict, such as two projects defining one toolchain setting or one
environment variable, it refuses and names both files. Choosing a winner is the owners' call, and a
composition that picks one has made that call for them.

## Nothing repository-specific

Shikumi has no vocabulary that belongs to one repository. A consuming repository fills Shikumi's
extension slots from its own declarations: its project kinds, its docs layout, its verbs' names, its
requirement tags, its lease provider. Repository-specific tools, such as a doc tool, a scratch policy
or a licence-header rule, are declarations a repository makes, not things Shikumi ships.

This repository is public. It never names, describes or vendors code from a private consumer, and the
gate's `private-names` check holds the names it can match.

## The cache never lies

An output in a shared cache was built by a sandboxed derivation from its declared inputs, and nothing
else. Shikumi may produce or reuse an output some faster way on a developer's machine, but such an
output is asserted, stays local and is never shared until a sandboxed build confirms its bytes. A
cached unit's output is reproducible, and each backend proves it by building twice and comparing.
[The architecture](explanation/architecture.md#provenance) has the mechanism.

## Speed is measured, not claimed

A claimed speedup is measured on the benchmark fixtures, the same way for every candidate and every
backend. The fixtures and the result record are maintained product, not a debugging aid, so two
measurements taken months apart still compare. A mechanism still being chosen is chosen by those
numbers.

## Docs are part of the system

Product docs say what Shikumi is, does and will do, and how to work in it. They ship in the same
change as the code that makes them stale. Plans, roadmaps and research live outside this repository,
and nothing in it depends on them. [Writing docs](writing.md) and [maintaining docs](maintaining.md)
hold the rest.

## Correctness first

None of the laws above loosens these.

Fix a problem in the component that owns it. A workaround placed wherever the symptom shows up hides
a broken invariant, even when the test passes. An invariant that's hard to keep is still kept, and
changing one is an explicit, reviewed change that states the old rule and the new one.

Get the algorithm and data layout right before optimizing, and measure before doing anything unusual.
Whether to take a library or write the code is [contributing's call](contributing.md#dependencies).
