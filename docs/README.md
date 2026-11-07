# Shikumi docs

- [Spec](spec.md): every capability Shikumi will provide, with its status.
- [Architecture](architecture.md): components, flows, invariants and the contracts other systems see.
- [Roadmap](roadmap.md): bands, splits and joins, rulings, environment, findings and risks.
- [Foundation effort](efforts/foundation/README.md): Band 0's page, phases and baton.

## Research

The design rests on these reports. Claims are tagged by how they were checked.

- [Rust pipeline](research/rust.md): rustc stage boundaries, `.rmeta`, codegen units, prior art.
- [Rust below the crate](research/rust-subcrate.md): store-seeded incremental, a codegen driver per CGU,
  linking.
- [Rust upstream](research/rust-upstream.md): the public-API hash, nightly flags, where Shikumi could
  contribute.
- [Nix substrate](research/nix.md): capability matrix across CppNix, Determinate and Lix, measured
  per-derivation overhead, early cutoff without CA derivations, locking, GC.
