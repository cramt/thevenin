//! Resolved XSPICE instance with matrix indices and per-instance state.

use std::any::Any;
use std::cell::RefCell;

use crate::types::ParamValue;

/// A resolved connection for a single port of an XSPICE instance.
#[derive(Debug, Clone)]
pub struct PortConnection {
    /// Index into the code model's port definition list.
    pub port_def_index: usize,
    /// Positive node matrix index (None = ground).
    pub pos_idx: Option<usize>,
    /// Negative node matrix index (None = ground), used for differential ports.
    pub neg_idx: Option<usize>,
    /// Branch equation index in the solution vector, used for voltage-out and
    /// current-in ports that need a branch variable.
    pub branch_idx: Option<usize>,
}

/// A resolved XSPICE code model instance, ready for simulation.
pub struct XspiceInstance {
    /// Instance name (e.g., "A1").
    pub name: String,
    /// Model type name (uppercase, e.g., "D_GAIN").
    pub model_type: String,
    /// Resolved port connections with matrix indices.
    pub port_connections: Vec<PortConnection>,
    /// Resolved parameter values (in definition order).
    pub params: Vec<ParamValue>,
    /// Mutable per-instance state (wrapped in RefCell for interior mutability).
    pub state: RefCell<Box<dyn Any>>,
    /// Branch indices allocated for this instance (voltage-out / current-in ports).
    pub branch_indices: Vec<usize>,
}

impl std::fmt::Debug for XspiceInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XspiceInstance")
            .field("name", &self.name)
            .field("model_type", &self.model_type)
            .field("port_connections", &self.port_connections)
            .field("params", &self.params)
            .field("branch_indices", &self.branch_indices)
            .finish_non_exhaustive()
    }
}
