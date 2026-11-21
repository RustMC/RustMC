//! Data-driven feature registry: the placement stage that runs after the
//! surface and carving passes.
//!
//! The registry is read from the operator-provisioned data pack exactly as
//! the carver registry is — one entry per pack element, so its size is
//! bounded by the pack rather than by the world. Identifiers the runtime
//! cannot model are kept as explicit `Unsupported` positions instead of
//! being dropped: the decoration loop numbers each placed feature within
//! its generation step, and the number is part of that feature's random
//! identity, so the numbering must stay stable whether or not the runtime
//! models the feature itself.
//!
//! That number — the ordinal — is not the position in the biome's own list.
//! The reference runtime derives one ordering per generation step from the
//! whole biome source before it decorates anything, and a chunk runs the
//! ordinals its 3x3 biome union contributes to a step, ascending. Both the
//! order and the union are reproduced here from the pack's own data; see
//! `docs/PROVENANCE.md` session 13 for the rules and their derivation.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::sync::OnceLock;

use serde_json::Value;

use super::plant_support;

use crate::vanilla::random::DecorationRandom;
use crate::vanilla::worldgen::{WorldgenError, read_dir_optional, read_json, stem, walk_json};
use crate::world::Direction;

/// One block-replacement rule test, as the data pack writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleTest {
    /// `minecraft:tag_match` — the block belongs to the named tag.
    Tag { tag: String },
    /// `minecraft:block_match` — exactly one block.
    Block { block: String },
    /// `minecraft:matching_blocks` — one of the listed blocks.
    Blocks { blocks: Vec<String> },
    /// `minecraft:height_match` — the absolute row lies in the range.
    Height {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    /// `minecraft:any_of` — at least one nested test passes.
    AnyOf(Vec<RuleTest>),
    /// `minecraft:all_of` — every nested test passes.
    AllOf(Vec<RuleTest>),
    /// `minecraft:not` — the nested test fails.
    Not(Box<RuleTest>),
    /// A test this runtime does not model. The name is kept so the caller
    /// can refuse the feature instead of guessing at its meaning.
    Unsupported { kind: String },
}

impl RuleTest {
    /// Whether this test and every nested test can be evaluated here.
    pub fn is_modelable(&self) -> bool {
        match self {
            Self::Unsupported { .. } => false,
            Self::AnyOf(rules) | Self::AllOf(rules) => rules.iter().all(Self::is_modelable),
            Self::Not(rule) => rule.is_modelable(),
            _ => true,
        }
    }

    /// Evaluates the test against the block at a candidate position.
    /// `block` is `None` for a position that holds no block at all.
    pub fn test(&self, block: Option<&str>, y: i32, tags: &BlockTags) -> bool {
        match self {
            Self::Tag { tag } => block.is_some_and(|block| tags.contains(tag, block)),
            Self::Block { block: want } => block == Some(want.as_str()),
            Self::Blocks { blocks } => {
                block.is_some_and(|block| blocks.iter().any(|candidate| candidate == block))
            }
            Self::Height {
                min_inclusive,
                max_inclusive,
            } => (*min_inclusive..=*max_inclusive).contains(&y),
            Self::AnyOf(rules) => rules.iter().any(|rule| rule.test(block, y, tags)),
            Self::AllOf(rules) => rules.iter().all(|rule| rule.test(block, y, tags)),
            Self::Not(rule) => !rule.test(block, y, tags),
            // Never reached for a modelable feature; refusing to place is
            // the conservative reading, and placement checks
            // `is_modelable` before it gets here.
            Self::Unsupported { .. } => false,
        }
    }
}

mod tags;
pub use tags::*;
mod state;
pub use state::*;
mod configured;
pub use configured::*;
mod range;
pub use range::*;
mod placement;
pub use placement::*;
mod registry;
pub use registry::*;
mod resolve;
use resolve::*;
mod parse;
use parse::*;
mod unmodeled;
use unmodeled::*;
mod parse_placement;
use parse_placement::*;
mod place;
pub use place::*;
mod run;

#[cfg(test)]
mod tests;
