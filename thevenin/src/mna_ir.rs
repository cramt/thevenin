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

use cirq_frontend::to_netlist::{
    convert_model, convert_source_spec, extra_params, value_to_expr,
};
use cirq_ir::{
    BehavioralMode, Circuit, Element as IrElement, ElementKind as IrElementKind, Id, Model, Value,
    XspiceConnection as IrXspiceConnection,
};
use thevenin_types::{Expr, ModelDef, Source};
use thevenin_xspice::{
    CodeModelRegistry, ParamValue, PortConnection, PortDirection, PortType, XspiceInstance,
};

use crate::bjt::{BjtInstance, BjtModel};
use crate::bsim3::{Bsim3Instance, Bsim3Model};
use crate::cpl::{CplInstance, CplModel, setup_cpline};
use crate::expr_val_or;
use crate::ltra::{LtraInstance, LtraModel};
use crate::txl::{TxlInstance, TxlModel, setup_txline};
use crate::bsim3soi_dd::{Bsim3SoiDdInstance, Bsim3SoiDdModel};
use crate::bsim3soi_fd::{Bsim3SoiFdInstance, Bsim3SoiFdModel};
use crate::bsim3soi_pd::{Bsim3SoiPdInstance, Bsim3SoiPdModel};
use crate::bsim4::{Bsim4Instance, Bsim4Model};
use crate::diode::DiodeModel;
use crate::hfet::{HfetInstance, HfetModel, HfetPrecomp};
use crate::jfet::{JfetInstance, JfetModel};
use crate::mesa::{MesaInstance, MesaModel, MesaPrecomp};
use crate::mesfet::{MesfetInstance, MesfetModel};
use crate::mna::{
    BehavioralSourceInstance, BehavioralVoltageSourceInstance, CapacitorInstance,
    CurrentSourceInstance, DiodeInstance, InductorInstance, MnaError, MnaSystem,
    MutualCouplingInstance, NodeMap, ResistorInstance, VoltageSourceInstance,
    extract_resistor_noise_params, get_mosfet_level, get_mosfet_lw, get_nrd_nrs,
    parse_bsrc_params, push_bjt_caps, push_mosfet_caps, resolve_model_with_bins,
    resolve_resistor_value, stamp_conductance,
};
use crate::mos2::{Mos2Instance, Mos2Model};
use crate::mos6::{Mos6Instance, Mos6Model};
use crate::mosfet::{MosfetInstance, MosfetModel};
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
                | IrElementKind::Nmos
                | IrElementKind::Pmos
                | IrElementKind::NJfet
                | IrElementKind::PJfet
                | IrElementKind::NMesfet
                | IrElementKind::PMesfet
                | IrElementKind::Coupling
                | IrElementKind::BehavioralSource { .. }
                | IrElementKind::TransmissionLine
                | IrElementKind::Txl
                | IrElementKind::CoupledLine { .. }
                | IrElementKind::Xspice { .. }
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

/// Resolve a model reference stored in a `"model"` string param.
///
/// CPL and XSPICE elements store their model name as
/// `params: [("model", Value::String("LOSSYMODE"))]` rather than in the
/// typed `Element.model: Option<Id>` field. (The Netlist path looks up by
/// name string for these device classes.)
fn lookup_model_by_string_param<'a>(circuit: &'a Circuit, elem: &IrElement) -> Option<&'a Model> {
    let name = string_param(elem, "model")?;
    let upper = name.to_ascii_uppercase();
    circuit
        .models
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(&upper))
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
pub fn circuit_temp(circuit: &Circuit) -> f64 {
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

/// Circuit-side analogue of
/// [`crate::ac::collect_ac_excitations_from_netlist`].
///
/// Walks `circuit.elements`, picks up voltage/current sources with an AC
/// spec, and resolves them against `mna.vsource_names` (for V-sources) or
/// `mna.node_map` (for I-sources) into [`crate::ac::AcExcitation`]
/// records. The IR's `source_spec.ac: Option<AcSpec>` carries magnitude +
/// phase directly so no expression evaluation is needed.
pub fn collect_ac_excitations_from_circuit(
    circuit: &Circuit,
    mna: &MnaSystem,
    num_nodes: usize,
) -> Vec<crate::ac::AcExcitation> {
    use crate::ac::{AcExcitation, AcTarget};
    let net_name = build_net_name_map(circuit);
    let mut out = Vec::new();
    for elem in &circuit.elements {
        let Some(spec) = elem.source_spec.as_ref() else {
            continue;
        };
        let Some(ac) = spec.ac.as_ref() else {
            continue;
        };
        let phase_rad = ac.phase * std::f64::consts::PI / 180.0;
        let real = ac.mag * phase_rad.cos();
        let imag = ac.mag * phase_rad.sin();

        match elem.kind {
            IrElementKind::VoltageSource => {
                if let Some(branch_pos) = mna
                    .vsource_names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(&elem.name))
                {
                    out.push(AcExcitation {
                        target: AcTarget::VoltageBranch(num_nodes + branch_pos),
                        real,
                        imag,
                    });
                }
            }
            IrElementKind::CurrentSource => {
                let pos = terminal_name(elem, "pos", &net_name).ok();
                let neg = terminal_name(elem, "neg", &net_name).ok();
                let ni = pos.and_then(|n| mna.node_map.get(n));
                let nj = neg.and_then(|n| mna.node_map.get(n));
                out.push(AcExcitation {
                    target: AcTarget::CurrentInjection { ni, nj },
                    real,
                    imag,
                });
            }
            _ => {}
        }
    }
    out
}

/// Extract a fully-resolved `.ac` sweep parameter struct from a Circuit's
/// first declared `Analysis::Ac(AcAnalysis)`, with the AC source
/// excitations already collected against the assembled MnaSystem.
///
/// The IR `AcAnalysis` carries `start`/`stop` as Hz (no expressions) and a
/// `scale: FrequencyScale` enum that maps to Netlist's `AcVariation`.
pub fn ac_sweep_params_from_circuit(
    circuit: &Circuit,
    mna: &MnaSystem,
) -> Result<crate::ac::AcSweepRunParams, MnaError> {
    let ac = circuit
        .analyses
        .iter()
        .find_map(|a| match a {
            cirq_ir::Analysis::Ac(spec) => Some(spec),
            _ => None,
        })
        .ok_or_else(|| {
            MnaError::UnsupportedElement("no .ac analysis found on circuit".to_string())
        })?;

    let variation = match ac.scale {
        cirq_ir::FrequencyScale::Decade => thevenin_types::AcVariation::Dec,
        cirq_ir::FrequencyScale::Octave => thevenin_types::AcVariation::Oct,
        cirq_ir::FrequencyScale::Linear => thevenin_types::AcVariation::Lin,
    };
    let nr_opts = nr_options_from_circuit(circuit);
    let nodeset = resolve_nodeset_from_circuit(circuit, mna);
    let num_nodes = mna.total_num_nodes();
    let excitations = collect_ac_excitations_from_circuit(circuit, mna, num_nodes);

    Ok(crate::ac::AcSweepRunParams {
        variation,
        n: ac.points,
        fstart: ac.start,
        fstop: ac.stop,
        nr_opts,
        nodeset,
        excitations,
    })
}

/// Extract a fully-resolved `.tran` analysis parameter struct from a
/// Circuit's first declared `Analysis::Tran(TranAnalysis)`. `.ic` overrides
/// come from `circuit.initial_conditions` (Id → voltage pairs, pre-resolved
/// against `mna.node_map`), and `.print @device[param]` queries come from
/// `circuit.raw_directives` (which preserves verbatim SPICE directives via
/// the harness fix in
/// `docs/migration/cirq-harness-status.md`).
pub fn tran_params_from_circuit(
    circuit: &Circuit,
    mna: &MnaSystem,
) -> Result<crate::transient::TranRunParams, MnaError> {
    let tran = circuit
        .analyses
        .iter()
        .find_map(|a| match a {
            cirq_ir::Analysis::Tran(spec) => Some(spec),
            _ => None,
        })
        .ok_or_else(|| {
            MnaError::UnsupportedElement("no .tran analysis found on circuit".to_string())
        })?;

    // Resolve .ic overrides: IR carries (net Id, voltage). Map to matrix
    // indices via the net id → name → NodeMap lookup the same way
    // resolve_nodeset_from_circuit does.
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
    let mut ic_overrides = Vec::new();
    for (net_id, voltage) in &circuit.initial_conditions {
        if let Some(name) = net_lookup.get(net_id)
            && let Some(idx) = mna.node_map.get(name)
        {
            ic_overrides.push((idx, *voltage));
        }
    }

    let device_param_queries = crate::transient::collect_device_param_queries(
        circuit.raw_directives.iter().map(String::as_str),
        mna,
    );

    Ok(crate::transient::TranRunParams {
        t_step: tran.step,
        t_stop: tran.stop,
        t_start: tran.start,
        t_max: tran.tmax,
        uic: tran.uic,
        nr_opts: nr_options_from_circuit(circuit),
        nodeset: resolve_nodeset_from_circuit(circuit, mna),
        ic_overrides,
        device_param_queries,
        t_pause: None,
        start_state: None,
    })
}

/// Extract `(output, input)` for `.tf` analysis from a Circuit.
///
/// IR's [`cirq_ir::TfAnalysis`] stores `output` as a verbatim spec string
/// and `source` as an element [`Id`]; the source is resolved to an
/// element name for [`crate::tf::run_tf`].
pub fn tf_spec_from_circuit(circuit: &Circuit) -> Result<(String, String), MnaError> {
    let tf = circuit
        .analyses
        .iter()
        .find_map(|a| match a {
            cirq_ir::Analysis::Tf(spec) => Some(spec),
            _ => None,
        })
        .ok_or_else(|| {
            MnaError::UnsupportedElement("no .tf analysis found on circuit".to_string())
        })?;
    let input_name = element_name_by_id(circuit, tf.source)
        .ok_or_else(|| {
            MnaError::UnsupportedElement(format!(
                ".tf source references unknown element id {:?}",
                tf.source
            ))
        })?
        .to_string();
    Ok((tf.output.clone(), input_name))
}

/// Extract a fully-resolved [`crate::pz::PzRunParams`] from a Circuit's
/// first declared `Analysis::Pz(PzAnalysis)`. Net Ids are resolved to
/// their string names (with `gnd → 0` rewrite) and the typed enums map
/// onto the Netlist-shaped [`thevenin_types::PzInputType`] /
/// [`thevenin_types::PzAnalysisType`].
pub fn pz_params_from_circuit(circuit: &Circuit) -> Result<crate::pz::PzRunParams, MnaError> {
    use cirq_ir::{PzType, TransferType};
    let pz = circuit
        .analyses
        .iter()
        .find_map(|a| match a {
            cirq_ir::Analysis::Pz(spec) => Some(spec),
            _ => None,
        })
        .ok_or_else(|| {
            MnaError::UnsupportedElement("no .pz analysis found on circuit".to_string())
        })?;

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
    let resolve = |id: Id, role: &str| -> Result<String, MnaError> {
        net_lookup.get(&id).cloned().ok_or_else(|| {
            MnaError::UnsupportedElement(format!(
                ".pz {role} references unknown net id {id:?}"
            ))
        })
    };

    let input_type = match pz.transfer {
        TransferType::Voltage => thevenin_types::PzInputType::Vol,
        TransferType::Current => thevenin_types::PzInputType::Cur,
    };
    let analysis_type = match pz.analysis_type {
        PzType::Poles => thevenin_types::PzAnalysisType::Pol,
        PzType::Zeros => thevenin_types::PzAnalysisType::Zer,
        PzType::Both => thevenin_types::PzAnalysisType::Pz,
    };

    Ok(crate::pz::PzRunParams {
        node_i: resolve(pz.input_pos, "input_pos")?,
        node_g: resolve(pz.input_neg, "input_neg")?,
        node_j: resolve(pz.output_pos, "output_pos")?,
        node_k: resolve(pz.output_neg, "output_neg")?,
        input_type,
        analysis_type,
    })
}

/// Extract a fully-resolved [`crate::noise::NoiseRunParams`] from a Circuit's
/// first declared `Analysis::Noise(NoiseAnalysis)`. The IR carries typed
/// net / element Ids which are resolved to the SPICE-shaped string specs
/// (`"v(name,ref)"`, source name) that `crate::noise::run_noise` consumes.
pub fn noise_params_from_circuit(
    circuit: &Circuit,
    mna: &MnaSystem,
) -> Result<crate::noise::NoiseRunParams, MnaError> {
    let noise = circuit
        .analyses
        .iter()
        .find_map(|a| match a {
            cirq_ir::Analysis::Noise(spec) => Some(spec),
            _ => None,
        })
        .ok_or_else(|| {
            MnaError::UnsupportedElement("no .noise analysis found on circuit".to_string())
        })?;

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

    let out_name = net_lookup.get(&noise.output_net).cloned().ok_or_else(|| {
        MnaError::UnsupportedElement(format!(
            ".noise output references unknown net id {:?}",
            noise.output_net
        ))
    })?;
    let ref_name = net_lookup.get(&noise.reference_net).cloned();
    let output = match ref_name.as_ref() {
        Some(r) if r != "0" => format!("v({out_name},{r})"),
        _ => format!("v({out_name})"),
    };
    let ref_node = ref_name.filter(|r| r != "0");
    let src_name = element_name_by_id(circuit, noise.source)
        .ok_or_else(|| {
            MnaError::UnsupportedElement(format!(
                ".noise source references unknown element id {:?}",
                noise.source
            ))
        })?
        .to_string();
    let variation = match noise.scale {
        cirq_ir::FrequencyScale::Decade => thevenin_types::AcVariation::Dec,
        cirq_ir::FrequencyScale::Octave => thevenin_types::AcVariation::Oct,
        cirq_ir::FrequencyScale::Linear => thevenin_types::AcVariation::Lin,
    };
    let num_nodes = mna.total_num_nodes();
    let excitations = collect_ac_excitations_from_circuit(circuit, mna, num_nodes);

    Ok(crate::noise::NoiseRunParams {
        output,
        ref_node,
        src_name,
        variation,
        n: noise.points,
        fstart: noise.start,
        fstop: noise.stop,
        nr_opts: nr_options_from_circuit(circuit),
        nodeset: resolve_nodeset_from_circuit(circuit, mna),
        excitations,
    })
}

/// Extract a fully-resolved [`crate::sens::SensRunParams`] from a Circuit's
/// first declared `Analysis::Sens(SensAnalysis)`.
///
/// The IR `SensAnalysis` carries `output: String` (the single SPICE token
/// like `"v(out)"`) and an optional `SensAcSpec` with the AC sweep params.
/// AC excitations are collected against `mna` so the simulator's
/// [`crate::sens::run_sens`] is fully Netlist-free.
pub fn sens_params_from_circuit(
    circuit: &Circuit,
    mna: &MnaSystem,
) -> Result<crate::sens::SensRunParams, MnaError> {
    let sens = circuit
        .analyses
        .iter()
        .find_map(|a| match a {
            cirq_ir::Analysis::Sens(spec) => Some(spec),
            _ => None,
        })
        .ok_or_else(|| {
            MnaError::UnsupportedElement("no .sens analysis found on circuit".to_string())
        })?;

    let ac = sens.ac.as_ref().map(crate::sens::SensAcSpec::from_ir);
    let nr_opts = nr_options_from_circuit(circuit);
    let excitations = if ac.is_some() {
        collect_ac_excitations_from_circuit(circuit, mna, mna.total_num_nodes())
    } else {
        Vec::new()
    };
    let ckt_temp_k = circuit_temp(circuit) + 273.15;

    Ok(crate::sens::SensRunParams {
        output: sens.output.clone(),
        ac,
        nr_opts,
        excitations,
        ckt_temp_k,
    })
}

/// Resolve an element id to its name within `circuit.elements`.
///
/// Returns `None` when the id isn't in the table — callers handle that as
/// a missing-source error from the analysis routine.
pub fn element_name_by_id(circuit: &Circuit, id: Id) -> Option<&str> {
    circuit
        .elements
        .iter()
        .find(|e| e.id == id)
        .map(|e| e.name.as_str())
}

/// Extract a fully-resolved `.dc` sweep parameter struct from a Circuit's
/// first declared `Analysis::Dc(DcAnalysis)`. The IR stores sweep source
/// references as element [`Id`]s; this helper resolves them to the SPICE
/// element names that `crate::simulate::run_dc_sweep` looks up in
/// `mna.vsource_names` / `mna.current_sources`.
///
/// The first `DcSweep` in `DcAnalysis.sweeps` is the primary (inner)
/// sweep; an optional second entry becomes the outer (nested) sweep. SPICE
/// `.dc` only ever has up to two sweeps so anything beyond `[0..=1]` is
/// ignored.
pub fn dc_sweep_params_from_circuit(
    circuit: &Circuit,
) -> Result<crate::simulate::DcSweepRunParams, MnaError> {
    let dc_analysis = circuit
        .analyses
        .iter()
        .find_map(|a| match a {
            cirq_ir::Analysis::Dc(spec) => Some(spec),
            _ => None,
        })
        .ok_or_else(|| {
            MnaError::UnsupportedElement("no .dc analysis found on circuit".to_string())
        })?;

    let mut sweeps = dc_analysis.sweeps.iter();
    let sweep1 = sweeps.next().ok_or_else(|| {
        MnaError::UnsupportedElement(".dc analysis has no sweeps".to_string())
    })?;
    let src1_name = element_name_by_id(circuit, sweep1.source)
        .ok_or_else(|| {
            MnaError::UnsupportedElement(format!(
                ".dc sweep references unknown element id {:?}",
                sweep1.source
            ))
        })?
        .to_string();
    let src2 = if let Some(sweep2) = sweeps.next() {
        let name = element_name_by_id(circuit, sweep2.source)
            .ok_or_else(|| {
                MnaError::UnsupportedElement(format!(
                    ".dc second sweep references unknown element id {:?}",
                    sweep2.source
                ))
            })?
            .to_string();
        Some((name, sweep2.start, sweep2.stop, sweep2.step))
    } else {
        None
    };

    Ok(crate::simulate::DcSweepRunParams {
        src1: src1_name,
        start1: sweep1.start,
        stop1: sweep1.stop,
        step1: sweep1.step,
        src2,
    })
}

/// Circuit-side analogue of `crate::netlist_tnom(&Netlist) -> f64`.
///
/// Reads `TNOM` from `circuit.options` and returns Kelvin (default 300.15 K
/// when unset). Used by MESA temperature-precomputed parameters.
fn circuit_tnom(circuit: &Circuit) -> f64 {
    let mut tnom_c = 27.0_f64;
    for (name, value) in &circuit.options {
        if name.eq_ignore_ascii_case("TNOM")
            && let Some(v) = numeric_value(value)
        {
            tnom_c = v;
        }
    }
    tnom_c + 273.15
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

/// Owning storage for Netlist-shaped [`ModelDef`] values converted from the
/// circuit's IR models, plus the indexed lookup tables `assemble_mna_flat`
/// uses (exact-name `models` map and base-name `model_bins` map for
/// BSIM4-style W/L binning).
///
/// MOSFET stamping reuses the existing `resolve_model_with_bins` helper from
/// `crate::mna`, which works on `BTreeMap<String, &ModelDef>`. To keep the
/// lifetime story clean, the `models_by_name` / `bins_by_base` maps borrow
/// into `defs` — both live for the duration of `stamp_circuit`.
struct ModelTables {
    defs: Vec<(String, ModelDef)>,
}

impl ModelTables {
    fn build(circuit: &Circuit) -> Self {
        // Mirror `cirq_frontend::to_netlist::circuit_to_netlists` (lines 80-103):
        // skip synthetic aliases — empty-params models whose name is the
        // base of at least one `<name>.<digits>` sibling. The simulator's
        // `resolve_model_with_bins` already handles the base-name lookup;
        // the alias only exists so the IR element's `model: Option<Id>` has
        // somewhere to point.
        let all_names: std::collections::HashSet<String> = circuit
            .models
            .iter()
            .map(|m| m.name.to_ascii_uppercase())
            .collect();
        let alias_names: std::collections::HashSet<String> = circuit
            .models
            .iter()
            .filter(|m| m.params.is_empty())
            .map(|m| m.name.to_ascii_uppercase())
            .filter(|upper| {
                all_names.iter().any(|n| {
                    n.strip_prefix(&format!("{upper}."))
                        .map(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
                        .unwrap_or(false)
                })
            })
            .collect();

        let defs: Vec<(String, ModelDef)> = circuit
            .models
            .iter()
            .filter(|m| !alias_names.contains(&m.name.to_ascii_uppercase()))
            .map(|m| (m.name.to_ascii_uppercase(), convert_model(m)))
            .collect();
        Self { defs }
    }

    fn models_by_name(&self) -> BTreeMap<String, &ModelDef> {
        self.defs
            .iter()
            .map(|(name, def)| (name.clone(), def))
            .collect()
    }

    fn bins_by_base(&self) -> BTreeMap<String, Vec<&ModelDef>> {
        let mut bins: BTreeMap<String, Vec<&ModelDef>> = BTreeMap::new();
        for (upper, def) in &self.defs {
            if let Some(dot_pos) = upper.rfind('.') {
                let suffix = &upper[dot_pos + 1..];
                if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                    let base = upper[..dot_pos].to_string();
                    bins.entry(base).or_default().push(def);
                }
            }
        }
        bins
    }
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
    // MOSFET stamping needs the Netlist-shaped model + bins lookup that
    // `resolve_model_with_bins` operates on. Build it once up-front; both
    // passes borrow into `tables.defs`.
    let tables = ModelTables::build(circuit);
    let models_map = tables.models_by_name();
    let bins_map = tables.bins_by_base();

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
        match &elem.kind {
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
            IrElementKind::NJfet | IrElementKind::PJfet => {
                let d = terminal_name(elem, "drain", &net_name)?;
                let g = terminal_name(elem, "gate", &net_name)?;
                let s = terminal_name(elem, "source", &net_name)?;
                node_map.index(d);
                node_map.index(g);
                node_map.index(s);
                let jm = lookup_model(circuit, elem)
                    .map(|m| JfetModel::from_model_def(&convert_model(m)))
                    .unwrap_or_else(|| JfetModel::new(crate::jfet::JfetType::Njf));
                internal_node_count += jm.internal_node_count();
            }
            IrElementKind::BehavioralSource { mode, .. } => {
                let pos = terminal_name(elem, "pos", &net_name)?;
                let neg = terminal_name(elem, "neg", &net_name)?;
                node_map.index(pos);
                node_map.index(neg);
                // V= behavioural sources need a branch current variable; I=
                // injects current directly into the RHS.
                if matches!(mode, BehavioralMode::Voltage) {
                    vsource_offset_map.insert(elem.name.to_lowercase(), vsource_count);
                    vsource_count += 1;
                }
            }
            IrElementKind::Coupling => {
                // K-element: no nodes of its own and no branch — the post-
                // pass below resolves it against the inductor branches.
            }
            IrElementKind::TransmissionLine | IrElementKind::Txl => {
                for term in ["in_pos", "in_neg", "out_pos", "out_neg"] {
                    let n = terminal_name(elem, term, &net_name)?;
                    node_map.index(n);
                }
                // Both LTRA and TXL add 2 branch equations (one per port).
                vsource_count += 2;
            }
            IrElementKind::CoupledLine { width } => {
                for i in 0..*width {
                    let n = terminal_name(elem, &format!("in{i}"), &net_name)?;
                    node_map.index(n);
                }
                for i in 0..*width {
                    let n = terminal_name(elem, &format!("out{i}"), &net_name)?;
                    node_map.index(n);
                }
                // P-element ground terminal is indexed but stays ground.
                if elem.connections.iter().any(|cn| cn.terminal == "gnd") {
                    let g = terminal_name(elem, "gnd", &net_name)?;
                    node_map.index(g);
                }
                // CPL adds 2 branch equations per line.
                vsource_count += 2 * width;
            }
            IrElementKind::Xspice {
                connections: xspice_conns,
            } => {
                // XSPICE port allocation depends on the registry: skip the
                // whole element if no registry is supplied (matches the
                // Netlist path's silent backward-compat fallback). Models
                // come from elem.model (typed registry-name lookup) — or
                // from the element's own model id when present.
                let Some(registry) = xspice_registry.as_ref() else {
                    continue;
                };
                let model_type = lookup_model_by_string_param(circuit, elem)
                    .or_else(|| lookup_model(circuit, elem))
                    .map(|m| convert_model(m).kind.to_uppercase())
                    .unwrap_or_default();
                let Some(cm_def) = registry.get(&model_type) else {
                    // Defer the error to the second pass so unknown
                    // models produce a useful diagnostic rather than a
                    // silent skip.
                    continue;
                };
                for (ci, conn) in xspice_conns.iter().enumerate() {
                    if ci >= cm_def.ports.len() {
                        break;
                    }
                    match conn {
                        IrXspiceConnection::Scalar(id) => {
                            let name = net_name.get(id).map(String::as_str).ok_or_else(|| {
                                MnaError::UnsupportedElement(format!(
                                    "XSPICE `{}`: connection `{ci}` references unknown net id",
                                    elem.name
                                ))
                            })?;
                            node_map.index(name);
                        }
                        IrXspiceConnection::Array(ids) => {
                            for id in ids {
                                let name =
                                    net_name.get(id).map(String::as_str).ok_or_else(|| {
                                        MnaError::UnsupportedElement(format!(
                                            "XSPICE `{}`: array connection references unknown net id",
                                            elem.name
                                        ))
                                    })?;
                                node_map.index(name);
                            }
                        }
                    }
                }
                for port_def in &cm_def.ports {
                    if matches!(
                        (port_def.port_type, port_def.direction),
                        (PortType::Voltage, PortDirection::Out)
                            | (PortType::Current, PortDirection::In)
                    ) {
                        vsource_count += 1;
                    }
                }
            }
            IrElementKind::NMesfet | IrElementKind::PMesfet => {
                let d = terminal_name(elem, "drain", &net_name)?;
                let g = terminal_name(elem, "gate", &net_name)?;
                let s = terminal_name(elem, "source", &net_name)?;
                node_map.index(d);
                node_map.index(g);
                node_map.index(s);
                let model = lookup_model(circuit, elem);
                let mdef = model.map(convert_model);
                let kind = mdef.as_ref().map(|m| m.kind.to_uppercase());
                let params_nl = extra_params(elem, &["value"]);
                let level = get_mosfet_level(mdef.as_ref().as_ref(), &params_nl);
                match kind.as_deref() {
                    Some("NMF" | "PMF") if level == 1 => {
                        let mm = MesfetModel::from_model_def(mdef.as_ref().unwrap());
                        internal_node_count += mm.internal_node_count();
                    }
                    Some("NHFET" | "PHFET") => {
                        let mm = HfetModel::from_model_def_with_level(
                            mdef.as_ref().unwrap(),
                            level,
                        );
                        internal_node_count += mm.internal_node_count();
                    }
                    _ => {
                        let mm = mdef
                            .as_ref()
                            .map(MesaModel::from_model_def)
                            .unwrap_or_default();
                        internal_node_count += mm.internal_node_count();
                    }
                }
            }
            IrElementKind::Nmos | IrElementKind::Pmos => {
                let d = terminal_name(elem, "drain", &net_name)?;
                let g = terminal_name(elem, "gate", &net_name)?;
                let s = terminal_name(elem, "source", &net_name)?;
                let bulk = terminal_name(elem, "bulk", &net_name)?;
                node_map.index(d);
                node_map.index(g);
                node_map.index(s);
                node_map.index(bulk);
                let has_body = elem.connections.iter().any(|cn| cn.terminal == "body");
                if has_body {
                    let body = terminal_name(elem, "body", &net_name)?;
                    node_map.index(body);
                }

                let params_nl = extra_params(elem, &["value"]);
                let (inst_l, inst_w) = get_mosfet_lw(&params_nl);
                let (nrd, nrs) = get_nrd_nrs(&params_nl);
                let model_name = lookup_model(circuit, elem).map(|m| m.name.clone());
                let resolved = model_name.as_deref().and_then(|name| {
                    resolve_model_with_bins(&models_map, &bins_map, name, inst_l, inst_w)
                });
                let level = get_mosfet_level(resolved.as_ref(), &params_nl);

                if level == 8 || level == 49 {
                    let bm = resolved
                        .map(Bsim3Model::from_model_def)
                        .unwrap_or_else(|| Bsim3Model::new(crate::mosfet::MosfetType::Nmos));
                    internal_node_count += bm.internal_node_count(nrd, nrs);
                } else if level == 14 || level == 54 {
                    let bm = resolved
                        .map(Bsim4Model::from_model_def)
                        .unwrap_or_else(|| Bsim4Model::new(crate::mosfet::MosfetType::Nmos));
                    internal_node_count += bm.internal_node_count(nrd, nrs);
                } else if level == 56 {
                    let bm = resolved
                        .map(Bsim3SoiDdModel::from_model_def)
                        .unwrap_or_else(|| Bsim3SoiDdModel::new(crate::mosfet::MosfetType::Nmos));
                    internal_node_count += bm.internal_node_count(nrd, nrs);
                } else if level == 57 {
                    let bm = resolved
                        .map(Bsim3SoiPdModel::from_model_def)
                        .unwrap_or_else(|| Bsim3SoiPdModel::new(crate::mosfet::MosfetType::Nmos));
                    internal_node_count += bm.internal_node_count(nrd, nrs);
                } else if level == 55 {
                    let bm = resolved
                        .map(Bsim3SoiFdModel::from_model_def)
                        .unwrap_or_else(|| Bsim3SoiFdModel::new(crate::mosfet::MosfetType::Nmos));
                    internal_node_count += bm.internal_node_count_fd(nrd, nrs, has_body);
                } else if level == 2 {
                    let mm = resolved
                        .map(Mos2Model::from_model_def)
                        .unwrap_or_else(|| Mos2Model::new(crate::mosfet::MosfetType::Nmos));
                    internal_node_count += mm.internal_node_count();
                } else if level == 6 {
                    let mm = resolved
                        .map(Mos6Model::from_model_def)
                        .unwrap_or_else(|| Mos6Model::new(crate::mosfet::MosfetType::Nmos));
                    internal_node_count += mm.internal_node_count();
                } else {
                    let mm = resolved
                        .map(MosfetModel::from_model_def)
                        .unwrap_or_else(|| MosfetModel::new(crate::mosfet::MosfetType::Nmos));
                    internal_node_count += mm.internal_node_count();
                }
            }
        }
    }

    let n_nodes = node_map.len() + internal_node_count;
    let dim = n_nodes + vsource_count;
    let mut mna = MnaSystem::empty(dim, xspice_registry.clone());
    mna.node_map = node_map;
    let mut vsource_idx = 0usize;
    let mut internal_idx = mna.node_map.len();

    // -----------------------------------------------------------------
    // Second pass: stamp every element.
    // -----------------------------------------------------------------
    for elem in &circuit.elements {
        match &elem.kind {
            IrElementKind::Resistor => {
                let pos = terminal_name(elem, "pos", &net_name)?;
                let neg = terminal_name(elem, "neg", &net_name)?;

                // Project the IR value param into a Netlist-shaped Expr so
                // we can reuse the existing model-based resistor resolver
                // (handles `.model rmod r RSH=... NARROW=...` via L/W and
                // direct `r=` / `resistance=` model params). The Netlist
                // path's value field accepts Expr::Num, Expr::Param (model
                // name), or Expr::Brace; value_to_expr produces all three.
                let value_expr = elem
                    .params
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("value"))
                    .map(|(_, v)| value_to_expr(v))
                    .ok_or_else(|| {
                        MnaError::UnsupportedElement(format!(
                            "resistor `{}` missing `value` param",
                            elem.name
                        ))
                    })?;
                let instance_params = extra_params(elem, &["value"]);
                let r =
                    resolve_resistor_value(&value_expr, &elem.name, &instance_params, &models_map)?;
                let g = 1.0 / r;
                let pi = mna.node_map.get(pos);
                let ni = mna.node_map.get(neg);
                stamp_conductance(&mut mna.system.matrix, pi, ni, g);

                let ac_resistance = numeric_param(elem, &["ac"]);
                let m_val = numeric_param(elem, &["m"]).unwrap_or(1.0);
                let (kf, af, ef, noise_area) =
                    extract_resistor_noise_params(&value_expr, &instance_params, &models_map);
                mna.resistors.push(ResistorInstance {
                    name: elem.name.clone(),
                    pos_idx: pi,
                    neg_idx: ni,
                    resistance: r,
                    ac_resistance,
                    kf,
                    af,
                    ef,
                    noise_area,
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
                    pos_idx: pi,
                    neg_idx: ni,
                    name: elem.name.clone(),
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
            IrElementKind::BehavioralSource { mode, spec } => {
                let pos_idx = mna.node_map.get(terminal_name(elem, "pos", &net_name)?);
                let neg_idx = mna.node_map.get(terminal_name(elem, "neg", &net_name)?);

                // Parse tc1= / tc2= / reciproctc= out of the expression tail
                // exactly the way the Netlist path does.
                let params = parse_bsrc_params(spec.as_str());
                let expr_trimmed = params.expr.trim();
                let expr_clean = if let Some(inner) = expr_trimmed
                    .strip_prefix('{')
                    .and_then(|s| s.strip_suffix('}'))
                {
                    inner.trim()
                } else if let Some(inner) = expr_trimmed
                    .strip_prefix('\'')
                    .and_then(|s| s.strip_suffix('\''))
                {
                    inner.trim()
                } else {
                    expr_trimmed
                };
                let dt = circuit_temp(circuit) - 27.0;
                let raw_factor = 1.0 + params.tc1 * dt + params.tc2 * dt * dt;
                let tc_factor = if params.reciproc_tc {
                    1.0 / raw_factor
                } else {
                    raw_factor
                };

                match mode {
                    BehavioralMode::Current => {
                        mna.behavioral_sources.push(BehavioralSourceInstance {
                            pos_idx,
                            neg_idx,
                            expr: expr_clean.to_string(),
                            tc_factor,
                        });
                    }
                    BehavioralMode::Voltage => {
                        let branch = n_nodes + vsource_idx;
                        if let Some(i) = pos_idx {
                            mna.system.matrix.add(i, branch, 1.0);
                            mna.system.matrix.add(branch, i, 1.0);
                        }
                        if let Some(j) = neg_idx {
                            mna.system.matrix.add(j, branch, -1.0);
                            mna.system.matrix.add(branch, j, -1.0);
                        }
                        mna.behavioral_voltage_sources
                            .push(BehavioralVoltageSourceInstance {
                                expr: expr_clean.to_string(),
                                branch_idx: branch,
                                tc_factor,
                            });
                        mna.vsource_names.push(elem.name.clone());
                        vsource_idx += 1;
                    }
                }
            }
            IrElementKind::Coupling => {
                // K-element: deferred to the post-pass below — needs every
                // inductor's branch_idx and inductance already allocated.
            }
            IrElementKind::TransmissionLine => {
                let model = lookup_model(circuit, elem).ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "LTRA `{}` requires a `.model` reference",
                        elem.name
                    ))
                })?;
                let ltra_model = LtraModel::from_model_def(&convert_model(model));

                let pos1_idx = mna.node_map.get(terminal_name(elem, "in_pos", &net_name)?);
                let neg1_idx = mna.node_map.get(terminal_name(elem, "in_neg", &net_name)?);
                let pos2_idx = mna.node_map.get(terminal_name(elem, "out_pos", &net_name)?);
                let neg2_idx = mna.node_map.get(terminal_name(elem, "out_neg", &net_name)?);

                let br1 = vsource_idx;
                let br2 = vsource_idx + 1;
                vsource_idx += 2;
                mna.vsource_names
                    .push(format!("{}#branch1", elem.name.to_lowercase()));
                mna.vsource_names
                    .push(format!("{}#branch2", elem.name.to_lowercase()));

                mna.ltras.push(LtraInstance {
                    name: elem.name.clone(),
                    pos1_idx,
                    neg1_idx,
                    pos2_idx,
                    neg2_idx,
                    br_eq1: br1,
                    br_eq2: br2,
                    model: ltra_model,
                });
                // LTRA DC stamps are added separately in the DC solver
                // path — see `MnaSystem::stamp_ltra_dc_all`.
            }
            IrElementKind::Txl => {
                let model = lookup_model(circuit, elem).ok_or_else(|| {
                    MnaError::UnsupportedElement(format!(
                        "TXL `{}` requires a `.model` reference",
                        elem.name
                    ))
                })?;
                let txl_model = TxlModel::from_model_def(&convert_model(model));

                // Y-element uses (n1+, n2+) — the n1-/n2- terminals are
                // typically ground (ignored by the TXL stamps).
                let pos_idx = mna.node_map.get(terminal_name(elem, "in_pos", &net_name)?);
                let neg_idx = mna.node_map.get(terminal_name(elem, "out_pos", &net_name)?);

                let br1 = vsource_idx;
                let br2 = vsource_idx + 1;
                vsource_idx += 2;
                mna.vsource_names
                    .push(format!("{}#branch1", elem.name.to_lowercase()));
                mna.vsource_names
                    .push(format!("{}#branch2", elem.name.to_lowercase()));

                // Instance-level length override.
                let params_nl = extra_params(elem, &["value"]);
                let length = params_nl
                    .iter()
                    .find(|p| {
                        let u = p.name.to_uppercase();
                        u == "LEN" || u == "LENGTH"
                    })
                    .map_or(txl_model.length, |p| {
                        expr_val_or(&p.value, txl_model.length)
                    });

                let txline = setup_txline(&txl_model, length);
                let txline2 = txline.clone();

                mna.txls.push(TxlInstance {
                    name: elem.name.clone(),
                    pos_idx,
                    neg_idx,
                    ibr1: br1,
                    ibr2: br2,
                    model: txl_model,
                    txline,
                    txline2,
                    length,
                    dc_given: false,
                });
            }
            IrElementKind::CoupledLine { width } => {
                // CPL stores its model name in the `model` string param, not
                // in `Element.model: Option<Id>` — see cirq_spice_import's
                // SpiceElementKind::Cpl handler.
                let model = lookup_model_by_string_param(circuit, elem)
                    .or_else(|| lookup_model(circuit, elem))
                    .ok_or_else(|| {
                        MnaError::UnsupportedElement(format!(
                            "CPL `{}` requires a `.model` reference",
                            elem.name
                        ))
                    })?;
                let no_l = *width;
                let cpl_model = CplModel::from_model_def(&convert_model(model), no_l);

                let mut pos_nodes = Vec::with_capacity(no_l);
                let mut neg_nodes = Vec::with_capacity(no_l);
                for i in 0..no_l {
                    pos_nodes.push(mna.node_map.get(terminal_name(
                        elem,
                        &format!("in{i}"),
                        &net_name,
                    )?));
                }
                for i in 0..no_l {
                    neg_nodes.push(mna.node_map.get(terminal_name(
                        elem,
                        &format!("out{i}"),
                        &net_name,
                    )?));
                }

                let mut ibr1 = Vec::with_capacity(no_l);
                let mut ibr2 = Vec::with_capacity(no_l);
                for m in 0..no_l {
                    ibr1.push(vsource_idx);
                    vsource_idx += 1;
                    mna.vsource_names
                        .push(format!("{}#branch1_{}", elem.name.to_lowercase(), m));
                }
                for m in 0..no_l {
                    ibr2.push(vsource_idx);
                    vsource_idx += 1;
                    mna.vsource_names
                        .push(format!("{}#branch2_{}", elem.name.to_lowercase(), m));
                }

                let params_nl = extra_params(elem, &["value"]);
                let length = params_nl
                    .iter()
                    .find(|p| {
                        let u = p.name.to_uppercase();
                        u == "LEN" || u == "LENGTH"
                    })
                    .map_or(cpl_model.length, |p| {
                        expr_val_or(&p.value, cpl_model.length)
                    });

                let mut model_with_length = cpl_model.clone();
                model_with_length.length = length;
                let cpline = setup_cpline(&model_with_length);
                let cpline2 = cpline.clone();

                mna.cpls.push(CplInstance {
                    name: elem.name.clone(),
                    no_l,
                    pos_nodes,
                    neg_nodes,
                    ibr1,
                    ibr2,
                    model: model_with_length,
                    cpline,
                    cpline2,
                    dc_given: false,
                    length,
                });
            }
            IrElementKind::Xspice {
                connections: xspice_conns,
            } => {
                let Some(registry) = xspice_registry.as_ref() else {
                    continue;
                };
                let (model_type, model_params): (String, Vec<thevenin_types::Param>) =
                    match lookup_model_by_string_param(circuit, elem)
                        .or_else(|| lookup_model(circuit, elem))
                    {
                        Some(m) => {
                            let mdef = convert_model(m);
                            (mdef.kind.to_uppercase(), mdef.params)
                        }
                        None => (String::new(), Vec::new()),
                    };
                let cm_def = registry
                    .get(&model_type)
                    .ok_or_else(|| MnaError::XspiceModelNotFound(model_type.clone()))?;

                let mut port_connections = Vec::new();
                let mut branch_indices = Vec::new();
                let mut conn_iter = xspice_conns.iter();
                for (pi, port_def) in cm_def.ports.iter().enumerate() {
                    let conn = conn_iter.next().ok_or_else(|| MnaError::XspiceError {
                        instance: elem.name.clone(),
                        detail: format!("not enough connections for port `{}`", port_def.name),
                    })?;
                    let (pos_idx, neg_idx) = match conn {
                        IrXspiceConnection::Scalar(id) => {
                            let name = net_name.get(id).map(String::as_str).unwrap_or("0");
                            (mna.node_map.get(name), None)
                        }
                        IrXspiceConnection::Array(ids) => {
                            let resolve = |i: usize| {
                                ids.get(i).and_then(|id| {
                                    net_name
                                        .get(id)
                                        .and_then(|name| mna.node_map.get(name.as_str()))
                                })
                            };
                            (resolve(0), resolve(1))
                        }
                    };
                    let branch_idx = match (port_def.port_type, port_def.direction) {
                        (PortType::Voltage, PortDirection::Out)
                        | (PortType::Current, PortDirection::In) => {
                            let br = n_nodes + vsource_idx;
                            vsource_idx += 1;
                            mna.vsource_names
                                .push(format!("{}#{}", elem.name, port_def.name));
                            branch_indices.push(br);
                            Some(br)
                        }
                        _ => None,
                    };
                    port_connections.push(PortConnection {
                        port_def_index: pi,
                        pos_idx,
                        neg_idx,
                        branch_idx,
                    });
                }

                let params: Vec<ParamValue> = cm_def
                    .params
                    .iter()
                    .map(|pdef| {
                        model_params
                            .iter()
                            .find(|p| p.name.eq_ignore_ascii_case(&pdef.name))
                            .and_then(|p| {
                                if let thevenin_types::Expr::Num(v) = &p.value {
                                    match pdef.param_type {
                                        thevenin_xspice::ParamType::Real => {
                                            Some(ParamValue::Real(*v))
                                        }
                                        thevenin_xspice::ParamType::Integer => {
                                            Some(ParamValue::Integer(*v as i64))
                                        }
                                        thevenin_xspice::ParamType::Boolean => {
                                            Some(ParamValue::Boolean(*v != 0.0))
                                        }
                                        _ => None,
                                    }
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| pdef.default.clone())
                    })
                    .collect();

                let state = std::cell::RefCell::new(cm_def.create_state());
                mna.xspice_instances.push(XspiceInstance {
                    name: elem.name.clone(),
                    model_type,
                    port_connections,
                    params,
                    state,
                    branch_indices,
                });
            }
            IrElementKind::NJfet | IrElementKind::PJfet => {
                let drain_idx = mna.node_map.get(terminal_name(elem, "drain", &net_name)?);
                let gate_idx = mna.node_map.get(terminal_name(elem, "gate", &net_name)?);
                let source_idx = mna.node_map.get(terminal_name(elem, "source", &net_name)?);

                let jm = lookup_model(circuit, elem)
                    .map(|m| JfetModel::from_model_def(&convert_model(m)))
                    .unwrap_or_else(|| JfetModel::new(crate::jfet::JfetType::Njf));

                let drain_prime_idx = if jm.rd > 0.0 {
                    let idx = internal_idx;
                    internal_idx += 1;
                    Some(idx)
                } else {
                    drain_idx
                };
                let source_prime_idx = if jm.rs > 0.0 {
                    let idx = internal_idx;
                    internal_idx += 1;
                    Some(idx)
                } else {
                    source_idx
                };

                let mut area = 1.0;
                let mut m_mult = 1.0;
                for (name, value) in &elem.params {
                    if let Some(v) = numeric_value(value) {
                        match name.to_uppercase().as_str() {
                            "AREA" => area = v,
                            "M" => m_mult = v,
                            _ => {}
                        }
                    }
                }

                mna.jfets.push(JfetInstance {
                    name: elem.name.clone(),
                    drain_idx,
                    gate_idx,
                    source_idx,
                    drain_prime_idx,
                    source_prime_idx,
                    model: jm,
                    area,
                    m: m_mult,
                });
            }
            IrElementKind::NMesfet | IrElementKind::PMesfet => {
                let drain_idx = mna.node_map.get(terminal_name(elem, "drain", &net_name)?);
                let gate_idx = mna.node_map.get(terminal_name(elem, "gate", &net_name)?);
                let source_idx = mna.node_map.get(terminal_name(elem, "source", &net_name)?);

                let model = lookup_model(circuit, elem);
                let mdef = model.map(convert_model);
                let kind = mdef.as_ref().map(|m| m.kind.to_uppercase());
                let params_nl = extra_params(elem, &["value"]);
                let level = get_mosfet_level(mdef.as_ref().as_ref(), &params_nl);

                match kind.as_deref() {
                    Some("NMF" | "PMF") if level == 1 => {
                        let mm = MesfetModel::from_model_def(mdef.as_ref().unwrap());

                        let mut area = 1.0;
                        let mut m_mult = 1.0;
                        for (name, value) in &elem.params {
                            if let Some(v) = numeric_value(value) {
                                match name.to_uppercase().as_str() {
                                    "AREA" => area = v,
                                    "M" => m_mult = v,
                                    _ => {}
                                }
                            }
                        }

                        let drain_prime_idx = if mm.rd > 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            drain_idx
                        };
                        let source_prime_idx = if mm.rs > 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            source_idx
                        };

                        mna.mesfets.push(MesfetInstance {
                            name: elem.name.clone(),
                            drain_idx,
                            gate_idx,
                            source_idx,
                            drain_prime_idx,
                            source_prime_idx,
                            model: mm,
                            area,
                            m: m_mult,
                        });
                    }
                    Some("NHFET" | "PHFET") => {
                        let mm = HfetModel::from_model_def_with_level(
                            mdef.as_ref().unwrap(),
                            level,
                        );

                        let mut w = 10e-6;
                        let mut l = 1e-6;
                        for (name, value) in &elem.params {
                            if let Some(v) = numeric_value(value) {
                                match name.to_uppercase().as_str() {
                                    "W" => w = v,
                                    "L" => l = v,
                                    _ => {}
                                }
                            }
                        }

                        let drain_prime_idx = if mm.rd != 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            drain_idx
                        };
                        let source_prime_idx = if mm.rs != 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            source_idx
                        };
                        let gate_prime_idx = if mm.rg > 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            gate_idx
                        };
                        let drain_prm_prm_idx = if mm.rf > 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            drain_prime_idx
                        };
                        let source_prm_prm_idx = if mm.ri > 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            source_prime_idx
                        };

                        let pre = HfetPrecomp::compute(&mm, 300.15, 300.15, w, l);
                        mna.hfets.push(HfetInstance {
                            name: elem.name.clone(),
                            drain_idx,
                            gate_idx,
                            source_idx,
                            gate_prime_idx,
                            drain_prime_idx,
                            source_prime_idx,
                            drain_prm_prm_idx,
                            source_prm_prm_idx,
                            model: mm,
                            precomp: pre,
                            w,
                            l,
                        });
                    }
                    _ => {
                        // Generic MESA.
                        let mm = mdef
                            .as_ref()
                            .map(MesaModel::from_model_def)
                            .unwrap_or_default();

                        let mut w = 20e-6;
                        let mut l = 1e-6;
                        let mut ts_given: Option<f64> = None;
                        let mut td_given: Option<f64> = None;
                        let mut dtemp = 0.0_f64;
                        for (name, value) in &elem.params {
                            if let Some(v) = numeric_value(value) {
                                match name.to_uppercase().as_str() {
                                    "W" => w = v,
                                    "L" => l = v,
                                    "TS" => ts_given = Some(v + 273.15),
                                    "TD" => td_given = Some(v + 273.15),
                                    "DTEMP" => dtemp = v,
                                    _ => {}
                                }
                            }
                        }
                        let ckt_temp = circuit_temp(circuit) + 273.15;
                        let ts = ts_given.unwrap_or(ckt_temp + dtemp);
                        let td = td_given.unwrap_or(ckt_temp + dtemp);

                        let drain_prime_idx = if mm.rd > 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            drain_idx
                        };
                        let source_prime_idx = if mm.rs > 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            source_idx
                        };
                        let gate_prime_idx = if mm.rg > 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            gate_idx
                        };
                        let source_prm_prm_idx = if mm.ri > 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            source_prime_idx
                        };
                        let drain_prm_prm_idx = if mm.rf > 0.0 {
                            let idx = internal_idx;
                            internal_idx += 1;
                            Some(idx)
                        } else {
                            drain_prime_idx
                        };

                        let tnom = circuit_tnom(circuit);
                        let pre = MesaPrecomp::compute(&mm, ts, td, tnom, w, l);
                        mna.mesas.push(MesaInstance {
                            name: elem.name.clone(),
                            model: mm,
                            precomp: pre,
                            w,
                            l,
                            drain_idx,
                            gate_idx,
                            source_idx,
                            drain_prime_idx,
                            gate_prime_idx,
                            source_prime_idx,
                            source_prm_prm_idx,
                            drain_prm_prm_idx,
                        });
                    }
                }
            }
            IrElementKind::Nmos | IrElementKind::Pmos => {
                let drain_idx = mna.node_map.get(terminal_name(elem, "drain", &net_name)?);
                let gate_idx = mna.node_map.get(terminal_name(elem, "gate", &net_name)?);
                let source_idx = mna.node_map.get(terminal_name(elem, "source", &net_name)?);
                let bulk_idx = mna.node_map.get(terminal_name(elem, "bulk", &net_name)?);
                let body_idx = if elem.connections.iter().any(|cn| cn.terminal == "body") {
                    mna.node_map.get(terminal_name(elem, "body", &net_name)?)
                } else {
                    None
                };

                // Instance scalars (defaults match `mna::assemble_mna_flat`).
                let mut w = 1e-4;
                let mut l = 1e-4;
                let mut ad = 0.0;
                let mut as_ = 0.0;
                let mut pd = 0.0;
                let mut ps = 0.0;
                let mut m_mult = 1.0;
                let mut nrd = 0.0;
                let mut nrs = 0.0;
                for (name, value) in &elem.params {
                    if let Some(v) = numeric_value(value) {
                        match name.to_uppercase().as_str() {
                            "W" => w = v,
                            "L" => l = v,
                            "AD" => ad = v,
                            "AS" => as_ = v,
                            "PD" => pd = v,
                            "PS" => ps = v,
                            "M" => m_mult = v,
                            "NRD" => nrd = v,
                            "NRS" => nrs = v,
                            _ => {}
                        }
                    }
                }

                let params_nl = extra_params(elem, &["value"]);
                let model_name = lookup_model(circuit, elem).map(|m| m.name.clone());
                let resolved = model_name.as_deref().and_then(|name| {
                    resolve_model_with_bins(&models_map, &bins_map, name, l, w)
                });
                let level = get_mosfet_level(resolved.as_ref(), &params_nl);

                if level == 8 || level == 49 {
                    // BSIM3.
                    let bm = resolved
                        .map(Bsim3Model::from_model_def)
                        .unwrap_or_else(|| Bsim3Model::new(crate::mosfet::MosfetType::Nmos));

                    let drain_prime_idx = if bm.rsh > 0.0 && nrd > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        drain_idx
                    };
                    let source_prime_idx = if bm.rsh > 0.0 && nrs > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        source_idx
                    };

                    let size_params = bm.size_dep_param(w, l, 300.15);
                    let vth0_inst = size_params.vth0;
                    let vfb_inst = size_params.vfbzb
                        + size_params.phi
                        + size_params.k1 * size_params.sqrt_phi;
                    let vfbzb_inst = size_params.vfbzb;
                    mna.bsim3s.push(Bsim3Instance {
                        name: elem.name.clone(),
                        drain_idx,
                        gate_idx,
                        source_idx,
                        bulk_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        w,
                        l,
                        ad,
                        as_,
                        pd,
                        ps,
                        nrd,
                        nrs,
                        m: m_mult,
                        vth0_inst,
                        vfb_inst,
                        vfbzb_inst,
                        size_params,
                        model: bm,
                    });
                } else if level == 14 || level == 54 {
                    // BSIM4.
                    let bm = resolved
                        .map(Bsim4Model::from_model_def)
                        .unwrap_or_else(|| Bsim4Model::new(crate::mosfet::MosfetType::Nmos));

                    let drain_prime_idx = if (bm.rsh > 0.0 && nrd > 0.0) || bm.rdsmod != 0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        drain_idx
                    };
                    let source_prime_idx = if (bm.rsh > 0.0 && nrs > 0.0) || bm.rdsmod != 0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        source_idx
                    };

                    let mut nf = 1.0;
                    let mut sa = 0.0;
                    let mut sb = 0.0;
                    for (name, value) in &elem.params {
                        if let Some(v) = numeric_value(value) {
                            match name.to_uppercase().as_str() {
                                "NF" => nf = v,
                                "SA" => sa = v,
                                "SB" => sb = v,
                                _ => {}
                            }
                        }
                    }

                    let size_params = bm.size_dep_param(w, l, nf, 300.15);
                    mna.bsim4s.push(Bsim4Instance {
                        name: elem.name.clone(),
                        drain_idx,
                        gate_idx,
                        source_idx,
                        bulk_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        w,
                        l,
                        nf,
                        ad,
                        as_,
                        pd,
                        ps,
                        nrd,
                        nrs,
                        m: m_mult,
                        sa,
                        sb,
                        model: bm,
                        size_params,
                    });
                } else if level == 56 {
                    // BSIM3SOI-DD.
                    let bm = resolved
                        .map(Bsim3SoiDdModel::from_model_def)
                        .unwrap_or_else(|| Bsim3SoiDdModel::new(crate::mosfet::MosfetType::Nmos));

                    let drain_prime_idx = if bm.rbsh > 0.0 && nrd > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        drain_idx
                    };
                    let source_prime_idx = if bm.rbsh > 0.0 && nrs > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        source_idx
                    };
                    let body_int_idx = Some({
                        let idx = internal_idx;
                        internal_idx += 1;
                        idx
                    });
                    let mut nbc = 0.0;
                    for (name, value) in &elem.params {
                        if name.eq_ignore_ascii_case("NBC")
                            && let Some(v) = numeric_value(value)
                        {
                            nbc = v;
                        }
                    }

                    let size_params = bm.size_dep_param(w, l, 300.15);
                    let vth0_inst = size_params.vth0;
                    mna.bsim3soi_dds.push(Bsim3SoiDdInstance {
                        name: elem.name.clone(),
                        drain_idx,
                        gate_idx,
                        source_idx,
                        e_idx: bulk_idx,
                        body_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        body_int_idx,
                        w,
                        l,
                        m: m_mult,
                        nrd,
                        nrs,
                        model: bm,
                        size_params,
                        vth0_inst,
                        nbc,
                    });
                } else if level == 57 {
                    // BSIM3SOI-PD.
                    let bm = resolved
                        .map(Bsim3SoiPdModel::from_model_def)
                        .unwrap_or_else(|| Bsim3SoiPdModel::new(crate::mosfet::MosfetType::Nmos));

                    let drain_prime_idx = if bm.rbsh > 0.0 && nrd > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        drain_idx
                    };
                    let source_prime_idx = if bm.rbsh > 0.0 && nrs > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        source_idx
                    };
                    let body_int_idx = Some({
                        let idx = internal_idx;
                        internal_idx += 1;
                        idx
                    });
                    let mut nbc = 0.0;
                    for (name, value) in &elem.params {
                        if name.eq_ignore_ascii_case("NBC")
                            && let Some(v) = numeric_value(value)
                        {
                            nbc = v;
                        }
                    }

                    let size_params = bm.size_dep_param(w, l, 300.15);
                    let vth0_inst = size_params.vth0;
                    mna.bsim3soi_pds.push(Bsim3SoiPdInstance {
                        name: elem.name.clone(),
                        drain_idx,
                        gate_idx,
                        source_idx,
                        e_idx: bulk_idx,
                        body_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        body_int_idx,
                        w,
                        l,
                        m: m_mult,
                        nrd,
                        nrs,
                        model: bm,
                        size_params,
                        vth0_inst,
                        nbc,
                    });
                } else if level == 55 {
                    // BSIM3SOI-FD. Body internal node only when body
                    // contact exists (floating-body sets bNode = ground in
                    // ngspice b3soifdset.c).
                    let bm = resolved
                        .map(Bsim3SoiFdModel::from_model_def)
                        .unwrap_or_else(|| Bsim3SoiFdModel::new(crate::mosfet::MosfetType::Nmos));

                    let has_ext_rd = nrd > 0.0 && bm.rbsh > 0.0;
                    let has_ext_rs = nrs > 0.0 && bm.rbsh > 0.0;
                    let drain_prime_idx = if has_ext_rd {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        drain_idx
                    };
                    let source_prime_idx = if has_ext_rs {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        source_idx
                    };
                    let body_int_idx = if body_idx.is_some() {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        None
                    };
                    let mut nbc = 0.0;
                    for (name, value) in &elem.params {
                        if name.eq_ignore_ascii_case("NBC")
                            && let Some(v) = numeric_value(value)
                        {
                            nbc = v;
                        }
                    }

                    let size_params = bm.size_dep_param(w, l, 300.15);
                    let vth0_inst = size_params.vth0;
                    mna.bsim3soi_fds.push(Bsim3SoiFdInstance {
                        name: elem.name.clone(),
                        drain_idx,
                        gate_idx,
                        source_idx,
                        e_idx: bulk_idx,
                        body_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        body_int_idx,
                        w,
                        l,
                        m: m_mult,
                        nrd,
                        nrs,
                        model: bm,
                        size_params,
                        vth0_inst,
                        nbc,
                    });
                } else if level == 2 {
                    // MOS Level 2.
                    let mm = resolved
                        .map(Mos2Model::from_model_def)
                        .unwrap_or_else(|| Mos2Model::new(crate::mosfet::MosfetType::Nmos));
                    let drain_prime_idx = if mm.rd > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        drain_idx
                    };
                    let source_prime_idx = if mm.rs > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        source_idx
                    };
                    mna.mos2s.push(Mos2Instance {
                        name: elem.name.clone(),
                        drain_idx,
                        gate_idx,
                        source_idx,
                        bulk_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        model: mm.clone(),
                        w,
                        l,
                        ad,
                        as_,
                        pd,
                        ps,
                        m: m_mult,
                    });
                    push_mosfet_caps(
                        &mut mna.capacitors,
                        gate_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        bulk_idx,
                        mm.cgso,
                        mm.cgdo,
                        mm.cgbo,
                        mm.cbd,
                        mm.cbs,
                        mm.cj,
                        mm.mj,
                        mm.cjsw,
                        mm.mjsw,
                        mm.pb,
                        mm.fc,
                        w,
                        l,
                        ad,
                        as_,
                        pd,
                        ps,
                        m_mult,
                    );
                } else if level == 6 {
                    // MOS6.
                    let mm = resolved
                        .map(Mos6Model::from_model_def)
                        .unwrap_or_else(|| Mos6Model::new(crate::mosfet::MosfetType::Nmos));
                    let drain_prime_idx = if mm.rd > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        drain_idx
                    };
                    let source_prime_idx = if mm.rs > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        source_idx
                    };
                    mna.mos6s.push(Mos6Instance {
                        name: elem.name.clone(),
                        drain_idx,
                        gate_idx,
                        source_idx,
                        bulk_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        model: mm.clone(),
                        w,
                        l,
                        ad,
                        as_,
                        pd,
                        ps,
                        m: m_mult,
                    });
                    push_mosfet_caps(
                        &mut mna.capacitors,
                        gate_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        bulk_idx,
                        mm.cgso,
                        mm.cgdo,
                        mm.cgbo,
                        mm.cbd,
                        mm.cbs,
                        mm.cj,
                        mm.mj,
                        mm.cjsw,
                        mm.mjsw,
                        mm.pb,
                        mm.fc,
                        w,
                        l,
                        ad,
                        as_,
                        pd,
                        ps,
                        m_mult,
                    );
                } else {
                    // MOS Level 1 (default).
                    let mm = resolved
                        .map(MosfetModel::from_model_def)
                        .unwrap_or_else(|| MosfetModel::new(crate::mosfet::MosfetType::Nmos));
                    let drain_prime_idx = if mm.rd > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        drain_idx
                    };
                    let source_prime_idx = if mm.rs > 0.0 {
                        let idx = internal_idx;
                        internal_idx += 1;
                        Some(idx)
                    } else {
                        source_idx
                    };
                    mna.mosfets.push(MosfetInstance {
                        name: elem.name.clone(),
                        drain_idx,
                        gate_idx,
                        source_idx,
                        bulk_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        model: mm.clone(),
                        w,
                        l,
                        ad,
                        as_,
                        pd,
                        ps,
                        m: m_mult,
                    });
                    push_mosfet_caps(
                        &mut mna.capacitors,
                        gate_idx,
                        drain_prime_idx,
                        source_prime_idx,
                        bulk_idx,
                        mm.cgso,
                        mm.cgdo,
                        mm.cgbo,
                        mm.cbd,
                        mm.cbs,
                        mm.cj,
                        mm.mj,
                        mm.cjsw,
                        mm.mjsw,
                        mm.pb,
                        mm.fc,
                        w,
                        l,
                        ad,
                        as_,
                        pd,
                        ps,
                        m_mult,
                    );
                }
                // MOSFET conductance/current stamps are applied during NR
                // iteration, not here.
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
        }
    }

    debug_assert_eq!(vsource_idx, vsource_count);

    // Post-pass: resolve Coupling (K-elements) now that every inductor has
    // a `branch_idx` and inductance assigned. Mirrors `assemble_mna_flat`'s
    // mutual_couplings_raw resolution.
    for elem in &circuit.elements {
        if !matches!(elem.kind, IrElementKind::Coupling) {
            continue;
        }
        let l1_name = string_param(elem, "l1").ok_or_else(|| {
            MnaError::UnsupportedElement(format!(
                "Coupling `{}` missing `l1` inductor reference",
                elem.name
            ))
        })?;
        let l2_name = string_param(elem, "l2").ok_or_else(|| {
            MnaError::UnsupportedElement(format!(
                "Coupling `{}` missing `l2` inductor reference",
                elem.name
            ))
        })?;
        let k = numeric_param(elem, &["coupling", "value"]).ok_or_else(|| {
            MnaError::UnsupportedElement(format!(
                "Coupling `{}` missing numeric `coupling`",
                elem.name
            ))
        })?;

        let l1_offset = vsource_offset_map
            .get(&l1_name.to_lowercase())
            .copied()
            .ok_or_else(|| {
                MnaError::UnsupportedElement(format!(
                    "Coupling `{}` references unknown inductor `{l1_name}`",
                    elem.name
                ))
            })?;
        let l2_offset = vsource_offset_map
            .get(&l2_name.to_lowercase())
            .copied()
            .ok_or_else(|| {
                MnaError::UnsupportedElement(format!(
                    "Coupling `{}` references unknown inductor `{l2_name}`",
                    elem.name
                ))
            })?;

        let branch1 = n_nodes + l1_offset;
        let branch2 = n_nodes + l2_offset;

        let (ind1_vec_idx, l1_val) = mna
            .inductors
            .iter()
            .enumerate()
            .find(|(_, ind)| ind.branch_idx == branch1)
            .map(|(idx, ind)| (idx, ind.inductance))
            .ok_or_else(|| {
                MnaError::UnsupportedElement(format!(
                    "Coupling `{}`: inductor `{l1_name}` not found in instances",
                    elem.name
                ))
            })?;
        let (ind2_vec_idx, l2_val) = mna
            .inductors
            .iter()
            .enumerate()
            .find(|(_, ind)| ind.branch_idx == branch2)
            .map(|(idx, ind)| (idx, ind.inductance))
            .ok_or_else(|| {
                MnaError::UnsupportedElement(format!(
                    "Coupling `{}`: inductor `{l2_name}` not found in instances",
                    elem.name
                ))
            })?;

        let factor = k * (l1_val * l2_val).abs().sqrt();
        mna.mutual_couplings.push(MutualCouplingInstance {
            branch1_idx: branch1,
            branch2_idx: branch2,
            ind1_vec_idx,
            ind2_vec_idx,
            factor,
        });
    }

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
