//! The fully-resolved circuit intermediate representation.
//!
//! Everything that CirQ or SPICE leave implicit is made explicit here:
//! - Net domains are computed from connectivity
//! - The ground net is always present
//! - All pin connections are named (not positional)
//! - Subcircuit port ordering is explicit

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

// ---------------------------------------------------------------------------
// Scalar values
// ---------------------------------------------------------------------------

// r[impl component.value.format]
/// A scalar value that may be a resolved number, a parameter reference, or
/// an unevaluated expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Resolved numeric literal.
    Num(f64),
    /// Reference to a `.param` / CirQ `params` entry by name.
    Param(String),
    /// Arbitrary expression (SPICE `{...}` or CirQ expression string).
    Expr(String),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Num(v) => write!(f, "{v}"),
            Value::Param(s) => write!(f, "{s}"),
            Value::Expr(s) => write!(f, "{{{s}}}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Net domain
// ---------------------------------------------------------------------------

// r[impl domain.values]
/// The signal domain of a net, fully resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Domain {
    Analog,
    Digital,
    Mixed,
    Unspecified,
}

impl fmt::Display for Domain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Domain::Analog => write!(f, "analog"),
            Domain::Digital => write!(f, "digital"),
            Domain::Mixed => write!(f, "mixed"),
            Domain::Unspecified => write!(f, "unspecified"),
        }
    }
}

// ---------------------------------------------------------------------------
// Net
// ---------------------------------------------------------------------------

// r[impl net.implicit]
// r[impl net.name]
/// A fully-resolved net with its inferred domain.
#[derive(Debug, Clone)]
pub struct Net {
    pub name: String,
    pub domain: Domain,
}

// ---------------------------------------------------------------------------
// Port direction
// ---------------------------------------------------------------------------

// r[impl port.direction]
/// Direction of a port on a circuit or subcircuit boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
    InOut,
    Passive,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Direction::Input => write!(f, "input"),
            Direction::Output => write!(f, "output"),
            Direction::InOut => write!(f, "inout"),
            Direction::Passive => write!(f, "passive"),
        }
    }
}

// ---------------------------------------------------------------------------
// Waveform
// ---------------------------------------------------------------------------

/// Transient waveform specification for voltage/current sources.
#[derive(Debug, Clone)]
pub enum Waveform {
    Pulse {
        v1: Value,
        v2: Value,
        td: Option<Value>,
        tr: Option<Value>,
        tf: Option<Value>,
        pw: Option<Value>,
        per: Option<Value>,
    },
    Sin {
        v0: Value,
        va: Value,
        freq: Option<Value>,
        td: Option<Value>,
        theta: Option<Value>,
        phi: Option<Value>,
    },
    Exp {
        v1: Value,
        v2: Value,
        td1: Option<Value>,
        tau1: Option<Value>,
        td2: Option<Value>,
        tau2: Option<Value>,
    },
    Pwl {
        points: Vec<(Value, Value)>,
    },
    Sffm {
        v0: Value,
        va: Value,
        fc: Option<Value>,
        fs: Option<Value>,
        md: Option<Value>,
    },
    Am {
        va: Value,
        vo: Value,
        fc: Value,
        fs: Value,
        td: Option<Value>,
    },
}

// ---------------------------------------------------------------------------
// Source specification
// ---------------------------------------------------------------------------

/// Full source specification (DC + AC + transient waveform).
#[derive(Debug, Clone, Default)]
pub struct SourceSpec {
    pub dc: Option<Value>,
    pub ac_mag: Option<Value>,
    pub ac_phase: Option<Value>,
    pub waveform: Option<Waveform>,
}

// ---------------------------------------------------------------------------
// Component kinds
// ---------------------------------------------------------------------------

// r[impl component.type]
/// The type-specific body of a component.
#[derive(Debug, Clone)]
pub enum ComponentKind {
    // -- Passives --
    Resistor {
        p: String,
        n: String,
        value: Value,
        params: BTreeMap<String, Value>,
    },
    Capacitor {
        p: String,
        n: String,
        value: Value,
        params: BTreeMap<String, Value>,
    },
    Inductor {
        p: String,
        n: String,
        value: Value,
        params: BTreeMap<String, Value>,
    },

    // -- Mutual coupling (not a physical component) --
    Coupling {
        l1: String,
        l2: String,
        coefficient: Value,
    },

    // -- Diodes --
    Diode {
        a: String,
        k: String,
        model: String,
        params: BTreeMap<String, Value>,
    },

    // -- BJTs --
    Bjt {
        polarity: BjtPolarity,
        c: String,
        b: String,
        e: String,
        s: Option<String>,
        model: String,
        params: BTreeMap<String, Value>,
        off: bool,
    },

    // -- MOSFETs --
    Mosfet {
        polarity: MosfetPolarity,
        d: String,
        g: String,
        s: String,
        b: String,
        body: Option<String>,
        model: String,
        params: BTreeMap<String, Value>,
    },

    // -- JFETs --
    Jfet {
        polarity: JfetPolarity,
        d: String,
        g: String,
        s: String,
        model: String,
        params: BTreeMap<String, Value>,
    },

    // -- MESFETs --
    Mesfet {
        d: String,
        g: String,
        s: String,
        model: String,
        params: BTreeMap<String, Value>,
    },

    // -- Independent sources --
    VSource {
        p: String,
        n: String,
        source: SourceSpec,
    },
    ISource {
        p: String,
        n: String,
        source: SourceSpec,
    },

    // -- Controlled sources --
    Vcvs {
        p: String,
        n: String,
        cp: String,
        cn: String,
        gain: Value,
    },
    Vccs {
        p: String,
        n: String,
        cp: String,
        cn: String,
        gm: Value,
    },
    Cccs {
        p: String,
        n: String,
        vsource: String,
        gain: Value,
    },
    Ccvs {
        p: String,
        n: String,
        vsource: String,
        transresistance: Value,
    },

    // -- Behavioral source --
    BehavioralSource {
        p: String,
        n: String,
        expr: BehavioralExpr,
    },

    // -- Switches --
    VSwitch {
        p: String,
        n: String,
        cp: String,
        cn: String,
        model: String,
        params: BTreeMap<String, Value>,
    },
    ISwitch {
        p: String,
        n: String,
        vsource: String,
        model: String,
        params: BTreeMap<String, Value>,
    },

    // -- Transmission lines --
    Tline {
        p1: String,
        n1: String,
        p2: String,
        n2: String,
        params: BTreeMap<String, Value>,
    },
    Ltra {
        p1: String,
        n1: String,
        p2: String,
        n2: String,
        model: String,
        params: BTreeMap<String, Value>,
    },
    Txl {
        p1: String,
        n1: String,
        p2: String,
        n2: String,
        model: String,
        params: BTreeMap<String, Value>,
    },

    // -- XSPICE --
    Xspice {
        connections: Vec<XspicePort>,
        model: String,
    },

    // -- Subcircuit instance --
    SubcktInstance {
        subckt: String,
        /// Named pin→net mapping. Resolved from positional SPICE or named CirQ.
        pins: BTreeMap<String, String>,
        params: BTreeMap<String, Value>,
    },

    // -- Port (circuit/subcircuit boundary) --
    Port {
        net: String,
        direction: Direction,
        order: u32,
        domain_override: Option<Domain>,
    },

    // -- Cell (black box) --
    Cell {
        model: String,
        pins: BTreeMap<String, String>,
    },

    // -- Digital logic gate --
    DigitalGate {
        gate_type: DigitalGateType,
        pins: BTreeMap<String, String>,
    },

    // -- Unrecognized element (no fabricated connections) --
    Raw {
        text: String,
    },
}

/// BJT polarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BjtPolarity {
    Npn,
    Pnp,
}

/// MOSFET polarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MosfetPolarity {
    Nmos,
    Pmos,
}

/// JFET polarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JfetPolarity {
    Njfet,
    Pjfet,
}

/// Behavioral source expression type.
#[derive(Debug, Clone)]
pub enum BehavioralExpr {
    Voltage(String),
    Current(String),
}

/// Digital gate type (spec-defined primitives).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigitalGateType {
    And,
    Or,
    Not,
    Nand,
    Nor,
    Xor,
    Xnor,
    Buf,
    Dff,
    DffSr,
    Mux2,
    Latch,
}

impl fmt::Display for DigitalGateType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::And => write!(f, "and"),
            Self::Or => write!(f, "or"),
            Self::Not => write!(f, "not"),
            Self::Nand => write!(f, "nand"),
            Self::Nor => write!(f, "nor"),
            Self::Xor => write!(f, "xor"),
            Self::Xnor => write!(f, "xnor"),
            Self::Buf => write!(f, "buf"),
            Self::Dff => write!(f, "dff"),
            Self::DffSr => write!(f, "dff_sr"),
            Self::Mux2 => write!(f, "mux2"),
            Self::Latch => write!(f, "latch"),
        }
    }
}

/// XSPICE port: scalar or vector.
#[derive(Debug, Clone)]
pub enum XspicePort {
    Scalar(String),
    Array(Vec<String>),
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

// r[impl component.id]
// r[impl component.description]
// r[impl component.tags]
/// A single component in the circuit.
#[derive(Debug, Clone)]
pub struct Component {
    /// Unique identifier (SPICE instance name or CirQ `id`).
    pub id: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Optional tags for tooling.
    pub tags: Vec<String>,
    /// The component body.
    pub kind: ComponentKind,
}

impl Component {
    // r[impl domain.primitives.analog]
    // r[impl domain.primitives.digital]
    // r[impl domain.primitives.cell]
    /// Returns the domain classification of this component for net inference.
    pub fn domain_class(&self) -> DomainClass {
        match &self.kind {
            ComponentKind::Resistor { .. }
            | ComponentKind::Capacitor { .. }
            | ComponentKind::Inductor { .. }
            | ComponentKind::Coupling { .. }
            | ComponentKind::Diode { .. }
            | ComponentKind::Bjt { .. }
            | ComponentKind::Mosfet { .. }
            | ComponentKind::Jfet { .. }
            | ComponentKind::Mesfet { .. }
            | ComponentKind::VSource { .. }
            | ComponentKind::ISource { .. }
            | ComponentKind::Vcvs { .. }
            | ComponentKind::Vccs { .. }
            | ComponentKind::Cccs { .. }
            | ComponentKind::Ccvs { .. }
            | ComponentKind::BehavioralSource { .. }
            | ComponentKind::VSwitch { .. }
            | ComponentKind::ISwitch { .. }
            | ComponentKind::Tline { .. }
            | ComponentKind::Ltra { .. }
            | ComponentKind::Txl { .. } => DomainClass::Analog,

            // r[impl domain.primitives.digital]
            ComponentKind::DigitalGate { .. } => DomainClass::Digital,

            ComponentKind::Xspice { .. } | ComponentKind::Cell { .. } => DomainClass::Unknown,

            ComponentKind::SubcktInstance { .. } | ComponentKind::Raw { .. } => {
                DomainClass::Unknown
            }

            ComponentKind::Port {
                domain_override, ..
            } => match domain_override {
                Some(Domain::Analog) => DomainClass::Analog,
                Some(Domain::Digital) => DomainClass::Digital,
                Some(Domain::Mixed) => DomainClass::Unknown,
                _ => DomainClass::Unknown,
            },
        }
    }

    /// Returns all net names this component connects to.
    pub fn connected_nets(&self) -> Vec<&str> {
        match &self.kind {
            ComponentKind::Resistor { p, n, .. }
            | ComponentKind::Capacitor { p, n, .. }
            | ComponentKind::Inductor { p, n, .. } => vec![p, n],

            // Coupling references inductor component IDs, not nets — the
            // actual net connections are counted via the inductor components.
            ComponentKind::Coupling { .. } => vec![],

            ComponentKind::Diode { a, k, .. } => vec![a, k],

            ComponentKind::Bjt { c, b, e, s, .. } => {
                let mut v = vec![c.as_str(), b.as_str(), e.as_str()];
                if let Some(sub) = s {
                    v.push(sub);
                }
                v
            }

            ComponentKind::Mosfet {
                d, g, s, b, body, ..
            } => {
                let mut v = vec![d.as_str(), g.as_str(), s.as_str(), b.as_str()];
                if let Some(bd) = body {
                    v.push(bd);
                }
                v
            }

            ComponentKind::Jfet { d, g, s, .. } | ComponentKind::Mesfet { d, g, s, .. } => {
                vec![d, g, s]
            }

            ComponentKind::VSource { p, n, .. }
            | ComponentKind::ISource { p, n, .. }
            | ComponentKind::BehavioralSource { p, n, .. } => vec![p, n],

            ComponentKind::Vcvs { p, n, cp, cn, .. } | ComponentKind::Vccs { p, n, cp, cn, .. } => {
                vec![p, n, cp, cn]
            }

            ComponentKind::Cccs { p, n, .. } | ComponentKind::Ccvs { p, n, .. } => vec![p, n],

            ComponentKind::VSwitch { p, n, cp, cn, .. } => vec![p, n, cp, cn],
            ComponentKind::ISwitch { p, n, .. } => vec![p, n],

            ComponentKind::Tline { p1, n1, p2, n2, .. }
            | ComponentKind::Ltra { p1, n1, p2, n2, .. }
            | ComponentKind::Txl { p1, n1, p2, n2, .. } => vec![p1, n1, p2, n2],

            ComponentKind::Xspice { connections, .. } => {
                let mut v = Vec::new();
                for conn in connections {
                    match conn {
                        XspicePort::Scalar(s) => v.push(s.as_str()),
                        XspicePort::Array(arr) => {
                            for s in arr {
                                v.push(s.as_str());
                            }
                        }
                    }
                }
                v
            }

            ComponentKind::SubcktInstance { pins, .. } => {
                pins.values().map(|s| s.as_str()).collect()
            }

            ComponentKind::Port { net, .. } => vec![net],

            ComponentKind::Cell { pins, .. } | ComponentKind::DigitalGate { pins, .. } => {
                pins.values().map(|s| s.as_str()).collect()
            }

            // Raw elements have no known connections — do not fabricate any.
            ComponentKind::Raw { .. } => vec![],
        }
    }
}

/// Domain classification for net inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DomainClass {
    Analog,
    Digital,
    Unknown,
}

// ---------------------------------------------------------------------------
// Model definition
// ---------------------------------------------------------------------------

/// The device type a model applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelType {
    Diode,
    Npn,
    Pnp,
    Nmos,
    Pmos,
    Njfet,
    Pjfet,
    Mesfet,
    Ltra,
    Txl,
    Cpl,
    VSwitch,
    ISwitch,
    /// XSPICE or other custom model type.
    Other(String),
}

impl fmt::Display for ModelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelType::Diode => write!(f, "diode"),
            ModelType::Npn => write!(f, "npn"),
            ModelType::Pnp => write!(f, "pnp"),
            ModelType::Nmos => write!(f, "nmos"),
            ModelType::Pmos => write!(f, "pmos"),
            ModelType::Njfet => write!(f, "njfet"),
            ModelType::Pjfet => write!(f, "pjfet"),
            ModelType::Mesfet => write!(f, "mesfet"),
            ModelType::Ltra => write!(f, "ltra"),
            ModelType::Txl => write!(f, "txl"),
            ModelType::Cpl => write!(f, "cpl"),
            ModelType::VSwitch => write!(f, "vswitch"),
            ModelType::ISwitch => write!(f, "iswitch"),
            ModelType::Other(s) => write!(f, "{s}"),
        }
    }
}

// r[impl model.name]
// r[impl model.type]
// r[impl model.level]
// r[impl model.params]
/// A device model definition.
#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    pub model_type: ModelType,
    pub level: Option<u32>,
    pub params: BTreeMap<String, Value>,
}

// ---------------------------------------------------------------------------
// Subcircuit definition
// ---------------------------------------------------------------------------

// r[impl subckt.name]
// r[impl subckt.components]
// r[impl subckt.scope]
/// A subcircuit definition (reusable sub-circuit template).
#[derive(Debug, Clone)]
pub struct Subcircuit {
    pub name: String,
    pub description: Option<String>,
    /// Default parameter values.
    pub params: BTreeMap<String, Value>,
    /// Components inside the subcircuit (including ports).
    pub components: Vec<Component>,
    /// Nested model definitions.
    pub models: Vec<Model>,
    /// Nested subcircuit definitions.
    pub subcircuits: Vec<Subcircuit>,
}

// ---------------------------------------------------------------------------
// Include
// ---------------------------------------------------------------------------

/// A file include directive.
#[derive(Debug, Clone)]
pub struct Include {
    pub file: String,
    /// Optional library section name.
    pub section: Option<String>,
}

// ---------------------------------------------------------------------------
// Function definition
// ---------------------------------------------------------------------------

/// A user-defined function.
#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub args: Vec<String>,
    pub body: String,
}

// ---------------------------------------------------------------------------
// Circuit (top-level IR)
// ---------------------------------------------------------------------------

// r[impl doc.name]
// r[impl doc.components]
// r[impl doc.subcircuits]
// r[impl doc.models]
// r[impl doc.params]
// r[impl doc.globals]
// r[impl doc.includes]
// r[impl doc.functions]
// r[impl doc.options]
// r[impl doc.temperature]
/// A complete circuit in fully-resolved form.
#[derive(Debug, Clone)]
pub struct Circuit {
    /// Circuit name (SPICE title line or CirQ `name`).
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// All components (including ports).
    pub components: Vec<Component>,
    /// Model definitions.
    pub models: Vec<Model>,
    /// Subcircuit definitions.
    pub subcircuits: Vec<Subcircuit>,
    /// Global parameters.
    pub params: BTreeMap<String, Value>,
    /// Global net names (visible across subcircuit boundaries).
    pub globals: Vec<String>,
    /// File includes.
    pub includes: Vec<Include>,
    /// User-defined functions.
    pub functions: Vec<Function>,
    /// Simulator options.
    pub options: BTreeMap<String, Value>,
    /// Circuit temperature in °C.
    pub temperature: Option<f64>,
    /// Fully-resolved net table with inferred domains.
    pub nets: BTreeMap<String, Net>,
}

impl Circuit {
    // r[impl domain.inferred]
    // r[impl domain.inference.analog]
    // r[impl domain.inference.digital]
    // r[impl domain.inference.mixed]
    // r[impl domain.inference.unspecified]
    // r[impl domain.override]
    // r[impl net.ground]
    /// Resolve net domains from component connectivity.
    ///
    /// This walks all components, collects which nets they touch, classifies
    /// each net based on the domain of connected components, and populates
    /// `self.nets`.
    pub fn resolve_domains(&mut self) {
        // Collect: net_name → set of domain classes touching it
        let mut net_classes: BTreeMap<String, BTreeSet<DomainClass>> = BTreeMap::new();

        // Ensure ground always exists
        net_classes.entry("0".to_string()).or_default();

        for comp in &self.components {
            let dc = comp.domain_class();
            for net_name in comp.connected_nets() {
                net_classes
                    .entry(net_name.to_string())
                    .or_default()
                    .insert(dc);
            }
        }

        // Check for port domain overrides
        let mut port_overrides: BTreeMap<String, Domain> = BTreeMap::new();
        for comp in &self.components {
            if let ComponentKind::Port {
                net,
                domain_override: Some(d),
                ..
            } = &comp.kind
            {
                port_overrides.insert(net.clone(), *d);
            }
        }

        self.nets.clear();
        for (name, classes) in &net_classes {
            let domain = if let Some(&override_domain) = port_overrides.get(name) {
                override_domain
            } else {
                // Remove Unknown from consideration for classification
                let has_analog = classes.contains(&DomainClass::Analog);
                let has_digital = classes.contains(&DomainClass::Digital);
                match (has_analog, has_digital) {
                    (true, true) => Domain::Mixed,
                    (true, false) => Domain::Analog,
                    (false, true) => Domain::Digital,
                    (false, false) => Domain::Unspecified,
                }
            };
            self.nets.insert(
                name.clone(),
                Net {
                    name: name.clone(),
                    domain,
                },
            );
        }
    }
}
