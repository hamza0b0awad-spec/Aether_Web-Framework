//! # Aether Router Crate
//!
//! Segment trie with bitmask method matching and zero-copy parameter extraction.

pub mod node;
pub mod params;
pub mod trie;

pub use node::{HandlerId, NodeType, RouteNode};
pub use params::{Param, PathParams};
pub use trie::{MatchResult, RadixRouter, RegisteredRoute};
