//! what the core does with a unit's outputs once they're built, over either store transport:
//! re-addressing them as content-addressed sources, which early cutoff rests on, and rooting the
//! live ones so the daemon's garbage collector keeps them.

#[cfg(test)]
mod memory;
mod readdress;
mod roots;

pub use readdress::{ReaddressError, Readdressed, output_address, readdress};
pub use roots::{add_manifest, root_manifest};
