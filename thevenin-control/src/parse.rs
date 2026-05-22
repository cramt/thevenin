//! Re-export of the `.control` block parser.
//!
//! The canonical home for the parser is [`cirq_ir::control`]. This module
//! re-exports so existing `crate::parse::parse_control_block` callers
//! continue to compile.

pub use cirq_ir::control::{parse_control_block, parse_spice_number};

/// Back-compat alias for the previously `pub(crate)` SPICE-number helper
/// that `exec.rs` reaches through.
pub(crate) fn parse_spice_number_pub(s: &str) -> Result<f64, String> {
    parse_spice_number(s)
}
