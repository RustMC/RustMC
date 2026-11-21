//! Persistence and crash-recovery unit tests for authoritative world journal and checkpoints.

use super::*;
use std::fs::OpenOptions;
use std::io::Write;

mod part1;
use part1::*;
mod part2;
