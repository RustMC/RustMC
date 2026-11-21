//! Deterministic wire tests for [`super`]: every assertion decodes the encoded
//! packet back off the byte stream, so a field-order or bit-width regression
//! fails here rather than in front of a real client.
//!
//! The fixtures use locally invented registry ids inside a 65,536-state and
//! 128-biome registry. Those sizes are chosen so the derived direct-mode
//! widths equal the ones the provisioned 26.3 registries produce (16 and 7),
//! which keeps the assertions meaningful without committing Mojang data.

use super::*;
use crate::java_wire::read_varint;

mod part1;
use part1::*;
mod part2;
