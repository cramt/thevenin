//! MNA stamping logic for XSPICE code model instances.
//!
//! Given evaluation outputs (port values and partial derivatives), stamps the
//! Norton companion model into the MNA matrix and RHS vector.

use crate::eval::CmOutputs;
use crate::instance::XspiceInstance;
use crate::types::{PortDef, PortDirection, PortType};

/// Stamp matrix entry at (row, col) += value, skipping ground (None).
fn stamp_matrix<M: MatrixStamp>(
    matrix: &mut M,
    row: Option<usize>,
    col: Option<usize>,
    value: f64,
) {
    if let (Some(r), Some(c)) = (row, col) {
        matrix.add(r, c, value);
    }
}

/// Stamp current into RHS: subtract from pos, add to neg.
fn stamp_rhs_current(rhs: &mut [f64], pos: Option<usize>, neg: Option<usize>, current: f64) {
    if let Some(i) = pos {
        rhs[i] -= current;
    }
    if let Some(j) = neg {
        rhs[j] += current;
    }
}

/// Trait abstracting over the sparse matrix `add(row, col, value)` operation,
/// so stamp logic doesn't depend on a specific matrix implementation.
pub trait MatrixStamp {
    fn add(&mut self, row: usize, col: usize, value: f64);
}

/// Stamp a single XSPICE instance's evaluation outputs into the MNA system.
///
/// This handles the Norton companion stamping for each port type:
/// - **Current Out**: stamp output current into RHS at port nodes
/// - **Voltage Out**: stamp output voltage into branch equation RHS
/// - **Conductance InOut**: stamp g_eq (from partials) into admittance matrix
///   and i_eq = f(v) - g_eq * v into RHS
///
/// `port_values` are the port input values used during evaluation (needed to
/// compute i_eq from the Norton companion).
pub fn stamp_xspice_instance<M: MatrixStamp>(
    matrix: &mut M,
    rhs: &mut [f64],
    instance: &XspiceInstance,
    port_defs: &[PortDef],
    outputs: &CmOutputs,
    port_values: &[f64],
) {
    // Process each port output
    for port_out in &outputs.port_outputs {
        let conn = &instance.port_connections[port_out.port_index];
        let port_def = &port_defs[conn.port_def_index];

        match (port_def.port_type, port_def.direction) {
            // Current output: Norton i_eq computed below after partials.
            // XSPICE convention: positive = current entering pos node.
            (PortType::Current, PortDirection::Out) | (PortType::Current, PortDirection::InOut) => {
            }

            // Conductance: Norton i_eq computed below after partials.
            // Convention: positive = current from pos to neg (leaving pos).
            (PortType::Conductance, PortDirection::InOut) => {}

            // Voltage output: set branch equation RHS to the output voltage
            (PortType::Voltage, PortDirection::Out) => {
                if let Some(br) = conn.branch_idx {
                    rhs[br] += port_out.value;
                }
            }

            _ => {}
        }
    }

    // Process partial derivatives for Jacobian stamping
    for partial in &outputs.partials {
        let out_conn = &instance.port_connections[partial.output_port];
        let in_conn = &instance.port_connections[partial.input_port];
        let out_def = &port_defs[out_conn.port_def_index];
        let in_def = &port_defs[in_conn.port_def_index];
        let g = partial.value;

        match (out_def.port_type, out_def.direction) {
            // Current output or conductance: stamp g into admittance matrix rows.
            // For current-out: XSPICE convention is positive = into pos, but KCL
            // convention is positive = leaving pos, so negate.
            // For conductance: positive = from pos to neg (leaving pos), no negate.
            (PortType::Current, PortDirection::Out)
            | (PortType::Current, PortDirection::InOut)
            | (PortType::Conductance, PortDirection::InOut) => {
                let is_current_port = matches!(out_def.port_type, PortType::Current);
                // Negate for current-out: XSPICE says positive enters pos,
                // but MNA KCL counts positive as leaving pos.
                let sign = if is_current_port { -1.0 } else { 1.0 };
                let gs = g * sign;

                match in_def.port_type {
                    PortType::Voltage | PortType::DifferentialVoltage | PortType::Conductance => {
                        stamp_matrix(matrix, out_conn.pos_idx, in_conn.pos_idx, gs);
                        stamp_matrix(matrix, out_conn.pos_idx, in_conn.neg_idx, -gs);
                        stamp_matrix(matrix, out_conn.neg_idx, in_conn.pos_idx, -gs);
                        stamp_matrix(matrix, out_conn.neg_idx, in_conn.neg_idx, gs);
                    }
                    PortType::Current => {
                        if let Some(in_br) = in_conn.branch_idx {
                            stamp_matrix(matrix, out_conn.pos_idx, Some(in_br), gs);
                            stamp_matrix(matrix, out_conn.neg_idx, Some(in_br), -gs);
                        }
                    }
                    _ => {}
                }
            }

            // Voltage output: stamp partials into branch equation row
            (PortType::Voltage, PortDirection::Out) => {
                if let Some(out_br) = out_conn.branch_idx {
                    match in_def.port_type {
                        PortType::Voltage
                        | PortType::DifferentialVoltage
                        | PortType::Conductance => {
                            stamp_matrix(matrix, Some(out_br), in_conn.pos_idx, -g);
                            stamp_matrix(matrix, Some(out_br), in_conn.neg_idx, g);
                        }
                        PortType::Current => {
                            if let Some(in_br) = in_conn.branch_idx {
                                stamp_matrix(matrix, Some(out_br), Some(in_br), -g);
                            }
                        }
                        _ => {}
                    }
                }
            }

            _ => {}
        }
    }

    // For current-out and conductance ports: compute Norton i_eq = f(v) - g_eq * v
    // and stamp it into RHS. The partials already stamped g into the matrix.
    for port_out in &outputs.port_outputs {
        let conn = &instance.port_connections[port_out.port_index];
        let port_def = &port_defs[conn.port_def_index];

        let is_current_out = matches!(
            (port_def.port_type, port_def.direction),
            (PortType::Current, PortDirection::Out)
                | (PortType::Current, PortDirection::InOut)
                | (PortType::Conductance, PortDirection::InOut)
        );

        if is_current_out {
            // Norton companion: i_eq = f(v) - g_eq * v
            let is_current_port = matches!(port_def.port_type, PortType::Current);
            let mut g_eq_v = 0.0;
            for partial in &outputs.partials {
                if partial.output_port == port_out.port_index {
                    g_eq_v += partial.value * port_values[partial.input_port];
                }
            }
            let i_eq = port_out.value - g_eq_v;
            // For current-out: XSPICE convention (positive = into pos) needs
            // negation for stamp_rhs_current (positive = leaving pos).
            let stamped_i_eq = if is_current_port { -i_eq } else { i_eq };
            stamp_rhs_current(rhs, conn.pos_idx, conn.neg_idx, stamped_i_eq);
        }
    }

    // Stamp voltage-out branch equation structure:
    // V(pos) - V(neg) = controlled voltage (already in RHS from port outputs)
    // Branch current enters KCL at pos/neg nodes
    for conn in &instance.port_connections {
        let port_def = &port_defs[conn.port_def_index];
        if port_def.port_type == PortType::Voltage
            && port_def.direction == PortDirection::Out
            && let Some(br) = conn.branch_idx
        {
            // Branch equation: V(pos) - V(neg) - ... = Vout
            stamp_matrix(matrix, Some(br), conn.pos_idx, 1.0);
            stamp_matrix(matrix, Some(br), conn.neg_idx, -1.0);
            // KCL: branch current enters pos, leaves neg
            stamp_matrix(matrix, conn.pos_idx, Some(br), 1.0);
            stamp_matrix(matrix, conn.neg_idx, Some(br), -1.0);
        }
    }

    // Stamp current-in measurement branch structure:
    // Zero-volt voltage source: V(pos) - V(neg) = 0
    // Branch current available in solution vector
    for conn in &instance.port_connections {
        let port_def = &port_defs[conn.port_def_index];
        if port_def.port_type == PortType::Current
            && port_def.direction == PortDirection::In
            && let Some(br) = conn.branch_idx
        {
            stamp_matrix(matrix, Some(br), conn.pos_idx, 1.0);
            stamp_matrix(matrix, Some(br), conn.neg_idx, -1.0);
            stamp_matrix(matrix, conn.pos_idx, Some(br), 1.0);
            stamp_matrix(matrix, conn.neg_idx, Some(br), -1.0);
        }
    }
}
