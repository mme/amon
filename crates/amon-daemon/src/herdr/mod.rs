//! Agents living inside herdr sessions, adopted without wrapping herdr.
//!
//! See docs/research/herdr-live-integration.md for the shape of the whole
//! thing and the decisions this implements.

pub mod proc;
pub mod project;
mod supervisor;
pub mod wire;

pub use supervisor::spawn;
