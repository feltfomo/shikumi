//! store paths, NAR serialization and derivation synthesis, computed bit for bit as CppNix,
//! Determinate Nix and Lix compute them. this crate is the one place Shikumi computes a store
//! path; the store transport and the conformance harness take paths from it and never re-derive
//! one.

mod address;
mod base32;
mod derivation;
mod hash;
mod nar;
mod path;

pub use address::{ContentAddress, FixedHash, Ingestion, References, SelfReference, SourceAddress};
pub use derivation::{Derivation, DerivationError, OutputName, Outputs, StoreDerivation};
pub use hash::{HashParseError, Sha256};
pub use nar::{Nar, NarError, NarNode};
pub use path::{HASH_PART_LEN, PathError, StoreDir, StoreName, StorePath};
