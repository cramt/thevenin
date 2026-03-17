//! Port and parameter type definitions for XSPICE code models.

/// Direction of a code model port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDirection {
    /// Input-only: reads from solution vector, no stamp.
    In,
    /// Output-only: stamps into matrix/RHS.
    Out,
    /// Bidirectional: reads and stamps (e.g., conductance port).
    InOut,
}

/// Electrical type of a code model port, determining MNA behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortType {
    /// Single-ended voltage port.
    /// - In: read V(node) from solution
    /// - Out: branch equation (like voltage source)
    Voltage,
    /// Differential voltage port (two nodes: pos, neg).
    /// - In: read V(n+) - V(n-)
    DifferentialVoltage,
    /// Current port.
    /// - In: zero-volt vsource branch to measure current
    /// - Out: stamp current into RHS at pos/neg nodes
    Current,
    /// Conductance port (InOut only).
    /// Read voltage, stamp Norton companion: g_eq and i_eq.
    Conductance,
    /// Resistance port (InOut only, CCVS-like).
    /// Read current via branch, stamp voltage via branch equation.
    Resistance,
}

/// Definition of a single port in a code model.
#[derive(Debug, Clone)]
pub struct PortDef {
    /// Port name (e.g., "in", "out").
    pub name: String,
    /// Port direction.
    pub direction: PortDirection,
    /// Port electrical type.
    pub port_type: PortType,
}

/// Type of a code model parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    Real,
    Integer,
    Boolean,
    String,
}

/// A resolved parameter value.
#[derive(Debug, Clone)]
pub enum ParamValue {
    Real(f64),
    Integer(i64),
    Boolean(bool),
    String(String),
}

impl ParamValue {
    /// Get as f64, returning the default if not a real.
    pub fn as_real(&self) -> Option<f64> {
        match self {
            ParamValue::Real(v) => Some(*v),
            _ => None,
        }
    }

    /// Get as i64.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            ParamValue::Integer(v) => Some(*v),
            _ => None,
        }
    }

    /// Get as bool.
    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            ParamValue::Boolean(v) => Some(*v),
            _ => None,
        }
    }

    /// Get as string ref.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ParamValue::String(v) => Some(v),
            _ => None,
        }
    }
}

/// Definition of a code model parameter with its default value.
#[derive(Debug, Clone)]
pub struct ParamDef {
    /// Parameter name.
    pub name: String,
    /// Parameter type.
    pub param_type: ParamType,
    /// Default value.
    pub default: ParamValue,
}
