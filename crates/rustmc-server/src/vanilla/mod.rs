//! Data-driven vanilla-style terrain generation, implemented independently.
//!
//! Numeric semantics of the noise core were established under the ADR-0014
//! (as amended) knowledge-consultation policy by observing vanilla 26.3
//! runtime behavior; the parity vectors asserted in tests are recorded as
//! facts in `docs/PROVENANCE.md`. Mojang world-generation data files are
//! operator-provisioned at runtime and are never committed to this repository.
//!
//! The generator and aquifer memos are keyed by world coordinates, so their
//! entry count would otherwise grow with the volume of world queried; they
//! live in `vanilla::cache`, a hand-rolled fixed-capacity two-queue FIFO
//! map, not a dependency. Each owner sizes it from its access pattern, and
//! since every value is a pure function of coordinates plus the seed,
//! eviction only ever costs a recomputation. The remaining caches in this
//! subsystem (`density::NoiseEngine` stacks, `density::DensityRegistry`
//! compiled functions, `carver::CarverData` registries) are keyed by
//! registry id instead, so they hold one entry per entry of the operator's
//! data pack and are bounded by the pack, not by the world.

pub mod aquifer;
pub mod biome;
pub mod cache;
pub mod carver;
pub mod density;
pub mod generator;
pub mod noise;
pub mod random;
pub mod surface;
pub mod worldgen;
