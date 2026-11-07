# Shikumi docs

This is the front door to Shikumi's docs: every page, by title and first sentence. Start with the
philosophy for the laws, the capability page for what Shikumi provides, and contributing to start
working.

## Rules for working here

- [Shikumi design philosophy](philosophy.md): These are Shikumi's laws.
- [Contributing](contributing.md): Everyone who changes this repository reads this page, people and
  agents both.
- [Writing docs](writing.md): This guide covers every doc in `docs/`, whoever the reader is, and the
  comments in the code.
- [Placing docs](placing.md): Where a doc goes depends on its page type.
- [Maintaining docs](maintaining.md): A doc stays true because the change that makes it stale also
  updates it.
- [Working as an agent](ai/working.md): These rules cover how an agent works in this repository.

## Reference

- [Shikumi capabilities](reference/capabilities.md): Shikumi is a build system and project registry
  for multi-language repositories, with the Nix store as its cache.
- [Nix style](reference/nix-style.md): How Nix in this repository is designed: values before syntax,
  kernels and stages, laziness, the module system, layout and list order.
- [Rust style](reference/rust-style.md): How Rust in this repository is written: the lints the gate
  holds, manifests, errors, contracts, types, unsafe code, concurrency and tests.
- [Architecture pages](reference/architecture-pages.md): How to write and keep an architecture page,
  such as Shikumi's `explanation/architecture.md`.
- [Capability pages](reference/capability-pages.md): How to write and keep a capability page, such as
  Shikumi's `reference/capabilities.md`.

## Explanation

- [Shikumi architecture](explanation/architecture.md): This page says how Shikumi is put together: who
  owns what, how a build and a shell entry flow, and which invariants hold.
