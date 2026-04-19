//! Cirq frontend pipeline — orchestrates parsing, lowering, and IR generation.
//!
//! This crate provides the high-level entry points for processing Cirq source
//! files into either Cirq IR (for tooling) or `thevenin_types::Netlist` (for
//! simulation).

pub mod diagnostics;
