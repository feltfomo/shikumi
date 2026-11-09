//! the registry document, `shikumi.registry`: its contract and the refusal every consumer shares,
//! producing it from a repository's declarations through the host's evaluator, and the read-only
//! viewer the `shikumi` CLI registers as its `registry` subcommand.

pub mod document;
pub mod produce;
pub mod view;
