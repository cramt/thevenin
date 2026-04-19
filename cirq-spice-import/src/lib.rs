//! SPICE import — converts `thevenin_types::Netlist` into `cirq_ir::Circuit`.
//!
//! This crate provides the bridge from legacy SPICE netlists into the canonical
//! Cirq IR. It enables gradual migration: existing SPICE files can be imported
//! into the Cirq toolchain without manual rewriting.
