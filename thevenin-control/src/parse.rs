//! Re-export of the `.control` block parser.
//!
//! The canonical home for the parser is [`cirq_ir::control`]. This module
//! re-exports so existing `crate::parse::parse_control_block` callers
//! continue to compile.

pub use cirq_ir::control::parse_control_block;
