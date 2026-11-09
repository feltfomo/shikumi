//! the registry document, `shikumi.registry`: its contract and the refusal every consumer shares;
//! the serialized declarations it's produced from; the checker that judges them; producing it from
//! a repository through the host's evaluator; and the read-only viewer the `shikumi` CLI registers
//! as its `registry` subcommand.

pub mod check;
mod contract;
pub mod declarations;
pub mod document;
mod graph;
pub mod produce;
pub mod reference;
pub mod render;
pub mod view;
pub mod vocabulary;

pub use contract::{ContractError, NonEmpty, render_json};
