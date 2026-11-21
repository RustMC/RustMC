//! Unit tests for `vanilla::feature`.

use super::*;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

mod part1;
use part1::*;
mod part2;
use part2::*;
mod part3;
use part3::*;
mod part4;
mod part5;
