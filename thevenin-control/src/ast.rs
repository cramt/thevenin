//! Re-export of the control-flow AST.
//!
//! The canonical home for these types is [`cirq_ir::control`] so the IR can
//! carry the parsed form alongside the verbatim source. This module re-exports
//! them so existing `crate::ast::Statement` imports continue to compile.

pub use cirq_ir::control::{AlterValue, EchoFragment, Statement, StopCondition};
