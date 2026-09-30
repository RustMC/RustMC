//! Data-driven vanilla-style terrain generation, implemented independently.
//!
//! Numeric semantics of the noise core were established under the ADR-0014
//! (as amended) knowledge-consultation policy by observing vanilla 26.3
//! runtime behavior; the parity vectors asserted in tests are recorded as
//! facts in `docs/PROVENANCE.md`. Mojang world-generation data files are
//! operator-provisioned at runtime and are never committed to this repository.

pub mod aquifer;
pub mod density;
pub mod generator;
pub mod noise;
pub mod random;
pub mod worldgen;
