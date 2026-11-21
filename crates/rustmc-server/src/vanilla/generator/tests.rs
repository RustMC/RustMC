//! Unit tests for `vanilla::generator`.

use super::*;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

mod part1;
use part1::*;
mod part2;
use part2::*;
mod part3;
