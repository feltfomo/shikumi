//! the registry document, `shikumi.registry`: its contract and the refusal every consumer shares;
//! the serialized declarations it's produced from; the checker that judges them; producing it from
//! a repository through the host's evaluator, from the context cache's store copies; the toolchain
//! context; and the read-only viewer the `shikumi` CLI registers as its `registry` subcommand.

pub mod check;
mod context;
mod contract;
pub mod declarations;
pub mod document;
pub mod evaluation;
mod failure;
mod graph;
pub mod layer;
pub mod produce;
pub mod reference;
pub mod render;
pub mod toolchain;
pub mod view;
pub mod vocabulary;

pub use contract::NonEmpty;
pub use shikumi_contract::{ContractError, render_json};
