//! CirQ circuit format: a unified intermediate representation for electronic circuits.
//!
//! This crate provides:
//! - A fully-resolved IR where implicit information (net domains, ground node, etc.)
//!   is made explicit
//! - A SPICE netlist → IR lowering path (via `thevenin-types`)
//! - A CirQ YAML/JSON parser → IR path
//!
//! Simulation commands (`.OP`, `.TRAN`, etc.) from SPICE are ignored — this IR
//! represents circuit *structure* only.

pub mod cirq_parse;
pub mod from_spice;
pub mod ir;

pub use ir::*;
