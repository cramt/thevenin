use std::collections::BTreeMap;
use std::sync::Arc;

use thevenin_types::Expr;
use thevenin_xspice::{CodeModelRegistry, XspiceInstance};
use thiserror::Error;

use crate::LinearSystem;
use crate::bjt::{BjtInstance, BjtModel};
use crate::bsim3::Bsim3Instance;
use crate::bsim4::Bsim4Instance;
use crate::diode::DiodeModel;
use crate::jfet::JfetInstance;
use crate::mosfet::MosfetInstance;
use crate::vbic::VbicInstance;

/// Ground node name — the reference node excluded from the MNA matrix.
const GROUND: &str = "0";

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum MnaError {
    #[error("unsupported element for MNA assembly: {0}")]
    UnsupportedElement(String),
    #[error("non-numeric value in element {element}: parameter expressions not yet supported")]
    NonNumericValue { element: String },
    #[error("voltage source {0} has no DC value")]
    NoVoltageValue(String),
    #[error("failed to solve MNA system: {0}")]
    SolveError(#[from] crate::SparseMatrixError),
    #[error("subcircuit expansion error: {0}")]
    SubcktError(#[from] crate::subckt::SubcktError),
    #[error("expression error in {element}: {detail}")]
    ExprError { element: String, detail: String },
    #[error("XSPICE model type '{0}' not found in registry")]
    XspiceModelNotFound(String),
    #[error("XSPICE instance '{instance}': {detail}")]
    XspiceError { instance: String, detail: String },
}

/// Maps node names to matrix indices. Ground node "0" is excluded.
#[derive(Debug, Clone)]
pub struct NodeMap {
    /// node name -> matrix index
    map: BTreeMap<String, usize>,
}

impl NodeMap {
    pub(crate) fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    /// Get or assign an index for a node. Returns `None` for ground.
    pub(crate) fn index(&mut self, node: &str) -> Option<usize> {
        if node == GROUND {
            return None;
        }
        let next = self.map.len();
        Some(*self.map.entry(node.to_string()).or_insert(next))
    }

    /// Number of non-ground nodes.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the NodeMap has no non-ground nodes.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Look up a node's index (returns None for ground or unknown nodes).
    /// SPICE is case-insensitive, so this does a case-insensitive lookup.
    pub fn get(&self, node: &str) -> Option<usize> {
        self.map.get(node).copied().or_else(|| {
            let upper = node.to_uppercase();
            self.map
                .iter()
                .find(|(k, _)| k.to_uppercase() == upper)
                .map(|(_, &v)| v)
        })
    }

    /// Iterate over (node_name, index) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, usize)> {
        self.map.iter().map(|(k, &v)| (k.as_str(), v))
    }
}

/// A resolved resistor instance with matrix indices (for noise analysis).
#[derive(Debug, Clone)]
pub struct ResistorInstance {
    /// Resistor element name.
    pub name: String,
    /// Positive node matrix index (None = ground).
    pub pos_idx: Option<usize>,
    /// Negative node matrix index (None = ground).
    pub neg_idx: Option<usize>,
    /// Resistance value in Ohms.
    pub resistance: f64,
    /// AC resistance value in Ohms (from `ac=` parameter), if different from DC.
    pub ac_resistance: Option<f64>,
    /// Flicker noise coefficient (model parameter KF, default 0).
    pub kf: f64,
    /// Flicker noise exponent (model parameter AF, default 1).
    pub af: f64,
    /// Frequency exponent (model parameter EF, default 1).
    pub ef: f64,
    /// Effective noise area in m² (from L, W, short, narrow, lf, wf).
    pub noise_area: f64,
    /// Instance multiplier.
    pub m: f64,
}

/// A resolved diode instance with matrix indices and model parameters.
#[derive(Debug, Clone)]
pub struct DiodeInstance {
    /// Anode node matrix index (None = ground).
    pub anode_idx: Option<usize>,
    /// Cathode node matrix index (None = ground).
    pub cathode_idx: Option<usize>,
    /// Internal node index when RS > 0 (between RS and junction).
    pub internal_idx: Option<usize>,
    /// Resolved diode model parameters.
    pub model: DiodeModel,
}

/// A resolved capacitor instance with matrix indices.
#[derive(Debug, Clone)]
pub struct CapacitorInstance {
    /// Element name (`Some` for user-defined, `None` for device-internal parasitics).
    pub name: Option<String>,
    /// Positive node matrix index (None = ground).
    pub pos_idx: Option<usize>,
    /// Negative node matrix index (None = ground).
    pub neg_idx: Option<usize>,
    /// Capacitance value in Farads.
    pub capacitance: f64,
    /// Initial condition voltage (from IC= parameter), if specified.
    pub ic: Option<f64>,
}

/// A resolved inductor instance with matrix indices.
#[derive(Debug, Clone)]
pub struct InductorInstance {
    /// Element name (`Some` for user-defined, `None` for device-internal).
    pub name: Option<String>,
    /// Positive node matrix index (None = ground).
    pub pos_idx: Option<usize>,
    /// Negative node matrix index (None = ground).
    pub neg_idx: Option<usize>,
    /// Index of this inductor's branch current in the solution vector.
    pub branch_idx: usize,
    /// Inductance value in Henrys.
    pub inductance: f64,
    /// Initial condition current (from IC= parameter), if specified.
    pub ic: Option<f64>,
}

/// A resolved mutual coupling (K-element) instance linking two inductors.
#[derive(Debug, Clone)]
pub struct MutualCouplingInstance {
    /// Branch index of the first coupled inductor.
    pub branch1_idx: usize,
    /// Branch index of the second coupled inductor.
    pub branch2_idx: usize,
    /// Index into the `inductors` vec for the first inductor.
    pub ind1_vec_idx: usize,
    /// Index into the `inductors` vec for the second inductor.
    pub ind2_vec_idx: usize,
    /// Mutual inductance factor: M = k * sqrt(L1 * L2).
    pub factor: f64,
}

/// A resolved voltage source instance with matrix indices and waveform.
#[derive(Debug, Clone)]
pub struct VoltageSourceInstance {
    /// Index of this source's branch equation in the RHS vector.
    pub branch_idx: usize,
    /// Positive terminal matrix index (None = ground).
    pub pos_idx: Option<usize>,
    /// Negative terminal matrix index (None = ground).
    pub neg_idx: Option<usize>,
    /// Source name (matches the corresponding entry in `vsource_names`).
    pub name: String,
    /// Transient waveform, if any.
    pub waveform: Option<thevenin_types::Waveform>,
}

/// A resolved current source instance with matrix indices and waveform.
#[derive(Debug, Clone)]
pub struct CurrentSourceInstance {
    /// Element name.
    pub name: String,
    /// Positive node matrix index (None = ground).
    pub pos_idx: Option<usize>,
    /// Negative node matrix index (None = ground).
    pub neg_idx: Option<usize>,
    /// DC value.
    pub dc_value: f64,
    /// Transient waveform, if any.
    pub waveform: Option<thevenin_types::Waveform>,
}

/// A resolved behavioral source (B-element) instance.
#[derive(Debug, Clone)]
pub struct BehavioralSourceInstance {
    /// Positive terminal node index (None = ground).
    pub pos_idx: Option<usize>,
    /// Negative terminal node index (None = ground).
    pub neg_idx: Option<usize>,
    /// Expression string after `I=`.
    pub expr: String,
    /// Temperature coefficient scaling factor: (1 + tc1*dT + tc2*dT²).
    pub tc_factor: f64,
}

/// A resolved behavioral voltage source (B-element with V=expr) instance.
#[derive(Debug, Clone)]
pub struct BehavioralVoltageSourceInstance {
    /// Expression string after `V=`.
    pub expr: String,
    /// Branch current variable index in the solution vector.
    pub branch_idx: usize,
    /// Temperature coefficient scaling factor: (1 + tc1*dT + tc2*dT²).
    pub tc_factor: f64,
}

/// The assembled MNA system ready for solving.
#[derive(Debug)]
pub struct MnaSystem {
    /// The linear system (matrix + RHS).
    pub system: LinearSystem,
    /// Mapping from node names to matrix indices (first N entries of solution).
    pub node_map: NodeMap,
    /// Names of voltage sources whose branch currents appear in the solution
    /// (entries N..N+M of solution vector).
    pub vsource_names: Vec<String>,
    /// Resolved resistor instances (for noise analysis).
    pub resistors: Vec<ResistorInstance>,
    /// Resolved diode instances for NR iteration.
    pub diodes: Vec<DiodeInstance>,
    /// Resolved capacitor instances for transient analysis.
    pub capacitors: Vec<CapacitorInstance>,
    /// Resolved inductor instances for transient analysis.
    pub inductors: Vec<InductorInstance>,
    /// Resolved mutual coupling (K-element) instances for transient/AC analysis.
    pub mutual_couplings: Vec<MutualCouplingInstance>,
    /// Resolved BJT instances for NR iteration.
    pub bjts: Vec<BjtInstance>,
    /// Capacitor indices for each BJT's depletion caps (CJE, CJC).
    pub bjt_cap_indices: Vec<BjtCapIndices>,
    /// Resolved MOSFET instances for NR iteration.
    pub mosfets: Vec<MosfetInstance>,
    /// Resolved MOS Level 2 instances for NR iteration.
    pub mos2s: Vec<crate::mos2::Mos2Instance>,
    /// Resolved MOS Level 3 instances for NR iteration.
    pub mos3s: Vec<crate::mos3::Mos3Instance>,
    /// Resolved BSIM1 (MOSFET LEVEL=4) instances for NR iteration.
    pub bsim1s: Vec<crate::bsim1::Bsim1Instance>,
    /// Resolved BSIM2 (level 5) instances for NR iteration.
    pub bsim2s: Vec<crate::bsim2::Bsim2Instance>,
    /// Resolved MOS6 MOSFET instances for NR iteration.
    pub mos6s: Vec<crate::mos6::Mos6Instance>,
    /// Resolved HiSIM2 (LEVEL=68) MOSFET instances for NR iteration.
    pub hisims: Vec<crate::hisim::HisimInstance>,
    /// Resolved VDMOS power-MOSFET instances for NR iteration.
    pub vdmoses: Vec<crate::vdmos::VdmosInstance>,
    /// Resolved JFET instances for NR iteration.
    pub jfets: Vec<JfetInstance>,
    /// Resolved MESA FET instances for NR iteration.
    pub mesas: Vec<crate::mesa::MesaInstance>,
    /// Resolved MESFET (MES) instances for NR iteration.
    pub mesfets: Vec<crate::mesfet::MesfetInstance>,
    /// Resolved HFET instances for NR iteration.
    pub hfets: Vec<crate::hfet::HfetInstance>,
    /// Resolved BSIM3 MOSFET instances for NR iteration.
    pub bsim3s: Vec<Bsim3Instance>,
    /// Resolved BSIM3SOI-DD MOSFET instances for NR iteration.
    pub bsim3soi_dds: Vec<crate::bsim3soi_dd::Bsim3SoiDdInstance>,
    /// Resolved BSIM3SOI-FD MOSFET instances for NR iteration.
    pub bsim3soi_fds: Vec<crate::bsim3soi_fd::Bsim3SoiFdInstance>,
    /// Resolved BSIM3SOI-PD MOSFET instances for NR iteration.
    pub bsim3soi_pds: Vec<crate::bsim3soi_pd::Bsim3SoiPdInstance>,
    /// Resolved BSIM4 MOSFET instances for NR iteration.
    pub bsim4s: Vec<Bsim4Instance>,
    /// Resolved VBIC BJT instances (LEVEL=4) for NR iteration.
    pub vbics: Vec<VbicInstance>,
    /// Resolved LTRA (lossy transmission line) instances.
    pub ltras: Vec<crate::ltra::LtraInstance>,
    /// Resolved TXL (single lossy transmission line) instances.
    pub txls: Vec<crate::txl::TxlInstance>,
    /// Resolved CPL (coupled multiconductor transmission line) instances.
    pub cpls: Vec<crate::cpl::CplInstance>,
    /// Resolved ideal lossless transmission line (T element) instances.
    pub tlines: Vec<crate::tline::TlineInstance>,
    /// Resolved voltage source instances (for transient waveform evaluation).
    pub voltage_sources: Vec<VoltageSourceInstance>,
    /// Resolved current source instances (for transient waveform evaluation).
    pub current_sources: Vec<CurrentSourceInstance>,
    /// Resolved behavioral current source (B-element with I=) instances for NR iteration.
    pub behavioral_sources: Vec<BehavioralSourceInstance>,
    /// Resolved behavioral voltage source (B-element with V=) instances for NR iteration.
    pub behavioral_voltage_sources: Vec<BehavioralVoltageSourceInstance>,
    /// Resolved XSPICE code model instances.
    pub xspice_instances: Vec<XspiceInstance>,
    /// XSPICE code model registry (shared across instances).
    pub xspice_registry: Option<Arc<CodeModelRegistry>>,
    /// Resolved voltage- and current-controlled switch instances
    /// (SPICE S / W elements). Stamped through the NR loop because
    /// conductance depends nonlinearly on the control variable.
    pub switches: Vec<crate::switch::SwitchInstance>,
}

impl MnaSystem {
    /// Construct an empty `MnaSystem` with a `LinearSystem` of the given
    /// dimension and every device-instance vec initialised to empty.
    ///
    /// Used by `crate::mna_ir` (the Stage 4 direct IR → MNA path) to
    /// avoid duplicating the long field list on every call. Callers
    /// fill `node_map`, `vsource_names`, the matrix/RHS, and whichever
    /// device-instance vecs are relevant after construction.
    pub fn empty(dim: usize, xspice_registry: Option<Arc<CodeModelRegistry>>) -> Self {
        Self {
            system: crate::LinearSystem::new(dim),
            node_map: NodeMap::new(),
            vsource_names: Vec::new(),
            resistors: Vec::new(),
            diodes: Vec::new(),
            capacitors: Vec::new(),
            inductors: Vec::new(),
            mutual_couplings: Vec::new(),
            bjts: Vec::new(),
            bjt_cap_indices: Vec::new(),
            mosfets: Vec::new(),
            mos2s: Vec::new(),
            mos3s: Vec::new(),
            bsim1s: Vec::new(),
            bsim2s: Vec::new(),
            mos6s: Vec::new(),
            hisims: Vec::new(),
            vdmoses: Vec::new(),
            jfets: Vec::new(),
            mesas: Vec::new(),
            mesfets: Vec::new(),
            hfets: Vec::new(),
            bsim3s: Vec::new(),
            bsim3soi_dds: Vec::new(),
            bsim3soi_fds: Vec::new(),
            bsim3soi_pds: Vec::new(),
            bsim4s: Vec::new(),
            vbics: Vec::new(),
            ltras: Vec::new(),
            txls: Vec::new(),
            cpls: Vec::new(),
            tlines: Vec::new(),
            voltage_sources: Vec::new(),
            current_sources: Vec::new(),
            behavioral_sources: Vec::new(),
            behavioral_voltage_sources: Vec::new(),
            xspice_instances: Vec::new(),
            xspice_registry,
            switches: Vec::new(),
        }
    }

    /// Solve the MNA system, returning an `MnaSolution`.
    pub fn solve(&self) -> Result<MnaSolution<'_>, MnaError> {
        let x = self.system.solve()?;
        let n = self.node_map.len();
        Ok(MnaSolution {
            values: x,
            num_nodes: n,
            node_map: &self.node_map,
            vsource_names: &self.vsource_names,
        })
    }

    /// Returns `true` if the circuit contains any nonlinear devices requiring
    /// Newton-Raphson iteration.
    pub fn has_nonlinear(&self) -> bool {
        !self.diodes.is_empty()
            || !self.bjts.is_empty()
            || !self.mosfets.is_empty()
            || !self.mos2s.is_empty()
            || !self.mos3s.is_empty()
            || !self.bsim1s.is_empty()
            || !self.bsim2s.is_empty()
            || !self.mos6s.is_empty()
            || !self.hisims.is_empty()
            || !self.vdmoses.is_empty()
            || !self.jfets.is_empty()
            || !self.bsim3s.is_empty()
            || !self.bsim3soi_dds.is_empty()
            || !self.bsim3soi_fds.is_empty()
            || !self.bsim3soi_pds.is_empty()
            || !self.bsim4s.is_empty()
            || !self.vbics.is_empty()
            || !self.mesas.is_empty()
            || !self.mesfets.is_empty()
            || !self.hfets.is_empty()
            || !self.behavioral_sources.is_empty()
            || !self.behavioral_voltage_sources.is_empty()
            || !self.xspice_instances.is_empty()
            || !self.switches.is_empty()
    }

    /// Total number of nodes including internal nodes created by nonlinear
    /// device series resistances (RS, RB, RC, RE, RD).
    pub fn total_num_nodes(&self) -> usize {
        self.node_map.len()
            + self
                .diodes
                .iter()
                .filter(|d| d.internal_idx.is_some())
                .count()
            + self
                .bjts
                .iter()
                .map(|b| b.model.internal_node_count())
                .sum::<usize>()
            + self
                .mosfets
                .iter()
                .map(|m| m.model.internal_node_count())
                .sum::<usize>()
            + self
                .mos3s
                .iter()
                .map(|m| m.model.internal_node_count())
                .sum::<usize>()
            + self
                .bsim1s
                .iter()
                .map(|m| m.model.internal_node_count(m.nrd, m.nrs))
                .sum::<usize>()
            + self
                .mos6s
                .iter()
                .map(|m| m.model.internal_node_count())
                .sum::<usize>()
            + self
                .hisims
                .iter()
                .map(|m| m.model.internal_node_count())
                .sum::<usize>()
            + self
                .vdmoses
                .iter()
                .map(|m| m.model.internal_node_count())
                .sum::<usize>()
            + self
                .jfets
                .iter()
                .map(|j| j.model.internal_node_count())
                .sum::<usize>()
            + self
                .bsim3s
                .iter()
                .filter(|b| b.drain_prime_idx.is_some() && b.drain_prime_idx != b.drain_idx)
                .count()
            + self
                .bsim3s
                .iter()
                .filter(|b| b.source_prime_idx.is_some() && b.source_prime_idx != b.source_idx)
                .count()
            + self
                .bsim4s
                .iter()
                .filter(|b| b.drain_prime_idx.is_some() && b.drain_prime_idx != b.drain_idx)
                .count()
            + self
                .bsim4s
                .iter()
                .filter(|b| b.source_prime_idx.is_some() && b.source_prime_idx != b.source_idx)
                .count()
            + self
                .bsim3soi_pds
                .iter()
                .filter(|b| b.drain_prime_idx.is_some() && b.drain_prime_idx != b.drain_idx)
                .count()
            + self
                .bsim3soi_pds
                .iter()
                .filter(|b| b.source_prime_idx.is_some() && b.source_prime_idx != b.source_idx)
                .count()
            + self
                .bsim3soi_pds
                .iter()
                .filter(|b| b.body_int_idx.is_some())
                .count()
            + self
                .bsim3soi_dds
                .iter()
                .filter(|b| b.drain_prime_idx.is_some() && b.drain_prime_idx != b.drain_idx)
                .count()
            + self
                .bsim3soi_dds
                .iter()
                .filter(|b| b.source_prime_idx.is_some() && b.source_prime_idx != b.source_idx)
                .count()
            + self
                .bsim3soi_dds
                .iter()
                .filter(|b| b.body_int_idx.is_some())
                .count()
            + self
                .bsim3soi_fds
                .iter()
                .filter(|b| b.drain_prime_idx.is_some() && b.drain_prime_idx != b.drain_idx)
                .count()
            + self
                .bsim3soi_fds
                .iter()
                .filter(|b| b.source_prime_idx.is_some() && b.source_prime_idx != b.source_idx)
                .count()
            + self
                .bsim3soi_fds
                .iter()
                .filter(|b| b.body_int_idx.is_some())
                .count()
            + self
                .vbics
                .iter()
                .map(|v| v.model.internal_node_count())
                .sum::<usize>()
            + self
                .mesas
                .iter()
                .map(|m| m.model.internal_node_count())
                .sum::<usize>()
            + self
                .mesfets
                .iter()
                .map(|m| m.model.internal_node_count())
                .sum::<usize>()
            + self
                .hfets
                .iter()
                .map(|m| m.model.internal_node_count())
                .sum::<usize>()
    }

    /// Query a device instance parameter from the current solution vector.
    ///
    /// Supports `@device[param]` syntax from `.print` directives.
    /// Currently handles SOI MOSFET terminal voltages: `vbs`, `vgs`, `vds`, `ves`.
    pub fn query_device_param(&self, device: &str, param: &str, solution: &[f64]) -> Option<f64> {
        let param_lower = param.to_lowercase();
        // BSIM3SOI-DD instances
        for inst in &self.bsim3soi_dds {
            if inst.name.eq_ignore_ascii_case(device) {
                let (vgs, vds, vbs, ves) = inst.terminal_voltages(solution);
                return match param_lower.as_str() {
                    "vbs" => Some(vbs),
                    "vgs" => Some(vgs),
                    "vds" => Some(vds),
                    "ves" => Some(ves),
                    _ => None,
                };
            }
        }
        // BSIM3SOI-FD instances
        for inst in &self.bsim3soi_fds {
            if inst.name.eq_ignore_ascii_case(device) {
                let (vgs, vds, vbs, ves) = inst.terminal_voltages(solution);
                return match param_lower.as_str() {
                    "vbs" => Some(vbs),
                    "vgs" => Some(vgs),
                    "vds" => Some(vds),
                    "ves" => Some(ves),
                    _ => None,
                };
            }
        }
        // BSIM3SOI-PD instances
        for inst in &self.bsim3soi_pds {
            if inst.name.eq_ignore_ascii_case(device) {
                let (vgs, vds, vbs, ves) = inst.terminal_voltages(solution);
                return match param_lower.as_str() {
                    "vbs" => Some(vbs),
                    "vgs" => Some(vgs),
                    "vds" => Some(vds),
                    "ves" => Some(ves),
                    _ => None,
                };
            }
        }
        None
    }

    /// Stamp LTRA DC equations into the given linear system.
    /// Called by DC solver paths (not stored in the base matrix so that
    /// transient can use different convolution-based stamps).
    pub fn stamp_ltra_dc_all(&self, system: &mut crate::LinearSystem) {
        let n = self.total_num_nodes();
        for inst in &self.ltras {
            crate::ltra::stamp_ltra_dc(inst, system, n);
        }
    }

    /// Stamp TXL DC equations into the given linear system.
    pub fn stamp_txl_dc_all(&self, system: &mut crate::LinearSystem) {
        let n = self.total_num_nodes();
        for inst in &self.txls {
            crate::txl::stamp_txl_dc(inst, system, n);
        }
    }

    /// Stamp T-line (ideal lossless line) DC equations.
    ///
    /// Mirrors the LTRA / TXL / CPL pattern: kept out of the base matrix so
    /// transient and AC paths can write their own stamps without conflict.
    pub fn stamp_tline_dc_all(&self, system: &mut crate::LinearSystem) {
        let n = self.total_num_nodes();
        for inst in &self.tlines {
            crate::tline::stamp_tline_dc(inst, system, n);
        }
    }

    pub fn stamp_cpl_dc_all(&self, system: &mut crate::LinearSystem) {
        let n = self.total_num_nodes();
        for inst in &self.cpls {
            crate::cpl::stamp_cpl_dc(inst, system, n);
        }
    }
}

/// Solution of an MNA system with named access to node voltages and branch currents.
#[derive(Debug)]
pub struct MnaSolution<'a> {
    values: Vec<f64>,
    num_nodes: usize,
    node_map: &'a NodeMap,
    vsource_names: &'a [String],
}

impl MnaSolution<'_> {
    /// Get the voltage at a node by name. Ground returns 0.0.
    pub fn voltage(&self, node: &str) -> Option<f64> {
        if node == GROUND {
            return Some(0.0);
        }
        self.node_map.get(node).map(|i| self.values[i])
    }

    /// Get the branch current through a voltage source by name.
    pub fn branch_current(&self, vsource: &str) -> Option<f64> {
        let vsource_lower = vsource.to_lowercase();
        self.vsource_names
            .iter()
            .position(|n| n.to_lowercase() == vsource_lower)
            .map(|i| self.values[self.num_nodes + i])
    }
}

/// Extract a numeric value from an `Expr`, or return an error.
fn expr_value(expr: &Expr, element_name: &str) -> Result<f64, MnaError> {
    match expr {
        Expr::Num(v) => Ok(*v),
        _ => Err(MnaError::NonNumericValue {
            element: element_name.to_string(),
        }),
    }
}

/// Resolve a resistor value, supporting model-referenced resistors.
///
/// When the value is a model name (e.g. `r1 2 0 my` where `.model my r r=2k`),
/// Resolve a resistor's DC resistance value.
///
/// Handles: numeric value, model-name reference (with RSH+L/W or R= params),
/// `m` (multiplicity) and `scale` instance parameters.
pub(crate) fn resolve_resistor_value(
    value: &Expr,
    element_name: &str,
    params: &[thevenin_types::Param],
    models: &std::collections::BTreeMap<String, &thevenin_types::ModelDef>,
) -> Result<f64, MnaError> {
    // Helper: extract a numeric param by name from a list
    fn get_num(list: &[thevenin_types::Param], name: &str) -> Option<f64> {
        list.iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .and_then(|p| {
                if let Expr::Num(v) = &p.value {
                    Some(*v)
                } else {
                    None
                }
            })
    }

    let base_r = match value {
        Expr::Num(v) => *v,
        Expr::Param(name) => {
            // Try to look up as a resistor model
            if let Some(mdef) = models.get(&name.to_uppercase())
                && mdef.kind.eq_ignore_ascii_case("r")
            {
                // Look for explicit r= or resistance= in model
                for p in &mdef.params {
                    if p.name.eq_ignore_ascii_case("r") || p.name.eq_ignore_ascii_case("resistance")
                    {
                        return Ok(apply_multipliers(
                            expr_value(&p.value, element_name)?,
                            params,
                        ));
                    }
                }
                // Compute from RSH + L/W (sheet resistance model)
                let rsh = get_num(&mdef.params, "rsh").unwrap_or(0.0);
                if rsh != 0.0 {
                    let l = get_num(params, "l").unwrap_or(0.0);
                    let w = get_num(params, "w").unwrap_or(1.0);
                    let narrow = get_num(&mdef.params, "narrow").unwrap_or(0.0);
                    let w_eff = (w - narrow).max(1e-30);
                    if l > 0.0 {
                        let r = rsh * l / w_eff;
                        return Ok(apply_multipliers(r, params));
                    }
                    // No L/W given — model exists, default to 0 (short)
                    return Ok(0.0);
                }
                return Err(MnaError::NonNumericValue {
                    element: element_name.to_string(),
                });
            }
            return Err(MnaError::NonNumericValue {
                element: element_name.to_string(),
            });
        }
        _ => {
            return Err(MnaError::NonNumericValue {
                element: element_name.to_string(),
            });
        }
    };
    Ok(apply_multipliers(base_r, params))
}

/// Extract resistor flicker noise parameters (KF, AF, EF, effective noise area)
/// from the resistor model definition and instance parameters.
pub(crate) fn extract_resistor_noise_params(
    value: &Expr,
    params: &[thevenin_types::Param],
    models: &std::collections::BTreeMap<String, &thevenin_types::ModelDef>,
) -> (f64, f64, f64, f64) {
    fn get_num(list: &[thevenin_types::Param], name: &str) -> Option<f64> {
        list.iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .and_then(|p| {
                if let Expr::Num(v) = &p.value {
                    Some(*v)
                } else {
                    None
                }
            })
    }

    // Model name is either the value itself (when it's a model reference) or
    // a separate "model" param (when a numeric value + model name are both given).
    let model_name = match value {
        Expr::Param(name) => Some(name.to_uppercase()),
        _ => params
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case("model"))
            .and_then(|p| {
                if let Expr::Param(name) = &p.value {
                    Some(name.to_uppercase())
                } else {
                    None
                }
            }),
    };

    let mdef = model_name.and_then(|n| models.get(&n));

    if let Some(mdef) = mdef {
        let kf = get_num(&mdef.params, "kf").unwrap_or(0.0);
        let af = get_num(&mdef.params, "af").unwrap_or(1.0);
        let ef = get_num(&mdef.params, "ef").unwrap_or(1.0);
        // Length/width corrections for noise area (short=dlr, narrow=dw).
        let short = get_num(&mdef.params, "short")
            .or_else(|| get_num(&mdef.params, "dlr"))
            .unwrap_or(0.0);
        let narrow = get_num(&mdef.params, "narrow")
            .or_else(|| get_num(&mdef.params, "dw"))
            .unwrap_or(0.0);
        let lf = get_num(&mdef.params, "lf").unwrap_or(1.0);
        let wf = get_num(&mdef.params, "wf").unwrap_or(1.0);
        // Instance dimensions.
        let l_inst = get_num(params, "l").unwrap_or(0.0);
        let w_inst = get_num(params, "w").unwrap_or(0.0);
        let l_eff = (l_inst - 2.0 * short).max(1e-30);
        let w_eff = (w_inst - 2.0 * narrow).max(1e-30);
        let noise_area = if l_inst > 0.0 && w_inst > 0.0 {
            l_eff.powf(lf) * w_eff.powf(wf)
        } else {
            1.0 // No dimensions → default area (noise formula degenerates)
        };
        (kf, af, ef, noise_area)
    } else {
        (0.0, 1.0, 1.0, 1.0)
    }
}

/// Apply `m` (multiplicity) and `scale` instance parameters to a resistance.
fn apply_multipliers(r: f64, params: &[thevenin_types::Param]) -> f64 {
    let m = params
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case("m"))
        .and_then(|p| {
            if let Expr::Num(v) = &p.value {
                Some(*v)
            } else {
                None
            }
        })
        .unwrap_or(1.0);
    let scale = params
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case("scale"))
        .and_then(|p| {
            if let Expr::Num(v) = &p.value {
                Some(*v)
            } else {
                None
            }
        })
        .unwrap_or(1.0);
    // m parallel resistors: R_eff = R / m; scale multiplies the resistance
    r * scale / m
}

/// Extract the MOSFET LEVEL parameter from model definition and instance params.
/// Checks instance params first (override), then model params. Default is 1.
pub(crate) fn get_mosfet_level(
    model_def: Option<&&thevenin_types::ModelDef>,
    instance_params: &[thevenin_types::Param],
) -> i32 {
    // Check instance params first
    for p in instance_params {
        if p.name.eq_ignore_ascii_case("LEVEL")
            && let Expr::Num(v) = &p.value
        {
            return *v as i32;
        }
    }
    // Then model params
    if let Some(mdef) = model_def {
        for p in &mdef.params {
            if p.name.eq_ignore_ascii_case("LEVEL")
                && let Expr::Num(v) = &p.value
            {
                return *v as i32;
            }
        }
    }
    1 // default level
}

/// Extract NRD and NRS from instance params.
pub(crate) fn get_nrd_nrs(params: &[thevenin_types::Param]) -> (f64, f64) {
    let mut nrd = 0.0;
    let mut nrs = 0.0;
    for p in params {
        if let Expr::Num(v) = &p.value {
            match p.name.to_uppercase().as_str() {
                "NRD" => nrd = *v,
                "NRS" => nrs = *v,
                _ => {}
            }
        }
    }
    (nrd, nrs)
}

/// Extract MOSFET L and W from instance parameters, falling back to
/// `def_l` and `def_w` when the instance omits them. Callers should pass
/// the `.options DEFL / DEFW` values (defaults 1e-4 / 1e-4, matching
/// ngspice cktinit.c).
pub(crate) fn get_mosfet_lw(
    params: &[thevenin_types::Param],
    def_l: f64,
    def_w: f64,
) -> (f64, f64) {
    let mut l = def_l;
    let mut w = def_w;
    for p in params {
        if let Expr::Num(v) = &p.value {
            match p.name.to_uppercase().as_str() {
                "L" => l = *v,
                "W" => w = *v,
                _ => {}
            }
        }
    }
    (l, w)
}

/// Extract bin boundary parameters (LMIN, LMAX, WMIN, WMAX) from a model definition.
fn extract_bin_bounds(mdef: &thevenin_types::ModelDef) -> (f64, f64, f64, f64) {
    let mut lmin = f64::NEG_INFINITY;
    let mut lmax = f64::INFINITY;
    let mut wmin = f64::NEG_INFINITY;
    let mut wmax = f64::INFINITY;
    for p in &mdef.params {
        if let Expr::Num(v) = &p.value {
            match p.name.to_uppercase().as_str() {
                "LMIN" => lmin = *v,
                "LMAX" => lmax = *v,
                "WMIN" => wmin = *v,
                "WMAX" => wmax = *v,
                _ => {}
            }
        }
    }
    (lmin, lmax, wmin, wmax)
}

/// Resolve a MOSFET model name, supporting BSIM4-style model binning.
///
/// If no exact match for `name` exists in `models`, looks for binned models
/// (`name.1`, `name.2`, etc.) in `model_bins` and picks the bin whose
/// LMIN/LMAX/WMIN/WMAX range includes the given device L and W.
pub(crate) fn resolve_model_with_bins<'a>(
    models: &BTreeMap<String, &'a thevenin_types::ModelDef>,
    model_bins: &BTreeMap<String, Vec<&'a thevenin_types::ModelDef>>,
    name: &str,
    l: f64,
    w: f64,
) -> Option<&'a thevenin_types::ModelDef> {
    let upper = name.to_uppercase();
    // Exact match first.
    if let Some(m) = models.get(&upper) {
        return Some(m);
    }
    // Binned model lookup: find the bin where L ∈ [LMIN, LMAX] and W ∈ [WMIN, WMAX].
    if let Some(bins) = model_bins.get(&upper) {
        for mdef in bins {
            let (lmin, lmax, wmin, wmax) = extract_bin_bounds(mdef);
            if l >= lmin && l <= lmax && w >= wmin && w <= wmax {
                return Some(mdef);
            }
        }
        // Fallback: return first bin if no range matched.
        return bins.first().copied();
    }
    None
}

/// Capacitor indices for a BJT's junction capacitances.
/// `None` means the cap was zero and not created.
#[derive(Debug, Clone, Default)]
pub struct BjtCapIndices {
    /// Index of CJE capacitor in the capacitors array.
    pub cje_idx: Option<usize>,
    /// Index of CJC capacitor in the capacitors array.
    pub cjc_idx: Option<usize>,
}

/// Generate synthetic `CapacitorInstance` entries for BJT junction
/// capacitances (CJE, CJC, CJS).
///
/// CJE and CJC are stamped as constant zero-bias capacitors here. During
/// transient analysis, their values are updated to voltage-dependent values
/// at each accepted timestep, and diffusion capacitance (TF*gbe, TR*gbc)
/// is added dynamically.
///
/// Returns `BjtCapIndices` with the indices of the created caps.
pub(crate) fn push_bjt_caps(
    capacitors: &mut Vec<CapacitorInstance>,
    base_prime_idx: Option<usize>,
    col_prime_idx: Option<usize>,
    emit_prime_idx: Option<usize>,
    model: &BjtModel,
    area: f64,
    m: f64,
) -> BjtCapIndices {
    let mut indices = BjtCapIndices::default();
    // B-E junction capacitance: CJE * area
    let cje_total = model.cje * area * m;
    if cje_total > 0.0 {
        indices.cje_idx = Some(capacitors.len());
        capacitors.push(CapacitorInstance {
            name: None,
            pos_idx: base_prime_idx,
            neg_idx: emit_prime_idx,
            capacitance: cje_total,
            ic: None,
        });
    }

    // B-C junction capacitance: CJC * area
    let cjc_total = model.cjc * area * m;
    if cjc_total > 0.0 {
        indices.cjc_idx = Some(capacitors.len());
        capacitors.push(CapacitorInstance {
            name: None,
            pos_idx: base_prime_idx,
            neg_idx: col_prime_idx,
            capacitance: cjc_total,
            ic: None,
        });
    }

    // Collector-substrate capacitance: CJS * area (to ground)
    let cjs_total = model.cjs * area * m;
    if cjs_total > 0.0 {
        capacitors.push(CapacitorInstance {
            name: None,
            pos_idx: col_prime_idx,
            neg_idx: None, // substrate = ground
            capacitance: cjs_total,
            ic: None,
        });
    }

    indices
}

/// Generate synthetic `CapacitorInstance` entries for MOSFET overlap and
/// junction capacitances.
///
/// Overlap caps (CGSO, CGDO, CGBO) are constant-valued capacitors between
/// gate-source, gate-drain, and gate-bulk terminals.  Junction caps (CBD, CBS)
/// are voltage-dependent in ngspice (depletion model), but we approximate them
/// as constant capacitors at their zero-bias value to provide the necessary
/// conductive paths during transient analysis.  This prevents singular matrices
/// when internal nodes are only connected through MOSFET junction capacitances.
///
/// ngspice computes area-scaled junction caps as:
///   Cbd_total = CJ * AD + CJSW * PD  (if AD/PD given)
///   Cbd_total = CBD                    (if CBD given directly)
/// We follow the same priority: use CJ*area + CJSW*perimeter when area > 0,
/// otherwise fall back to the CBD/CBS model parameters.
#[expect(clippy::too_many_arguments)]
pub(crate) fn push_mosfet_caps(
    capacitors: &mut Vec<CapacitorInstance>,
    gate_idx: Option<usize>,
    drain_prime_idx: Option<usize>,
    source_prime_idx: Option<usize>,
    bulk_idx: Option<usize>,
    cgso: f64,
    cgdo: f64,
    cgbo: f64,
    cbd: f64,
    cbs: f64,
    cj: f64,
    _mj: f64,
    cjsw: f64,
    _mjsw: f64,
    _pb: f64,
    _fc: f64,
    w: f64,
    l: f64,
    ad: f64,
    as_: f64,
    pd: f64,
    ps: f64,
    m: f64,
) {
    // Gate-source overlap capacitance: CGSO * W
    let cgs_ov = cgso * w * m;
    if cgs_ov > 0.0 {
        capacitors.push(CapacitorInstance {
            name: None,
            pos_idx: gate_idx,
            neg_idx: source_prime_idx,
            capacitance: cgs_ov,
            ic: None,
        });
    }

    // Gate-drain overlap capacitance: CGDO * W
    let cgd_ov = cgdo * w * m;
    if cgd_ov > 0.0 {
        capacitors.push(CapacitorInstance {
            name: None,
            pos_idx: gate_idx,
            neg_idx: drain_prime_idx,
            capacitance: cgd_ov,
            ic: None,
        });
    }

    // Gate-bulk overlap capacitance: CGBO * L
    let cgb_ov = cgbo * l * m;
    if cgb_ov > 0.0 {
        capacitors.push(CapacitorInstance {
            name: None,
            pos_idx: gate_idx,
            neg_idx: bulk_idx,
            capacitance: cgb_ov,
            ic: None,
        });
    }

    // Bulk-drain junction capacitance (zero-bias value).
    // Priority: CJ*AD + CJSW*PD if AD > 0, else CBD directly.
    let cbd_total = if ad > 0.0 || pd > 0.0 {
        (cj * ad + cjsw * pd) * m
    } else {
        cbd * m
    };
    if cbd_total > 0.0 {
        capacitors.push(CapacitorInstance {
            name: None,
            pos_idx: bulk_idx,
            neg_idx: drain_prime_idx,
            capacitance: cbd_total,
            ic: None,
        });
    }

    // Bulk-source junction capacitance (zero-bias value).
    let cbs_total = if as_ > 0.0 || ps > 0.0 {
        (cj * as_ + cjsw * ps) * m
    } else {
        cbs * m
    };
    if cbs_total > 0.0 {
        capacitors.push(CapacitorInstance {
            name: None,
            pos_idx: bulk_idx,
            neg_idx: source_prime_idx,
            capacitance: cbs_total,
            ic: None,
        });
    }
}

/// Parsed B-source parameters extracted from the spec string.
pub(crate) struct BsrcParams<'a> {
    pub(crate) expr: &'a str,
    pub(crate) tc1: f64,
    pub(crate) tc2: f64,
    /// When true, use 1/tc_factor (behavioral resistor: tc scales resistance, not current).
    pub(crate) reciproc_tc: bool,
}

/// Parse a B-source expression string, extracting tc1/tc2/reciproctc parameters.
///
/// Input like `"v(1) tc1=0.001 tc2=1e-6"` returns expr="v(1)", tc1=0.001, tc2=1e-6.
/// Parameters not present default to 0.0 / false.
pub(crate) fn parse_bsrc_params(raw: &str) -> BsrcParams<'_> {
    let mut tc1 = 0.0;
    let mut tc2 = 0.0;
    let mut reciproc_tc = false;
    // Find the earliest tc1=, tc2=, or reciproctc= to split off the expression.
    let lower = raw.to_lowercase();
    let param_start = lower
        .find("tc1=")
        .into_iter()
        .chain(lower.find("tc2="))
        .chain(lower.find("reciproctc="))
        .min();
    let expr_end = param_start.unwrap_or(raw.len());
    // Parse parameters from the remainder
    if let Some(start) = param_start {
        let remainder = &raw[start..];
        for part in remainder.split_whitespace() {
            let part_lower = part.to_lowercase();
            if let Some(val_str) = part_lower.strip_prefix("tc1=") {
                tc1 = val_str.parse().unwrap_or(0.0);
            } else if let Some(val_str) = part_lower.strip_prefix("tc2=") {
                tc2 = val_str.parse().unwrap_or(0.0);
            } else if let Some(val_str) = part_lower.strip_prefix("reciproctc=") {
                reciproc_tc = val_str == "1";
            }
        }
    }
    BsrcParams {
        expr: &raw[..expr_end],
        tc1,
        tc2,
        reciproc_tc,
    }
}

/// Stamp a conductance G between nodes ni and nj into the matrix.
/// `None` means ground (not in matrix).
pub fn stamp_conductance(
    matrix: &mut crate::SparseMatrix,
    ni: Option<usize>,
    nj: Option<usize>,
    g: f64,
) {
    if let Some(i) = ni {
        matrix.add(i, i, g);
    }
    if let Some(j) = nj {
        matrix.add(j, j, g);
    }
    if let (Some(i), Some(j)) = (ni, nj) {
        matrix.add(i, j, -g);
        matrix.add(j, i, -g);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use thevenin_types::Netlist;
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn test_voltage_divider() {
        // V1=5V, R1=1k (between node 1 and mid), R2=1k (between mid and 0)
        // Expected: V(mid) = 2.5V, V(1) = 5V, I(V1) = -2.5mA
        let netlist = Netlist::parse_single(
            "Voltage divider test
V1 1 0 5
R1 1 mid 1k
R2 mid 0 1k
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();

        // 2 non-ground nodes (1, mid) + 1 voltage source = 3x3 system
        assert_eq!(mna.system.dim(), 3);
        assert_eq!(mna.node_map.len(), 2);
        assert_eq!(mna.vsource_names.len(), 1);

        let solution = mna.solve().unwrap();
        assert_abs_diff_eq!(solution.voltage("1").unwrap(), 5.0, epsilon = 1e-12);
        assert_abs_diff_eq!(solution.voltage("mid").unwrap(), 2.5, epsilon = 1e-12);
        assert_abs_diff_eq!(solution.voltage("0").unwrap(), 0.0, epsilon = 1e-12);

        // Current through V1: I = V/R_total = 5/2000 = 2.5mA
        // Convention: current flows from pos to neg through source (into node 1),
        // so branch current is negative (current flows out of source into circuit).
        let i_v1 = solution.branch_current("V1").unwrap();
        assert_abs_diff_eq!(i_v1.abs(), 2.5e-3, epsilon = 1e-12);
    }

    #[test]
    fn test_current_source_with_resistors() {
        // I1=1mA from 0 to node 1, R1=1k between node 1 and 0
        // V(1) = I * R = 1e-3 * 1000 = 1.0V
        let netlist = Netlist::parse_single(
            "Current source test
I1 0 1 1m
R1 1 0 1k
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        assert_eq!(mna.system.dim(), 1); // 1 node, 0 voltage sources

        let solution = mna.solve().unwrap();
        assert_abs_diff_eq!(solution.voltage("1").unwrap(), 1.0, epsilon = 1e-12);
    }

    #[test]
    fn test_series_resistors_with_voltage_source() {
        // V1=10V, R1=2k and R2=3k in series
        // V(mid) = 10 * 3k/(2k+3k) = 6V
        let netlist = Netlist::parse_single(
            "Series resistors
V1 in 0 10
R1 in mid 2k
R2 mid 0 3k
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        let solution = mna.solve().unwrap();

        assert_abs_diff_eq!(solution.voltage("in").unwrap(), 10.0, epsilon = 1e-12);
        assert_abs_diff_eq!(solution.voltage("mid").unwrap(), 6.0, epsilon = 1e-12);
    }

    #[test]
    fn test_ground_node_excluded() {
        // Single resistor from node 1 to ground with voltage source
        let netlist = Netlist::parse_single(
            "Ground test
V1 1 0 3.3
R1 1 0 330
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        // 1 node + 1 voltage source = 2x2
        assert_eq!(mna.system.dim(), 2);
        assert_eq!(mna.node_map.get("0"), None); // ground not in map
        assert!(mna.node_map.get("1").is_some());

        let solution = mna.solve().unwrap();
        assert_abs_diff_eq!(solution.voltage("1").unwrap(), 3.3, epsilon = 1e-12);
        // I = V/R = 3.3/330 = 0.01A
        let i_v1 = solution.branch_current("V1").unwrap();
        assert_abs_diff_eq!(i_v1.abs(), 0.01, epsilon = 1e-12);
    }

    #[test]
    fn test_multiple_current_sources() {
        // Two current sources into the same node with a resistor
        // I1=2mA from 0 to 1, I2=3mA from 0 to 1, R1=1k from 1 to 0
        // V(1) = (2m + 3m) * 1k = 5V
        let netlist = Netlist::parse_single(
            "Multiple current sources
I1 0 1 2m
I2 0 1 3m
R1 1 0 1k
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        let solution = mna.solve().unwrap();
        assert_abs_diff_eq!(solution.voltage("1").unwrap(), 5.0, epsilon = 1e-12);
    }

    #[test]
    fn test_rc_circuit_dc_op() {
        // RC circuit: V1=10V, R1=1k, C1=1u between mid and 0
        // In DC, capacitor is open circuit, so no current flows through R1.
        // V(mid) = V1 = 10V (no voltage drop across R1)
        let netlist = Netlist::parse_single(
            "RC circuit DC test
V1 1 0 10
R1 1 mid 1k
C1 mid 0 1u
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        // 2 non-ground nodes (1, mid) + 1 voltage source = 3x3
        // Capacitor adds no branch equation
        assert_eq!(mna.system.dim(), 3);

        let solution = mna.solve().unwrap();
        assert_abs_diff_eq!(solution.voltage("1").unwrap(), 10.0, epsilon = 1e-12);
        assert_abs_diff_eq!(solution.voltage("mid").unwrap(), 10.0, epsilon = 1e-12);
        // No current flows (capacitor is open)
        assert_abs_diff_eq!(solution.branch_current("V1").unwrap(), 0.0, epsilon = 1e-12);
    }

    #[test]
    fn test_rl_circuit_dc_op() {
        // RL circuit: V1=5V, R1=1k, L1=1m between mid and 0
        // In DC, inductor is short circuit, so full current flows.
        // V(mid) = 0V (inductor shorts mid to ground)
        // I = V1/R1 = 5/1000 = 5mA
        let netlist = Netlist::parse_single(
            "RL circuit DC test
V1 1 0 5
R1 1 mid 1k
L1 mid 0 1m
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        // 2 non-ground nodes (1, mid) + 1 voltage source + 1 inductor branch = 4x4
        assert_eq!(mna.system.dim(), 4);

        let solution = mna.solve().unwrap();
        assert_abs_diff_eq!(solution.voltage("1").unwrap(), 5.0, epsilon = 1e-12);
        assert_abs_diff_eq!(solution.voltage("mid").unwrap(), 0.0, epsilon = 1e-12);
        // Current through V1 = -5mA (flows out of source into circuit)
        assert_abs_diff_eq!(
            solution.branch_current("V1").unwrap().abs(),
            5e-3,
            epsilon = 1e-12
        );
        // Inductor branch current = 5mA
        assert_abs_diff_eq!(
            solution.branch_current("L1").unwrap().abs(),
            5e-3,
            epsilon = 1e-12
        );
    }

    #[test]
    fn test_vcvs_voltage_follower() {
        // VCVS with gain=1 (voltage follower): V(out) = V(in)
        // V1=5V at node "in", E1 copies to "out", R1=1k load on "out"
        let netlist = Netlist::parse_single(
            "VCVS voltage follower
V1 in 0 5
R1 in 0 10k
E1 out 0 in 0 1
R2 out 0 1k
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        // 2 nodes (in, out) + 2 branches (V1, E1)
        assert_eq!(mna.vsource_names.len(), 2);

        let solution = mna.solve().unwrap();
        assert_abs_diff_eq!(solution.voltage("in").unwrap(), 5.0, epsilon = 1e-12);
        assert_abs_diff_eq!(solution.voltage("out").unwrap(), 5.0, epsilon = 1e-12);
    }

    #[test]
    fn test_vcvs_amplifier() {
        // VCVS with gain=10: V(out) = 10 * V(in)
        // V1=1V, E1 gain=10
        let netlist = Netlist::parse_single(
            "VCVS amplifier
V1 in 0 1
R1 in 0 10k
E1 out 0 in 0 10
R2 out 0 1k
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        let solution = mna.solve().unwrap();
        assert_abs_diff_eq!(solution.voltage("in").unwrap(), 1.0, epsilon = 1e-12);
        assert_abs_diff_eq!(solution.voltage("out").unwrap(), 10.0, epsilon = 1e-12);
    }

    #[test]
    fn test_vcvs_inverting_amplifier() {
        // Op-amp as high-gain VCVS in inverting configuration:
        // V1=1V → R1=1k → inv node → R2=2k → out
        // E1 out 0 0 inv 100000 (gain from 0 to inv, i.e., V(out) = -100000 * V(inv))
        // With ideal gain: V(out) ≈ -R2/R1 * V(in) = -2V
        let netlist = Netlist::parse_single(
            "Inverting amplifier
V1 in 0 1
R1 in inv 1k
R2 inv out 2k
E1 out 0 0 inv 100000
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        let solution = mna.solve().unwrap();
        // V(out) ≈ -2.0 (with high gain, very close to ideal)
        assert_abs_diff_eq!(solution.voltage("out").unwrap(), -2.0, epsilon = 1e-3);
        // V(inv) ≈ 0 (virtual ground)
        assert_abs_diff_eq!(solution.voltage("inv").unwrap(), 0.0, epsilon = 1e-3);
    }

    #[test]
    fn test_vccs_transconductance() {
        // VCCS: G1 drives current gm * V(in) into node "out" through R2
        // SPICE convention: current flows from out- to out+ externally,
        // so G1 0 out means current enters "out" from ground.
        // V1=2V at "in", G1 gm=1m, R2=1k load
        // V(out) = gm * V(in) * R2 = 1e-3 * 2 * 1000 = 2V
        let netlist = Netlist::parse_single(
            "VCCS test
V1 in 0 2
R1 in 0 10k
G1 0 out in 0 1m
R2 out 0 1k
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        // No branch for G — only V1 has a branch
        assert_eq!(mna.vsource_names.len(), 1);

        let solution = mna.solve().unwrap();
        assert_abs_diff_eq!(solution.voltage("in").unwrap(), 2.0, epsilon = 1e-12);
        assert_abs_diff_eq!(solution.voltage("out").unwrap(), 2.0, epsilon = 1e-12);
    }

    #[test]
    fn test_cccs_current_amplifier() {
        // CCCS: F1 outputs gain * I(Vsense)
        // Vsense is a 0V sensing source in series with R1
        // V1=10V, R1=10k → I(Vsense) = 1mA (in MNA branch convention)
        // F1 0 out: current enters "out" from ground
        // F1 gain=5 → I_out = 5mA into R2=1k → V(out) = 5V
        let netlist = Netlist::parse_single(
            "CCCS test
V1 1 0 10
R1 1 sense 10k
Vsense sense 0 0
F1 0 out Vsense 5
R2 out 0 1k
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        let solution = mna.solve().unwrap();
        // I(Vsense) = 1mA, F1 gain=5 → 5mA into R2=1k
        assert_abs_diff_eq!(solution.voltage("out").unwrap(), 5.0, epsilon = 1e-9);
    }

    #[test]
    fn test_ccvs_transresistance() {
        // CCVS: H1 output voltage = rm * I(Vsense)
        // V1=5V, R1=5k → I(Vsense) = 1mA
        // H1 rm=2k → V(out) = 2k * 1mA = 2V
        let netlist = Netlist::parse_single(
            "CCVS test
V1 1 0 5
R1 1 sense 5k
Vsense sense 0 0
H1 out 0 Vsense 2k
R2 out 0 10k
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        // V1, Vsense, H1 all have branches
        assert_eq!(mna.vsource_names.len(), 3);

        let solution = mna.solve().unwrap();
        assert_abs_diff_eq!(solution.voltage("out").unwrap(), 2.0, epsilon = 1e-9);
    }

    #[test]
    fn test_mutual_coupling_assembly() {
        // Two coupled inductors with k=0.5. In DC both are shorts, so mutual
        // coupling has no DC effect. We verify the MNA assembles correctly and
        // the mutual_couplings vec is populated with the right factor.
        let netlist = Netlist::parse_single(
            "Mutual coupling test
V1 1 0 5
R1 1 2 100
L1 2 0 10m
L2 3 0 20m
R2 3 0 100
K1 L1 L2 0.5
.op
.end
",
        )
        .unwrap();

        let mna = crate::test_support::assemble_ir(&netlist).unwrap();
        assert_eq!(mna.mutual_couplings.len(), 1);

        let mc = &mna.mutual_couplings[0];
        // M = k * sqrt(L1 * L2) = 0.5 * sqrt(0.01 * 0.02) = 0.5 * sqrt(0.0002)
        let expected_m = 0.5 * (0.01_f64 * 0.02).sqrt();
        assert_abs_diff_eq!(mc.factor, expected_m, epsilon = 1e-12);

        // In DC, inductors are shorts: V(2) = 0V, V(3) = 0V.
        let sol = mna.solve().unwrap();
        assert_abs_diff_eq!(sol.voltage("2").unwrap(), 0.0, epsilon = 1e-9);
        assert_abs_diff_eq!(sol.voltage("3").unwrap(), 0.0, epsilon = 1e-9);
    }

    #[test]
    fn test_mutual_coupling_unknown_inductor() {
        // K element referencing a non-existent inductor should error.
        let netlist = Netlist::parse_single(
            "Bad coupling
V1 1 0 5
L1 1 0 10m
K1 L1 L99 0.5
.op
.end
",
        )
        .unwrap();

        let result = crate::test_support::assemble_ir(&netlist);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("L99") || err_msg.contains("l99"));
    }
}
