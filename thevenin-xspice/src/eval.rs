//! Evaluation interface for XSPICE code models.
//!
//! Code models receive `CmInputs` and return `CmOutputs` containing port
//! values and partial derivatives for MNA stamping.

use crate::types::ParamValue;

/// Analysis mode passed to code model evaluation.
#[derive(Debug, Clone, Copy)]
pub enum AnalysisMode {
    /// DC operating point.
    DcOp,
    /// DC sweep at a given sweep value.
    DcSweep { sweep_value: f64 },
    /// Transient analysis at a given time and timestep.
    Transient { time: f64, dt: f64 },
}

/// Inputs provided to a code model's evaluate function.
pub struct CmInputs<'a> {
    /// Port values read from the current solution vector, indexed by port
    /// definition order. For voltage ports this is voltage; for current ports
    /// this is current through the measurement branch.
    pub port_values: &'a [f64],
    /// Resolved parameter values, indexed by parameter definition order.
    pub params: &'a [ParamValue],
    /// Current analysis mode.
    pub mode: AnalysisMode,
}

/// A single port output value from evaluation.
#[derive(Debug, Clone)]
pub struct PortOutput {
    /// Index into the port definition list.
    pub port_index: usize,
    /// Output value: current for current-out ports, voltage for voltage-out
    /// ports, Norton equivalent current for conductance ports.
    pub value: f64,
}

/// A partial derivative of one output port with respect to one input port.
#[derive(Debug, Clone)]
pub struct PartialDerivative {
    /// Index of the output port.
    pub output_port: usize,
    /// Index of the input port.
    pub input_port: usize,
    /// Value of d(output)/d(input).
    pub value: f64,
}

/// Outputs returned by a code model evaluation.
#[derive(Debug, Clone)]
pub struct CmOutputs {
    /// Port output values.
    pub port_outputs: Vec<PortOutput>,
    /// Partial derivatives for Newton-Raphson Jacobian entries.
    pub partials: Vec<PartialDerivative>,
}

impl CmOutputs {
    /// Create empty outputs.
    pub fn new() -> Self {
        Self {
            port_outputs: Vec::new(),
            partials: Vec::new(),
        }
    }

    /// Add a port output.
    pub fn set_output(&mut self, port_index: usize, value: f64) {
        self.port_outputs.push(PortOutput { port_index, value });
    }

    /// Add a partial derivative.
    pub fn set_partial(&mut self, output_port: usize, input_port: usize, value: f64) {
        self.partials.push(PartialDerivative {
            output_port,
            input_port,
            value,
        });
    }
}

impl Default for CmOutputs {
    fn default() -> Self {
        Self::new()
    }
}
