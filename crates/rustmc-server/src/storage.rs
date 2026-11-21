//! World storage on disk: the NBT format and Anvil region files.
//!
//! Why: the server saves chunks and players in the format vanilla tools and
//! servers use, and the measurement tools read vanilla saves with the same
//! code.
//! Done: NBT reading and writing, region reading. Not done: see submodules.

pub mod nbt;
pub mod region;
