//! Direct Cirq IR → MNA assembly (linear element subset).
//!
//! Stage 4 of `docs/migration/cirq-adoption-plan.md`. For circuits whose
//! elements are all in the linear subset — R / V / I / C / L plus the E /
//! G / H / F dependent sources — this module builds an [`MnaSystem`]
//! directly from a [`cirq_ir::Circuit`], skipping the
//! `circuit_to_netlists` + `flatten_netlist` round-trip that the existing
//! `assemble_mna(&Netlist)` path takes.
//!
//! When the circuit contains any element outside that subset,
//! [`assemble_mna_from_circuit`] returns `Ok(None)` so the caller can fall
//! back to the Netlist-shaped path. Subsequent sessions in the
//! `feat/mna-circuit-input` branch grow the supported subset device class
//! by device class, per `docs/migration/mna-ir-pivot-plan.md`.
//!
//! Bit-for-bit equivalence with the lowered path is pinned by
//! `thevenin-cirq/tests/direct_path_equivalence.rs`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use cirq_frontend::to_netlist::{convert_model, convert_source_spec, extra_params};
use cirq_ir::{Circuit, Element as IrElement, ElementKind as IrElementKind, Id, Model, Value};
use thevenin_types::{Expr, Source};
use thevenin_xspice::CodeModelRegistry;

use crate::bjt::{BjtInstance, BjtModel};
use crate::diode::DiodeModel;
use crate::mna::{
    CapacitorInstance, CurrentSourceInstance, DiodeInstance, InductorInstance, MnaError, MnaSystem,
    NodeMap, ResistorInstance, VoltageSourceInstance, push_bjt_caps, stamp_conductance,
};
use crate::newton::NrOptions;
use crate::vbic::{VbicInstance, VbicModel};

/// Extract Newton-Raphson options from a circuit's `options` field.
///
/// Circuit-side equivalent of `crate::simulate::nr_options_from_netlist`.
/// Recognises the same `.OPTIONS` keys (GMIN, ABSTOL, RELTOL, VNTOL,
/// ITL1/ITL2/ITL4) and ignores non-numeric values silently.
pub fn nr_options_from_circuit(circuit: &Circuit) -> NrOptions {
    let mut opts = NrOptions::default();
    for (name, value) in &circuit.options {
        let v = match value {
            Value::Real(f) => *f,
            Value::Integer(i) => *i as f64,
            _ => continue,
        };
        match name.to_uppercase().as_str() {
            "GMIN" => opts.gmin = v,
            "ABSTOL" => opts.abstol = v,
            "RELTOL" => opts.reltol = v,
            "VNTOL" => opts.vntol = v,
            "ITL1" => opts.itl1 = v as usize,
            "ITL2" => opts.itl2 = v as usize,
            "ITL4" => opts.itl4 = v as usize,
            _ => {}
        }
    }
    opts
}

/// Resolve a circuit's `.nodeset` (IR-shaped: `(Id, f64)` net-id pairs) into
/// `(matrix_index, voltage)` pairs against an assembled [`MnaSystem`].
///
/// Circuit-side equivalent of `crate::simulate::resolve_nodeset`. Hidden
/// pairs that don't resolve (unknown ids, or the named net is ground and
/// therefore excluded from the matrix) are dropped silently — matching the
/// Netlist path's behaviour.
pub fn resolve_nodeset_from_circuit(circuit: &Circuit, mna: &MnaSystem) -> Vec<(usize, f64)> {
    let net_lookup: HashMap<Id, String> = circuit
        .nets
        .iter()
        .map(|n| {
            let name = if n.name == "gnd" {
                "0".to_string()
            } else {
                n.name.clone()
            };
            (n.id, name)
        })
        .collect();
    let mut pairs = Vec::new();
    for (net_id, val) in &circuit.nodeset {
        if let Some(name) = net_lookup.get(net_id)
            && let Some(idx) = mna.node_map.get(name)
        {
            pairs.push((idx, *val));
        }
    }
    pairs
}

/// Build an `MnaSystem` directly from a Cirq IR circuit when every element
/// is part of the linear subset (R / V / I / C / L / E / G / H / F).
///
/// Returns `Ok(None)` when the circuit contains any other element kind,
/// signalling the caller should fall back to
/// `crate::mna::assemble_mna(&Netlist)`.
pub fn assemble_mna_from_circuit(
    circuit: &Circuit,
    modedc: bool,
    xspice_registry: Option<Arc<CodeModelRegistry>>,
) -> Result<Option<MnaSystem>, MnaError> {
    if !circuit_is_supported_subset(circuit) {
        return Ok(None);
    }
    Ok(Some(stamp_circuit(circuit, modedc, xspice_registry)?))
}

/// Whether every element in `circuit` is in the device subset this module
/// currently handles. Anything outside — BJTs, MOSFETs, JFETs, behavioural
/// sources, distributed elements, XSPICE — sends the caller back to the
/// Netlist path. Coverage grows session by session per
/// `docs/migration/mna-ir-pivot-plan.md`.
fn circuit_is_supported_subset(circuit: &Circuit) -> bool {
    circuit.elements.iter().all(|e| {
        matches!(
            e.kind,
            IrElementKind::Resistor
                | IrElementKind::Capacitor
                | IrElementKind::Inductor
                | IrElementKind::VoltageSource
                | IrElementKind::CurrentSource
                | IrElementKind::Vcvs
                | IrElementKind::Vccs
                | IrElementKind::Ccvs
                | IrElementKind::Cccs
                | IrElementKind::Diode
                | IrElementKind::Npn
                | IrElementKind::Pnp
        )
    })
}

/// Build the `Id → name` map for nets, rewriting `gnd` → `0` so the produced
/// `NodeMap` keys match what `circuit_to_netlists` would have produced.
///
/// SPICE-imported circuits use `"0"` already; Cirq-source-compiled circuits
/// use `"gnd"`. After this rewrite, `NodeMap::index` (which excludes the
/// literal `"0"` from the matrix) treats both as ground uniformly.
fn build_net_name_map(circuit: &Circuit) -> HashMap<Id, String> {
    circuit
        .nets
        .iter()
        .map(|n| {
            let name = if n.name == "gnd" {
                "0".to_string()
            } else {
                n.name.clone()
            };
            (n.id, name)
        })
        .collect()
}

/// Resolve the net name attached to an element terminal.
fn terminal_name<'a>(
    elem: &IrElement,
    terminal: &str,
    net_name: &'a HashMap<Id, String>,
) -> Result<&'a str, MnaError> {
    let conn = elem
        .connections
        .iter()
        .find(|c| c.terminal == terminal)
        .ok_or_else(|| {
            MnaError::UnsupportedElement(format!(
                "element `{}`: missing terminal `{}`",
                elem.name, terminal
            ))
        })?;
    net_name.get(&conn.net).map(String::as_str).ok_or_else(|| {
        MnaError::UnsupportedElement(format!(
            "element `{}`: terminal `{}` references unknown net id",
            elem.name, terminal
        ))
    })
}

/// Read a numeric element parameter by trying several candidate names in
/// order (e.g. `["gain", "value"]` for VCVS). Returns the first match
/// converted to `f64`, ignoring `Value::String` / `Value::Bool` entries.
fn numeric_param(elem: &IrElement, names: &[&str]) -> Option<f64> {
    for name in names {
        for (k, v) in &elem.params {
            if k.eq_ignore_ascii_case(name) {
                return match v {
                    Value::Real(f) => Some(*f),
                    Value::Integer(i) => Some(*i as f64),
                    Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
                    Value::String(_) => None,
                };
            }
        }
    }
    None
}

/// Read a string element parameter by name (used for controlled-source
/// `vsrc` references).
fn string_param(elem: &IrElement, name: &str) -> Option<String> {
    for (k, v) in &elem.params {
        if k.eq_ignore_ascii_case(name)
            && let Value::String(s) = v
        {
            return Some(s.clone());
        }
    }
    None
}

/// Resistor multiplier handling: `R_eff = R * scale / m`, matching the
/// existing `apply_multipliers` in `crate::mna`.
fn resistor_multipliers(elem: &IrElement) -> f64 {
    let m = numeric_param(elem, &["m"]).unwrap_or(1.0);
    let scale = numeric_param(elem, &["scale"]).unwrap_or(1.0);
    scale / m
}

/// Evaluate a source's DC contribution following the same MODEDC /
/// MODEDCOP convention as `crate::mna::stamp_element`.
fn evaluate_source_dc(source: &Source, modedc: bool) -> f64 {
    let dc_from_expr = |e: &Expr| match e {
        Expr::Num(v) => *v,
        // IR sources always carry a typed Value, so convert_source_spec only
        // emits Expr::Num. Anything else implies upstream drift; treat as
        // zero rather than failing here (matches expr_value(Param/Brace)'s
        // documented behaviour for sources without a numeric DC).
        _ => 0.0,
    };
    let waveform_at_zero = || {
        source.waveform.as_ref().map_or(0.0, |wf| {
            let tran = crate::waveform::TranParams {
                tstep: 1e-9,
                tstop: 1.0,
            };
            crate::waveform::evaluate(wf, 0.0, &tran)
        })
    };
    if modedc {
        // MODEDC: waveform takes precedence at t=0; only fall back to DC
        // when there's no waveform.
        if source.waveform.is_some() {
            waveform_at_zero()
        } else {
            source.dc.as_ref().map(dc_from_expr).unwrap_or(0.0)
        }
    } else {
        // MODEDCOP: explicit DC wins; otherwise use waveform at t=0.
        source
            .dc
            .as_ref()
            .map(dc_from_expr)
            .unwrap_or_else(waveform_at_zero)
    }
}

/// Resolve an element's IR model reference (`Element.model: Option<Id>`)
/// against `circuit.models`. Returns `None` when the element carries no
/// model link or the linked id isn't in the model table — callers fall
/// back to the model's default (e.g. `DiodeModel::default()`).
fn lookup_model<'a>(circuit: &'a Circuit, elem: &IrElement) -> Option<&'a Model> {
    let id = elem.model?;
    circuit.models.iter().find(|m| m.id == id)
}

/// Build a fully-resolved [`DiodeModel`] for a diode element, layering
/// instance parameters (e.g. `AREA`, `IC`, `TEMP`) over the model defaults
/// the way `mna::assemble_mna_flat` does. When the element has no model
/// reference, falls back to `DiodeModel::default()` — mirroring the
/// Netlist path's behaviour for "naked" diodes.
fn load_diode_model(circuit: &Circuit, elem: &IrElement) -> DiodeModel {
    let base = lookup_model(circuit, elem)
        .map(|m| DiodeModel::from_model_def(&convert_model(m)))
        .unwrap_or_default();
    base.with_instance_params(&extra_params(elem, &["value"]))
}

/// Circuit-side analogue of `crate::netlist_temp(&Netlist) -> f64`.
///
/// Reads the first `.temp` directive if present, else looks for a numeric
/// `TEMP` entry in `circuit.options`, else returns 27 °C.
fn circuit_temp(circuit: &Circuit) -> f64 {
    if let Some(t) = circuit.temps.first() {
        return *t;
    }
    for (name, value) in &circuit.options {
        if name.eq_ignore_ascii_case("TEMP")
            && let Some(v) = match value {
                Value::Real(f) => Some(*f),
                Value::Integer(i) => Some(*i as f64),
                _ => None,
            }
        {
            return v;
        }
    }
    27.0
}

/// Determine the BJT model level from instance params first, then model
/// params. Default is 1 (Gummel-Poon); level 4 is VBIC. Mirrors
/// `crate::mna::get_bjt_level` exactly.
fn bjt_level(model: Option<&Model>, instance_params: &[(String, Value)]) -> i32 {
    for (name, value) in instance_params {
        if name.eq_ignore_ascii_case("LEVEL")
            && let Some(v) = numeric_value(value)
        {
            return v as i32;
        }
    }
    if let Some(m) = model {
        for (name, value) in &m.params {
            if name.eq_ignore_ascii_case("LEVEL")
                && let Some(v) = numeric_value(value)
            {
                return v as i32;
            }
        }
    }
    1
}

/// Extract a numeric value from a Cirq IR [`Value`], or `None` for strings.
fn numeric_value(v: &Value) -> Option<f64> {
    match v {
        Value::Real(f) => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(_) => None,
    }
}

/// Build a fully-resolved [`BjtModel`] (level 1 Gummel-Poon) for an Npn/Pnp
/// element with instance overrides applied. Mirrors the Netlist path's
/// "default to NPN when no model is linked" behaviour exactly.
fn load_bjt_model(circuit: &Circuit, elem: &IrElement) -> BjtModel {
    let base = lookup_model(circuit, elem)
        .map(|m| BjtModel::from_model_def(&convert_model(m)))
        .unwrap_or_else(|| BjtModel::new(crate::bjt::BjtType::Npn));
    base.with_instance_params(&extra_params(elem, &["value"]))
}

/// Build a fully-resolved [`VbicModel`] (level 4) with temperature applied.
/// Mirrors `assemble_mna_flat`'s VBIC branch exactly: no
/// `with_instance_params` call (the existing Netlist path doesn't apply
/// instance IS/RCX/RBX/RE/RS overrides to VBIC even though the method
/// exists).
fn load_vbic_model(circuit: &Circuit, elem: &IrElement) -> VbicModel {
    let mut vm = lookup_model(circuit, elem)
        .map(|m| VbicModel::from_model_def(&convert_model(m)))
        .unwrap_or_else(|| VbicModel::new(crate::vbic::VbicType::Npn));
    vm.temperature_adjust(circuit_temp(circuit));
    vm
}

fn stamp_circuit(
    circuit: &Circuit,
    modedc: bool,
    xspice_registry: Option<Arc<CodeModelRegistry>>,
) -> Result<MnaSystem, MnaError> {
    let net_name = build_net_name_map(circuit);

    // -----------------------------------------------------------------
    // First pass: index nodes, count vsource branches and internal nodes,
    // build the name → branch-offset map that F/H reference for their
    // controlling source.
    // -----------------------------------------------------------------
    let mut node_map = NodeMap::new();
    let mut vsource_count = 0usize;
    let mut internal_node_count = 0usize;
    let mut vsource_offset_map: BTreeMap<String, usize> = BTreeMap::new();

    for elem in &circuit.elements {
        match elem.kind {
            IrElementKind::Resistor
            | IrElementKind::Capacitor
            | IrElementKind::CurrentSource => {
                let pos = terminal_name(elem, "pos", &net_name)?;
                let neg = terminal_name(elem, "neg", &net_name)?;
                node_map.index(pos);
                node_map.index(neg);
            }
            IrElementKind::VoltageSource | IrElementKind::Inductor => {
                let pos = terminal_name(elem, "pos", &net_name)?;
                let neg = terminal_name(elem, "neg", &net_name)?;
                node_map.index(pos);
                node_map.index(neg);
                vsource_offset_map.insert(elem.name.to_lowercase(), vsource_count);
                vsource_count += 1;
            }
            IrElementKind::Vcvs | IrElementKind::Vccs => {
                for term in ["out_pos", "out_neg", "in_pos", "in_neg"] {
                    let n = terminal_name(elem, term, &net_name)?;
                    node_map.index(n);
                }
                if matches!(elem.kind, IrElementKind::Vcvs) {
                    vsource_offset_map.insert(elem.name.to_lowercase(), vsource_count);
                    vsource_count += 1;
                }
            }
            IrElementKind::Ccvs => {
                let op = terminal_name(elem, "out_pos", &net_name)?;
                let on = terminal_name(elem, "out_neg", &net_name)?;
                node_map.index(op);
                node_map.index(on);
                vsource_offset_map.insert(elem.name.to_lowercase(), vsource_count);
                vsource_count += 1;
            }
            IrElementKind::Cccs => {
                let op = terminal_name(elem, "out_pos", &net_name)?;
                let on = terminal_name(elem, "out_neg", &net_name)?;
                node_map.index(op);
                node_map.index(on);
            }
            IrElementKind::Diode => {
                let anode = terminal_name(elem, "anode", &net_name)?;
                let cathode = terminal_name(elem, "cathode", &net_name)?;
                node_map.index(anode);
                node_map.index(cathode);
                // Internal node only when the resolved DiodeModel has RS > 0.
                let dm = load_diode_model(circuit, elem);
                if dm.has_series_resistance() {
                    internal_node_count += 1;
                }
            }
            IrElementKind::Npn | IrElementKind::Pnp => {
                let c = terminal_name(elem, "collector", &net_name)?;
                let b = terminal_name(elem, "base", &net_name)?;
                let e = terminal_name(elem, "emitter", &net_name)?;
                node_map.index(c);
                node_map.index(b);
                node_map.index(e);
                let has_substrate = elem
                    .connections
                    .iter()
                    .any(|cn| cn.terminal == "substrate");
                if has_substrate {
                    let s = terminal_name(elem, "substrate", &net_name)?;
                    node_map.index(s);
                }
                // Mirror `mna::assemble_mna_flat`'s first-pass counting:
                // build the *unmodified* model and ask it for its internal
                // node count. with_instance_params (BJT level 1) only
                // touches IS/BF/BR — none of which affect the count.
                let model = lookup_model(circuit, elem);
                let level = bjt_level(model, &elem.params);
                if level == 4 {
                    let vm = model
                        .map(|m| VbicModel::from_model_def(&convert_model(m)))
                        .unwrap_or_else(|| VbicModel::new(crate::vbic::VbicType::Npn));
                    // Mirror mna::assemble_mna_flat: it uses the substrate-
                    // present variant unconditionally, and the second pass
                    // allocates an SI internal node whenever vm.rs > 0
                    // regardless of substrate being None. Consistency here
                    // requires the same assumption.
                    let _ = has_substrate;
                    internal_node_count += vm.internal_node_count();
                } else {
                    let bm = model
                        .map(|m| BjtModel::from_model_def(&convert_model(m)))
                        .unwrap_or_else(|| BjtModel::new(crate::bjt::BjtType::Npn));
                    internal_node_count += bm.internal_node_count();
                }
            }
            _ => unreachable!("circuit_is_supported_subset filtered above"),
        }
    }

    let n_nodes = node_map.len() + internal_node_count;
    let dim = n_nodes + vsource_count;
    let mut mna = MnaSystem::empty(dim, xspice_registry);
    mna.node_map = node_map;
    let mut vsource_idx = 0usize;
    let mut internal_idx = mna.node_map.len();

    // -----------------------------------------------------------------
    // Second pass: stamp every element.
    // -----------------------------------------------------------------
    for elem in &circuit.elements {
        match elem.kind {
            IrElementKind::Resistor => {
                let pos = terminal_name(elem, "pos", &net_name)?;
                let neg = terminal_name(elem, "neg", &net_name)?;
                let r_raw = numeric_param(elem, &["value"]).ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "resistor `{}` missing numeric `value`",
                        elem.name
                    ))
                })?;
                let r = r_raw * resistor_multipliers(elem);
                if r == 0.0 {
                    return Err(MnaError::UnsupportedElement(format!(
                        "resistor `{}` has zero resistance",
                        elem.name
                    )));
                }
                let g = 1.0 / r;
                let pi = mna.node_map.get(pos);
                let ni = mna.node_map.get(neg);
                stamp_conductance(&mut mna.system.matrix, pi, ni, g);

                let ac_resistance = numeric_param(elem, &["ac"]);
                let m_val = numeric_param(elem, &["m"]).unwrap_or(1.0);
                mna.resistors.push(ResistorInstance {
                    name: elem.name.clone(),
                    pos_idx: pi,
                    neg_idx: ni,
                    resistance: r,
                    ac_resistance,
                    kf: 0.0,
                    af: 1.0,
                    ef: 1.0,
                    noise_area: 0.0,
                    m: m_val,
                });
            }
            IrElementKind::VoltageSource => {
                let pos = terminal_name(elem, "pos", &net_name)?;
                let neg = terminal_name(elem, "neg", &net_name)?;
                let source = convert_source_spec(elem);
                let v = evaluate_source_dc(&source, modedc);
                let pi = mna.node_map.get(pos);
                let ni = mna.node_map.get(neg);
                let branch = n_nodes + vsource_idx;

                if let Some(i) = pi {
                    mna.system.matrix.add(i, branch, 1.0);
                    mna.system.matrix.add(branch, i, 1.0);
                }
                if let Some(j) = ni {
                    mna.system.matrix.add(j, branch, -1.0);
                    mna.system.matrix.add(branch, j, -1.0);
                }
                mna.system.rhs[branch] = v;

                mna.voltage_sources.push(VoltageSourceInstance {
                    branch_idx: branch,
                    waveform: source.waveform.clone(),
                });
                mna.vsource_names.push(elem.name.clone());
                vsource_idx += 1;
            }
            IrElementKind::CurrentSource => {
                let pos = terminal_name(elem, "pos", &net_name)?;
                let neg = terminal_name(elem, "neg", &net_name)?;
                let source = convert_source_spec(elem);
                let i_val = evaluate_source_dc(&source, modedc);
                let pi = mna.node_map.get(pos);
                let ni = mna.node_map.get(neg);

                // SPICE convention: current flows pos → neg through the
                // external circuit, exits pos (subtract from RHS) and
                // enters neg (add to RHS).
                if let Some(i) = pi {
                    mna.system.rhs[i] -= i_val;
                }
                if let Some(j) = ni {
                    mna.system.rhs[j] += i_val;
                }

                mna.current_sources.push(CurrentSourceInstance {
                    name: elem.name.clone(),
                    pos_idx: pi,
                    neg_idx: ni,
                    dc_value: i_val,
                    waveform: source.waveform,
                });
            }
            IrElementKind::Capacitor => {
                let pos = terminal_name(elem, "pos", &net_name)?;
                let neg = terminal_name(elem, "neg", &net_name)?;
                let cap = numeric_param(elem, &["value"]).ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "capacitor `{}` missing numeric `value`",
                        elem.name
                    ))
                })?;
                let ic = numeric_param(elem, &["ic"]);
                mna.capacitors.push(CapacitorInstance {
                    name: Some(elem.name.clone()),
                    pos_idx: mna.node_map.get(pos),
                    neg_idx: mna.node_map.get(neg),
                    capacitance: cap,
                    ic,
                });
                // DC: capacitor is open — no matrix stamp.
            }
            IrElementKind::Inductor => {
                let pos = terminal_name(elem, "pos", &net_name)?;
                let neg = terminal_name(elem, "neg", &net_name)?;
                let ind = numeric_param(elem, &["value"]).ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "inductor `{}` missing numeric `value`",
                        elem.name
                    ))
                })?;
                let ic = numeric_param(elem, &["ic"]);
                let pi = mna.node_map.get(pos);
                let ni = mna.node_map.get(neg);
                let branch = n_nodes + vsource_idx;

                // DC: inductor is a short — stamp like a 0V vsource.
                if let Some(i) = pi {
                    mna.system.matrix.add(i, branch, 1.0);
                    mna.system.matrix.add(branch, i, 1.0);
                }
                if let Some(j) = ni {
                    mna.system.matrix.add(j, branch, -1.0);
                    mna.system.matrix.add(branch, j, -1.0);
                }
                mna.system.rhs[branch] = 0.0;

                mna.inductors.push(InductorInstance {
                    name: Some(elem.name.clone()),
                    pos_idx: pi,
                    neg_idx: ni,
                    branch_idx: branch,
                    inductance: ind,
                    ic,
                });
                mna.vsource_names.push(elem.name.clone());
                vsource_idx += 1;
            }
            IrElementKind::Vcvs => {
                let gain = numeric_param(elem, &["gain", "value"]).ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "VCVS `{}` missing numeric `gain`/`value`",
                        elem.name
                    ))
                })?;
                let op = mna
                    .node_map
                    .get(terminal_name(elem, "out_pos", &net_name)?);
                let on = mna
                    .node_map
                    .get(terminal_name(elem, "out_neg", &net_name)?);
                let cp = mna.node_map.get(terminal_name(elem, "in_pos", &net_name)?);
                let cn = mna.node_map.get(terminal_name(elem, "in_neg", &net_name)?);
                let branch = n_nodes + vsource_idx;

                if let Some(i) = op {
                    mna.system.matrix.add(i, branch, 1.0);
                    mna.system.matrix.add(branch, i, 1.0);
                }
                if let Some(j) = on {
                    mna.system.matrix.add(j, branch, -1.0);
                    mna.system.matrix.add(branch, j, -1.0);
                }
                if let Some(p) = cp {
                    mna.system.matrix.add(branch, p, -gain);
                }
                if let Some(n) = cn {
                    mna.system.matrix.add(branch, n, gain);
                }

                mna.vsource_names.push(elem.name.clone());
                vsource_idx += 1;
            }
            IrElementKind::Vccs => {
                let gm = numeric_param(elem, &["gm", "value"]).ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "VCCS `{}` missing numeric `gm`/`value`",
                        elem.name
                    ))
                })?;
                let op = mna
                    .node_map
                    .get(terminal_name(elem, "out_pos", &net_name)?);
                let on = mna
                    .node_map
                    .get(terminal_name(elem, "out_neg", &net_name)?);
                let cp = mna.node_map.get(terminal_name(elem, "in_pos", &net_name)?);
                let cn = mna.node_map.get(terminal_name(elem, "in_neg", &net_name)?);

                if let Some(i) = op {
                    if let Some(p) = cp {
                        mna.system.matrix.add(i, p, gm);
                    }
                    if let Some(n) = cn {
                        mna.system.matrix.add(i, n, -gm);
                    }
                }
                if let Some(j) = on {
                    if let Some(p) = cp {
                        mna.system.matrix.add(j, p, -gm);
                    }
                    if let Some(n) = cn {
                        mna.system.matrix.add(j, n, gm);
                    }
                }
            }
            IrElementKind::Ccvs => {
                let rm = numeric_param(elem, &["rm", "value"]).ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "CCVS `{}` missing numeric `rm`/`value`",
                        elem.name
                    ))
                })?;
                let vsrc = string_param(elem, "vsrc").ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "CCVS `{}` missing `vsrc` reference",
                        elem.name
                    ))
                })?;
                let ctrl_offset =
                    vsource_offset_map
                        .get(&vsrc.to_lowercase())
                        .ok_or_else(|| {
                            MnaError::UnsupportedElement(format!(
                                "CCVS `{}` references unknown controlling source `{}`",
                                elem.name, vsrc
                            ))
                        })?;
                let ctrl_branch = n_nodes + ctrl_offset;
                let op = mna
                    .node_map
                    .get(terminal_name(elem, "out_pos", &net_name)?);
                let on = mna
                    .node_map
                    .get(terminal_name(elem, "out_neg", &net_name)?);
                let branch = n_nodes + vsource_idx;

                if let Some(i) = op {
                    mna.system.matrix.add(i, branch, 1.0);
                    mna.system.matrix.add(branch, i, 1.0);
                }
                if let Some(j) = on {
                    mna.system.matrix.add(j, branch, -1.0);
                    mna.system.matrix.add(branch, j, -1.0);
                }
                mna.system.matrix.add(branch, ctrl_branch, -rm);

                mna.vsource_names.push(elem.name.clone());
                vsource_idx += 1;
            }
            IrElementKind::Cccs => {
                let gain = numeric_param(elem, &["gain", "value"]).ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "CCCS `{}` missing numeric `gain`/`value`",
                        elem.name
                    ))
                })?;
                let vsrc = string_param(elem, "vsrc").ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "CCCS `{}` missing `vsrc` reference",
                        elem.name
                    ))
                })?;
                let ctrl_offset =
                    vsource_offset_map
                        .get(&vsrc.to_lowercase())
                        .ok_or_else(|| {
                            MnaError::UnsupportedElement(format!(
                                "CCCS `{}` references unknown controlling source `{}`",
                                elem.name, vsrc
                            ))
                        })?;
                let ctrl_branch = n_nodes + ctrl_offset;
                let op = mna
                    .node_map
                    .get(terminal_name(elem, "out_pos", &net_name)?);
                let on = mna
                    .node_map
                    .get(terminal_name(elem, "out_neg", &net_name)?);

                if let Some(i) = op {
                    mna.system.matrix.add(i, ctrl_branch, gain);
                }
                if let Some(j) = on {
                    mna.system.matrix.add(j, ctrl_branch, -gain);
                }
            }
            IrElementKind::Diode => {
                let dm = load_diode_model(circuit, elem);
                let anode_idx = mna.node_map.get(terminal_name(elem, "anode", &net_name)?);
                let cathode_idx = mna.node_map.get(terminal_name(elem, "cathode", &net_name)?);

                let int_idx = if dm.has_series_resistance() {
                    let idx = internal_idx;
                    internal_idx += 1;
                    Some(idx)
                } else {
                    None
                };

                mna.diodes.push(DiodeInstance {
                    anode_idx,
                    cathode_idx,
                    internal_idx: int_idx,
                    model: dm.clone(),
                });

                // Synthetic capacitor for diode junction cap (CJO at zero bias).
                // Junction node is `internal_idx` when RS > 0, else the anode.
                let jct_node = int_idx.or(anode_idx);
                if dm.cjo > 0.0 {
                    mna.capacitors.push(CapacitorInstance {
                        name: None,
                        pos_idx: jct_node,
                        neg_idx: cathode_idx,
                        capacitance: dm.cjo,
                        ic: None,
                    });
                }
                // The conductance/current stamps are applied during NR
                // iteration in `device_stamp::diode`, not here.
            }
            IrElementKind::Npn | IrElementKind::Pnp => {
                // Instance scalars (defaults match `mna::assemble_mna_flat`).
                let mut area = 1.0;
                let mut areab = 1.0;
                let mut areac = 1.0;
                let mut m_mult = 1.0;
                let mut inst_temp = f64::NAN;
                let mut off_flag = false;
                for (name, value) in &elem.params {
                    let key = name.to_uppercase();
                    if let Some(v) = numeric_value(value) {
                        match key.as_str() {
                            "AREA" => area = v,
                            "AREAB" => areab = v,
                            "AREAC" => areac = v,
                            "M" => m_mult = v,
                            "TEMP" => inst_temp = v,
                            _ => {}
                        }
                    }
                    if key == "OFF"
                        && let Value::Bool(b) = value
                    {
                        off_flag = *b;
                    }
                }

                let coll_idx = mna.node_map.get(terminal_name(elem, "collector", &net_name)?);
                let base_idx = mna.node_map.get(terminal_name(elem, "base", &net_name)?);
                let emit_idx = mna.node_map.get(terminal_name(elem, "emitter", &net_name)?);
                let subs_idx = if elem
                    .connections
                    .iter()
                    .any(|cn| cn.terminal == "substrate")
                {
                    mna.node_map
                        .get(terminal_name(elem, "substrate", &net_name)?)
                } else {
                    None
                };

                let level = bjt_level(lookup_model(circuit, elem), &elem.params);
                if level == 4 {
                    // VBIC: 3 always-internal nodes + 4 conditional + thermal.
                    let vm = load_vbic_model(circuit, elem);

                    let coll_ci_idx = Some({
                        let idx = internal_idx;
                        internal_idx += 1;
                        idx
                    });
                    let base_bi_idx = Some({
                        let idx = internal_idx;
                        internal_idx += 1;
                        idx
                    });
                    let base_bp_idx = Some({
                        let idx = internal_idx;
                        internal_idx += 1;
                        idx
                    });
                    let coll_cx_idx = if vm.rcx > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        coll_idx
                    };
                    let base_bx_idx = if vm.rbx > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        base_idx
                    };
                    let emit_ei_idx = if vm.re > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        emit_idx
                    };
                    let subs_si_idx = if vm.rs > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        subs_idx
                    };
                    let rth_idx = if vm.rth > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        None
                    };
                    let t_ambient = circuit_temp(circuit);

                    mna.vbics.push(VbicInstance {
                        name: elem.name.clone(),
                        coll_idx,
                        base_idx,
                        emit_idx,
                        subs_idx,
                        coll_ci_idx,
                        base_bi_idx,
                        base_bp_idx,
                        coll_cx_idx,
                        base_bx_idx,
                        emit_ei_idx,
                        subs_si_idx,
                        rth_idx,
                        model: vm,
                        area,
                        m: m_mult,
                        t_ambient,
                    });
                    let _ = (areab, areac);
                } else {
                    // Level 1 Gummel-Poon.
                    let bm = load_bjt_model(circuit, elem);

                    let base_prime_idx = if bm.rb > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        base_idx
                    };
                    let col_prime_idx = if bm.rc > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        coll_idx
                    };
                    let emit_prime_idx = if bm.re > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        emit_idx
                    };

                    mna.bjts.push(BjtInstance {
                        name: elem.name.clone(),
                        col_idx: coll_idx,
                        base_idx,
                        emit_idx,
                        base_prime_idx,
                        col_prime_idx,
                        emit_prime_idx,
                        model: bm.clone(),
                        area,
                        areab,
                        areac,
                        m: m_mult,
                        temp: inst_temp,
                        off: off_flag,
                    });

                    let cap_idx = push_bjt_caps(
                        &mut mna.capacitors,
                        base_prime_idx,
                        col_prime_idx,
                        emit_prime_idx,
                        &bm,
                        area,
                        m_mult,
                    );
                    mna.bjt_cap_indices.push(cap_idx);
                }
                // BJT/VBIC stamps are applied during NR iteration, not here.
            }
            _ => unreachable!("circuit_is_supported_subset filtered above"),
        }
    }

    debug_assert_eq!(vsource_idx, vsource_count);
    Ok(mna)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cirq_ir::{
        Analysis as IrAnalysis, Connection, Element, Net, ResolvedParam, SourceSpec, Value,
    };

    fn divider() -> Circuit {
        Circuit {
            name: "divider".into(),
            nets: vec![
                Net {
                    id: Id(0),
                    name: "0".into(),
                    is_global: true,
                },
                Net {
                    id: Id(1),
                    name: "in".into(),
                    is_global: false,
                },
                Net {
                    id: Id(2),
                    name: "mid".into(),
                    is_global: false,
                },
            ],
            elements: vec![
                Element {
                    id: Id(0),
                    name: "V1".into(),
                    kind: IrElementKind::VoltageSource,
                    connections: vec![
                        Connection {
                            terminal: "pos".into(),
                            net: Id(1),
                        },
                        Connection {
                            terminal: "neg".into(),
                            net: Id(0),
                        },
                    ],
                    params: vec![],
                    model: None,
                    source_spec: Some(SourceSpec {
                        dc: Some(3.0),
                        ac: None,
                        waveform: None,
                    }),
                },
                Element {
                    id: Id(1),
                    name: "R1".into(),
                    kind: IrElementKind::Resistor,
                    connections: vec![
                        Connection {
                            terminal: "pos".into(),
                            net: Id(1),
                        },
                        Connection {
                            terminal: "neg".into(),
                            net: Id(2),
                        },
                    ],
                    params: vec![("value".into(), Value::Real(1_000.0))],
                    model: None,
                    source_spec: None,
                },
                Element {
                    id: Id(2),
                    name: "R2".into(),
                    kind: IrElementKind::Resistor,
                    connections: vec![
                        Connection {
                            terminal: "pos".into(),
                            net: Id(2),
                        },
                        Connection {
                            terminal: "neg".into(),
                            net: Id(0),
                        },
                    ],
                    params: vec![("value".into(), Value::Real(2_000.0))],
                    model: None,
                    source_spec: None,
                },
            ],
            models: vec![],
            analyses: vec![IrAnalysis::Op],
            params: Vec::<ResolvedParam>::new(),
            options: vec![],
            temps: vec![],
            save: vec![],
            funcs: vec![],
            initial_conditions: vec![],
            nodeset: vec![],
            measures: vec![],
            code_blocks: vec![],
            raw_directives: vec![],
        }
    }

    #[test]
    fn linear_circuit_assembled_from_ir() {
        let mna = assemble_mna_from_circuit(&divider(), false, None)
            .unwrap()
            .expect("linear-subset circuit");
        // 2 non-ground nodes (in, mid) + 1 vsource branch = dim 3.
        assert_eq!(mna.system.dim(), 3);
        assert_eq!(mna.vsource_names, vec!["V1".to_string()]);

        // Solve and check V(mid) = 3V * 2k / (1k + 2k) = 2V.
        let solution = mna.system.solve().unwrap();
        let in_idx = mna.node_map.get("in").unwrap();
        let mid_idx = mna.node_map.get("mid").unwrap();
        assert!((solution[in_idx] - 3.0).abs() < 1e-9);
        assert!((solution[mid_idx] - 2.0).abs() < 1e-9);
    }

    #[test]
    fn unsupported_circuit_returns_none() {
        // MOSFET (Nmos) is not yet handled by the direct IR path — adding
        // one should make `assemble_mna_from_circuit` return Ok(None) so
        // the caller falls back to `assemble_mna(&Netlist)`. Diodes and
        // BJTs are now supported; this test pins the fallback for
        // still-pending device classes (see
        // `docs/migration/mna-ir-pivot-plan.md`).
        let mut c = divider();
        c.elements.push(Element {
            id: Id(3),
            name: "M1".into(),
            kind: IrElementKind::Nmos,
            connections: vec![
                Connection {
                    terminal: "drain".into(),
                    net: Id(1),
                },
                Connection {
                    terminal: "gate".into(),
                    net: Id(2),
                },
                Connection {
                    terminal: "source".into(),
                    net: Id(0),
                },
                Connection {
                    terminal: "bulk".into(),
                    net: Id(0),
                },
            ],
            params: vec![],
            model: None,
            source_spec: None,
        });
        let result = assemble_mna_from_circuit(&c, false, None).unwrap();
        assert!(result.is_none());
    }

    /// A diode-bearing circuit must be accepted by the direct path now that
    /// Session C support has landed. The produced `MnaSystem` carries one
    /// `DiodeInstance` so downstream `solve_op_raw_with_opts` routes through
    /// `solve_nonlinear_op` via `has_nonlinear()`.
    #[test]
    fn diode_circuit_accepted_and_carries_instance() {
        let mut c = divider();
        c.elements.push(Element {
            id: Id(3),
            name: "D1".into(),
            kind: IrElementKind::Diode,
            connections: vec![
                Connection {
                    terminal: "anode".into(),
                    net: Id(2),
                },
                Connection {
                    terminal: "cathode".into(),
                    net: Id(0),
                },
            ],
            params: vec![],
            model: None,
            source_spec: None,
        });
        let mna = assemble_mna_from_circuit(&c, false, None)
            .unwrap()
            .expect("diode-bearing circuit");
        assert_eq!(mna.diodes.len(), 1);
        assert!(mna.has_nonlinear());
    }

    #[test]
    fn ground_named_gnd_is_excluded() {
        let mut c = divider();
        // Rename the "0" net to "gnd" — Cirq-source-compiled circuits use
        // this convention. The direct path must still recognise it as
        // ground and exclude it from the NodeMap.
        c.nets[0].name = "gnd".into();
        let mna = assemble_mna_from_circuit(&c, false, None)
            .unwrap()
            .expect("ok");
        assert!(mna.node_map.get("gnd").is_none());
        assert!(mna.node_map.get("0").is_none());
    }
}
