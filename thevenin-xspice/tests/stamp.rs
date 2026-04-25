//! Tests for XSPICE MNA stamping logic.

use std::cell::RefCell;

use thevenin_xspice::*;

/// Simple dense matrix for testing stamps.
struct TestMatrix {
    dim: usize,
    data: Vec<f64>,
}

impl TestMatrix {
    fn new(dim: usize) -> Self {
        Self {
            dim,
            data: vec![0.0; dim * dim],
        }
    }

    fn get(&self, row: usize, col: usize) -> f64 {
        self.data[row * self.dim + col]
    }
}

impl MatrixStamp for TestMatrix {
    fn add(&mut self, row: usize, col: usize, value: f64) {
        self.data[row * self.dim + col] += value;
    }
}

/// Test: current-out port stamps current into RHS
#[test]
fn test_stamp_current_out() {
    // 2-node circuit: port reads V(0) (node 0) and outputs current at node 1
    let mut matrix = TestMatrix::new(2);
    let mut rhs = vec![0.0; 2];

    let instance = XspiceInstance {
        name: "A1".into(),
        model_type: "TEST".into(),
        port_connections: vec![
            PortConnection {
                port_def_index: 0,
                pos_idx: Some(0),
                neg_idx: None,
                branch_idx: None,
            },
            PortConnection {
                port_def_index: 1,
                pos_idx: Some(1),
                neg_idx: None,
                branch_idx: None,
            },
        ],
        params: vec![],
        state: RefCell::new(Box::new(())),
        branch_indices: vec![],
    };

    let port_defs = vec![
        PortDef {
            name: "in".into(),
            direction: PortDirection::In,
            port_type: PortType::Voltage,
        },
        PortDef {
            name: "out".into(),
            direction: PortDirection::Out,
            port_type: PortType::Current,
        },
    ];

    let mut outputs = CmOutputs::new();
    // Output 2.0 A at port 1
    outputs.set_output(1, 2.0);
    // Partial: dI_out/dV_in = 0.5
    outputs.set_partial(1, 0, 0.5);

    let port_values = vec![4.0, 0.0]; // V_in = 4V

    stamp_xspice_instance(
        &mut matrix,
        &mut rhs,
        &instance,
        &port_defs,
        &outputs,
        &port_values,
    );

    // Current-out with XSPICE convention (positive = into pos):
    // i_eq = I_total - g * V_in = 2.0 - 0.5 * 4.0 = 0.0
    // Negated for stamp_rhs_current: stamped_i_eq = -0.0 = 0.0
    assert!((rhs[1] - 0.0).abs() < 1e-12, "rhs[1] = {}", rhs[1]);

    // Partial stamps -g = -0.5 at (out_pos, in_pos) due to XSPICE sign convention
    assert!((matrix.get(1, 0) - (-0.5)).abs() < 1e-12);
}

/// Test: conductance InOut port stamps Norton companion
#[test]
fn test_stamp_conductance_inout() {
    // Single conductance port between nodes 0 and 1
    let mut matrix = TestMatrix::new(2);
    let mut rhs = vec![0.0; 2];

    let instance = XspiceInstance {
        name: "A1".into(),
        model_type: "TEST".into(),
        port_connections: vec![PortConnection {
            port_def_index: 0,
            pos_idx: Some(0),
            neg_idx: Some(1),
            branch_idx: None,
        }],
        params: vec![],
        state: RefCell::new(Box::new(())),
        branch_indices: vec![],
    };

    let port_defs = vec![PortDef {
        name: "port".into(),
        direction: PortDirection::InOut,
        port_type: PortType::Conductance,
    }];

    let mut outputs = CmOutputs::new();
    // Total current f(v) = 0.5 A (through the port)
    outputs.set_output(0, 0.5);
    // g_eq = dI/dV = 0.1 S
    outputs.set_partial(0, 0, 0.1);

    let port_values = vec![3.0]; // V across port = 3V

    stamp_xspice_instance(
        &mut matrix,
        &mut rhs,
        &instance,
        &port_defs,
        &outputs,
        &port_values,
    );

    // Conductance stamp: 2x2 stamp with g=0.1
    assert!((matrix.get(0, 0) - 0.1).abs() < 1e-12);
    assert!((matrix.get(0, 1) - (-0.1)).abs() < 1e-12);
    assert!((matrix.get(1, 0) - (-0.1)).abs() < 1e-12);
    assert!((matrix.get(1, 1) - 0.1).abs() < 1e-12);

    // Norton i_eq = f(v) - g_eq * v = 0.5 - 0.1 * 3.0 = 0.2
    // Stamps: rhs[0] -= 0.2, rhs[1] += 0.2
    assert!((rhs[0] - (-0.2)).abs() < 1e-12, "rhs[0] = {}", rhs[0]);
    assert!((rhs[1] - 0.2).abs() < 1e-12, "rhs[1] = {}", rhs[1]);
}

/// Test: differential voltage input port stamps at both pos and neg columns.
///
/// An input port with both `pos_idx` and `neg_idx` set should produce matrix
/// entries at (out_row, in_pos) and (out_row, in_neg) with opposite signs.
#[test]
fn test_stamp_differential_voltage_input() {
    // 3-node circuit: diff input between nodes 0 and 1, current output at node 2
    let mut matrix = TestMatrix::new(3);
    let mut rhs = vec![0.0; 3];

    let instance = XspiceInstance {
        name: "A1".into(),
        model_type: "TEST".into(),
        port_connections: vec![
            PortConnection {
                port_def_index: 0,
                pos_idx: Some(0),
                neg_idx: Some(1),
                branch_idx: None,
            },
            PortConnection {
                port_def_index: 1,
                pos_idx: Some(2),
                neg_idx: None,
                branch_idx: None,
            },
        ],
        params: vec![],
        state: RefCell::new(Box::new(())),
        branch_indices: vec![],
    };

    let port_defs = vec![
        PortDef {
            name: "diff_in".into(),
            direction: PortDirection::In,
            port_type: PortType::DifferentialVoltage,
        },
        PortDef {
            name: "out".into(),
            direction: PortDirection::Out,
            port_type: PortType::Current,
        },
    ];

    let mut outputs = CmOutputs::new();
    // Output 1.0 A at port 1
    outputs.set_output(1, 1.0);
    // Partial: dI_out/dV_diff_in = 0.4
    outputs.set_partial(1, 0, 0.4);

    // V_diff_in = V(0) - V(1) = 5.0 - 2.0 = 3.0
    let port_values = vec![3.0, 0.0];

    stamp_xspice_instance(
        &mut matrix,
        &mut rhs,
        &instance,
        &port_defs,
        &outputs,
        &port_values,
    );

    // Current-out partial with g=0.4, sign negated for XSPICE convention:
    // stamp_matrix(out_pos=2, in_pos=0, -0.4)
    // stamp_matrix(out_pos=2, in_neg=1, +0.4)
    // (out_neg is None so those are skipped)
    assert!(
        (matrix.get(2, 0) - (-0.4)).abs() < 1e-12,
        "matrix[2,0] = {}",
        matrix.get(2, 0)
    );
    assert!(
        (matrix.get(2, 1) - 0.4).abs() < 1e-12,
        "matrix[2,1] = {}",
        matrix.get(2, 1)
    );
}

/// Test: current-in port creates a zero-volt vsource branch for current measurement.
///
/// A port with `direction: In`, `port_type: Current`, and a `branch_idx` should
/// stamp the vsource structure: V(pos) - V(neg) = 0 with branch current in KCL.
#[test]
fn test_stamp_current_in_measurement() {
    // 4x4: 2 nodes + 1 measurement branch + 1 output node
    let mut matrix = TestMatrix::new(4);
    let mut rhs = vec![0.0; 4];

    let instance = XspiceInstance {
        name: "A1".into(),
        model_type: "TEST".into(),
        port_connections: vec![
            // Current-in measurement port between nodes 0 and 1, branch at index 2
            PortConnection {
                port_def_index: 0,
                pos_idx: Some(0),
                neg_idx: Some(1),
                branch_idx: Some(2),
            },
            // Current-out port at node 3
            PortConnection {
                port_def_index: 1,
                pos_idx: Some(3),
                neg_idx: None,
                branch_idx: None,
            },
        ],
        params: vec![],
        state: RefCell::new(Box::new(())),
        branch_indices: vec![2],
    };

    let port_defs = vec![
        PortDef {
            name: "sense".into(),
            direction: PortDirection::In,
            port_type: PortType::Current,
        },
        PortDef {
            name: "out".into(),
            direction: PortDirection::Out,
            port_type: PortType::Current,
        },
    ];

    let mut outputs = CmOutputs::new();
    outputs.set_output(1, 0.5);

    let port_values = vec![0.0, 0.0];

    stamp_xspice_instance(
        &mut matrix,
        &mut rhs,
        &instance,
        &port_defs,
        &outputs,
        &port_values,
    );

    // Zero-volt vsource structure for current-in:
    // Branch row: V(pos=0) - V(neg=1) = 0
    assert!(
        (matrix.get(2, 0) - 1.0).abs() < 1e-12,
        "branch V(pos): {}",
        matrix.get(2, 0)
    );
    assert!(
        (matrix.get(2, 1) - (-1.0)).abs() < 1e-12,
        "branch V(neg): {}",
        matrix.get(2, 1)
    );
    // KCL: branch current enters node 0, leaves node 1
    assert!(
        (matrix.get(0, 2) - 1.0).abs() < 1e-12,
        "KCL pos: {}",
        matrix.get(0, 2)
    );
    assert!(
        (matrix.get(1, 2) - (-1.0)).abs() < 1e-12,
        "KCL neg: {}",
        matrix.get(1, 2)
    );
    // RHS for branch equation should be 0 (zero-volt source)
    assert!((rhs[2]).abs() < 1e-12, "rhs[2] = {}", rhs[2]);
}

/// Test: multiple partials for the same output port referencing different inputs.
///
/// Two partials both referencing the same output but different input ports
/// should both contribute to g_eq_v summation.
#[test]
fn test_stamp_multiple_partials_same_output() {
    // 3-node circuit: two voltage inputs (nodes 0, 1), current output at node 2
    let mut matrix = TestMatrix::new(3);
    let mut rhs = vec![0.0; 3];

    let instance = XspiceInstance {
        name: "A1".into(),
        model_type: "TEST".into(),
        port_connections: vec![
            PortConnection {
                port_def_index: 0,
                pos_idx: Some(0),
                neg_idx: None,
                branch_idx: None,
            },
            PortConnection {
                port_def_index: 1,
                pos_idx: Some(1),
                neg_idx: None,
                branch_idx: None,
            },
            PortConnection {
                port_def_index: 2,
                pos_idx: Some(2),
                neg_idx: None,
                branch_idx: None,
            },
        ],
        params: vec![],
        state: RefCell::new(Box::new(())),
        branch_indices: vec![],
    };

    let port_defs = vec![
        PortDef {
            name: "in1".into(),
            direction: PortDirection::In,
            port_type: PortType::Voltage,
        },
        PortDef {
            name: "in2".into(),
            direction: PortDirection::In,
            port_type: PortType::Voltage,
        },
        PortDef {
            name: "out".into(),
            direction: PortDirection::Out,
            port_type: PortType::Current,
        },
    ];

    let mut outputs = CmOutputs::new();
    // I_out = 3.0 A total
    outputs.set_output(2, 3.0);
    // dI_out/dV_in1 = 0.5
    outputs.set_partial(2, 0, 0.5);
    // dI_out/dV_in2 = 0.3
    outputs.set_partial(2, 1, 0.3);

    // V_in1 = 2.0, V_in2 = 4.0
    let port_values = vec![2.0, 4.0, 0.0];

    stamp_xspice_instance(
        &mut matrix,
        &mut rhs,
        &instance,
        &port_defs,
        &outputs,
        &port_values,
    );

    // Both partials should stamp into the matrix (negated for current-out):
    // matrix[2, 0] = -0.5, matrix[2, 1] = -0.3
    assert!(
        (matrix.get(2, 0) - (-0.5)).abs() < 1e-12,
        "matrix[2,0] = {}",
        matrix.get(2, 0)
    );
    assert!(
        (matrix.get(2, 1) - (-0.3)).abs() < 1e-12,
        "matrix[2,1] = {}",
        matrix.get(2, 1)
    );

    // Norton i_eq = I_total - g_eq_v
    // g_eq_v = 0.5 * 2.0 + 0.3 * 4.0 = 1.0 + 1.2 = 2.2
    // i_eq = 3.0 - 2.2 = 0.8
    // Negated for current-out stamp: stamped_i_eq = -0.8
    // stamp_rhs_current(rhs, pos=Some(2), neg=None, -0.8)
    // rhs[2] -= -0.8 → rhs[2] = 0.8
    assert!((rhs[2] - 0.8).abs() < 1e-12, "rhs[2] = {}", rhs[2]);
}

/// Test: ground nodes (None indices) are silently skipped without panicking.
///
/// Ports where `pos_idx` or `neg_idx` is `None` (ground reference) should not
/// cause any out-of-bounds access or panics.
#[test]
fn test_stamp_ground_nodes_skipped() {
    // 2-node circuit, conductance port with neg=ground
    let mut matrix = TestMatrix::new(2);
    let mut rhs = vec![0.0; 2];

    let instance = XspiceInstance {
        name: "A1".into(),
        model_type: "TEST".into(),
        port_connections: vec![PortConnection {
            port_def_index: 0,
            pos_idx: Some(0),
            neg_idx: None, // ground
            branch_idx: None,
        }],
        params: vec![],
        state: RefCell::new(Box::new(())),
        branch_indices: vec![],
    };

    let port_defs = vec![PortDef {
        name: "port".into(),
        direction: PortDirection::InOut,
        port_type: PortType::Conductance,
    }];

    let mut outputs = CmOutputs::new();
    outputs.set_output(0, 1.0);
    outputs.set_partial(0, 0, 0.5);

    let port_values = vec![2.0];

    // Should not panic even though neg_idx is None
    stamp_xspice_instance(
        &mut matrix,
        &mut rhs,
        &instance,
        &port_defs,
        &outputs,
        &port_values,
    );

    // Only pos-pos entry should be stamped (g=0.5)
    assert!(
        (matrix.get(0, 0) - 0.5).abs() < 1e-12,
        "matrix[0,0] = {}",
        matrix.get(0, 0)
    );
    // All entries involving the ground node (row/col for neg) should be zero
    assert!(
        (matrix.get(1, 0)).abs() < 1e-12,
        "matrix[1,0] = {}",
        matrix.get(1, 0)
    );
    assert!(
        (matrix.get(0, 1)).abs() < 1e-12,
        "matrix[0,1] = {}",
        matrix.get(0, 1)
    );
    assert!(
        (matrix.get(1, 1)).abs() < 1e-12,
        "matrix[1,1] = {}",
        matrix.get(1, 1)
    );

    // RHS: i_eq = 1.0 - 0.5*2.0 = 0.0 → pos stamped with 0.0
    assert!((rhs[0]).abs() < 1e-12, "rhs[0] = {}", rhs[0]);
}

/// Test: ground nodes for both pos and neg — completely grounded port.
#[test]
fn test_stamp_both_nodes_ground() {
    let mut matrix = TestMatrix::new(2);
    let mut rhs = vec![0.0; 2];

    let instance = XspiceInstance {
        name: "A1".into(),
        model_type: "TEST".into(),
        port_connections: vec![
            PortConnection {
                port_def_index: 0,
                pos_idx: Some(0),
                neg_idx: None,
                branch_idx: None,
            },
            PortConnection {
                port_def_index: 1,
                pos_idx: None, // both ground
                neg_idx: None,
                branch_idx: None,
            },
        ],
        params: vec![],
        state: RefCell::new(Box::new(())),
        branch_indices: vec![],
    };

    let port_defs = vec![
        PortDef {
            name: "in".into(),
            direction: PortDirection::In,
            port_type: PortType::Voltage,
        },
        PortDef {
            name: "out".into(),
            direction: PortDirection::Out,
            port_type: PortType::Current,
        },
    ];

    let mut outputs = CmOutputs::new();
    outputs.set_output(1, 2.0);
    outputs.set_partial(1, 0, 0.5);

    let port_values = vec![4.0, 0.0];

    // Should not panic even when output port has both nodes as ground
    stamp_xspice_instance(
        &mut matrix,
        &mut rhs,
        &instance,
        &port_defs,
        &outputs,
        &port_values,
    );

    // No matrix entries should be stamped for the grounded output
    assert!((matrix.get(0, 0)).abs() < 1e-12, "matrix[0,0] should be 0");
    assert!((matrix.get(1, 1)).abs() < 1e-12, "matrix[1,1] should be 0");
    // No RHS entries for grounded nodes
    assert!((rhs[0]).abs() < 1e-12, "rhs[0] = {}", rhs[0]);
    assert!((rhs[1]).abs() < 1e-12, "rhs[1] = {}", rhs[1]);
}

/// Test: output-only model with no partials stamps RHS but no matrix entries.
#[test]
fn test_stamp_no_partials_current_out() {
    let mut matrix = TestMatrix::new(2);
    let mut rhs = vec![0.0; 2];

    let instance = XspiceInstance {
        name: "A1".into(),
        model_type: "TEST".into(),
        port_connections: vec![PortConnection {
            port_def_index: 0,
            pos_idx: Some(0),
            neg_idx: Some(1),
            branch_idx: None,
        }],
        params: vec![],
        state: RefCell::new(Box::new(())),
        branch_indices: vec![],
    };

    let port_defs = vec![PortDef {
        name: "out".into(),
        direction: PortDirection::Out,
        port_type: PortType::Current,
    }];

    let mut outputs = CmOutputs::new();
    // Output 3.0 A, no partials
    outputs.set_output(0, 3.0);

    let port_values = vec![0.0];

    stamp_xspice_instance(
        &mut matrix,
        &mut rhs,
        &instance,
        &port_defs,
        &outputs,
        &port_values,
    );

    // Matrix should have no entries (no partials)
    for r in 0..2 {
        for c in 0..2 {
            assert!(
                matrix.get(r, c).abs() < 1e-12,
                "matrix[{},{}] = {} (expected 0)",
                r,
                c,
                matrix.get(r, c)
            );
        }
    }

    // Norton i_eq = 3.0 - 0 = 3.0 (no partials, so g_eq_v = 0)
    // Negated for current-out: stamped_i_eq = -3.0
    // stamp_rhs_current(rhs, pos=0, neg=1, -3.0):
    //   rhs[0] -= -3.0 → rhs[0] = 3.0
    //   rhs[1] += -3.0 → rhs[1] = -3.0
    assert!((rhs[0] - 3.0).abs() < 1e-12, "rhs[0] = {}", rhs[0]);
    assert!((rhs[1] - (-3.0)).abs() < 1e-12, "rhs[1] = {}", rhs[1]);
}

/// Test: voltage-out with no partials stamps only the branch equation structure and RHS.
#[test]
fn test_stamp_voltage_out_no_partials() {
    let mut matrix = TestMatrix::new(3);
    let mut rhs = vec![0.0; 3];

    let instance = XspiceInstance {
        name: "A1".into(),
        model_type: "TEST".into(),
        port_connections: vec![PortConnection {
            port_def_index: 0,
            pos_idx: Some(0),
            neg_idx: Some(1),
            branch_idx: Some(2),
        }],
        params: vec![],
        state: RefCell::new(Box::new(())),
        branch_indices: vec![2],
    };

    let port_defs = vec![PortDef {
        name: "vout".into(),
        direction: PortDirection::Out,
        port_type: PortType::Voltage,
    }];

    let mut outputs = CmOutputs::new();
    // Output voltage = 10.0 V, no partials
    outputs.set_output(0, 10.0);

    let port_values = vec![0.0];

    stamp_xspice_instance(
        &mut matrix,
        &mut rhs,
        &instance,
        &port_defs,
        &outputs,
        &port_values,
    );

    // Branch equation structure: V(pos) - V(neg) = Vout
    assert!(
        (matrix.get(2, 0) - 1.0).abs() < 1e-12,
        "branch V(pos): {}",
        matrix.get(2, 0)
    );
    assert!(
        (matrix.get(2, 1) - (-1.0)).abs() < 1e-12,
        "branch V(neg): {}",
        matrix.get(2, 1)
    );
    // KCL entries
    assert!(
        (matrix.get(0, 2) - 1.0).abs() < 1e-12,
        "KCL pos: {}",
        matrix.get(0, 2)
    );
    assert!(
        (matrix.get(1, 2) - (-1.0)).abs() < 1e-12,
        "KCL neg: {}",
        matrix.get(1, 2)
    );

    // RHS: branch equation gets Vout = 10.0
    assert!((rhs[2] - 10.0).abs() < 1e-12, "rhs[2] = {}", rhs[2]);
}

/// Test: voltage-out port creates branch equation
#[test]
fn test_stamp_voltage_out() {
    // 3x3 system: 2 nodes + 1 branch
    let mut matrix = TestMatrix::new(3);
    let mut rhs = vec![0.0; 3];

    let instance = XspiceInstance {
        name: "A1".into(),
        model_type: "TEST".into(),
        port_connections: vec![
            PortConnection {
                port_def_index: 0,
                pos_idx: Some(0),
                neg_idx: None,
                branch_idx: None,
            },
            PortConnection {
                port_def_index: 1,
                pos_idx: Some(1),
                neg_idx: None,
                branch_idx: Some(2), // branch equation at index 2
            },
        ],
        params: vec![],
        state: RefCell::new(Box::new(())),
        branch_indices: vec![2],
    };

    let port_defs = vec![
        PortDef {
            name: "in".into(),
            direction: PortDirection::In,
            port_type: PortType::Voltage,
        },
        PortDef {
            name: "out".into(),
            direction: PortDirection::Out,
            port_type: PortType::Voltage,
        },
    ];

    let mut outputs = CmOutputs::new();
    // Output voltage = 5.0 V at port 1
    outputs.set_output(1, 5.0);
    // Partial: dV_out/dV_in = 2.0 (gain of 2)
    outputs.set_partial(1, 0, 2.0);

    let port_values = vec![2.5, 0.0];

    stamp_xspice_instance(
        &mut matrix,
        &mut rhs,
        &instance,
        &port_defs,
        &outputs,
        &port_values,
    );

    // Branch equation row (2): V(pos) - ... = Vout
    // stamp_matrix(2, pos=1, 1.0) and stamp_matrix(2, neg=None, -1.0) for structure
    assert!((matrix.get(2, 1) - 1.0).abs() < 1e-12, "branch V(pos)");

    // KCL: branch current at node 1
    assert!((matrix.get(1, 2) - 1.0).abs() < 1e-12, "KCL pos");

    // Partial dV_out/dV_in = 2.0 stamps into branch row: -(g) at in_pos
    assert!(
        (matrix.get(2, 0) - (-2.0)).abs() < 1e-12,
        "partial in branch eq"
    );

    // RHS: branch equation gets Vout = 5.0
    assert!((rhs[2] - 5.0).abs() < 1e-12, "rhs branch = {}", rhs[2]);
}
