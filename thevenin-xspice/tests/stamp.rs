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
