//! Canonical Cirq IR — the semantic center of the Cirq toolchain.
//!
//! This IR is produced by lowering the Cirq AST (name resolution, parameter
//! evaluation, subcircuit flattening, validation). All downstream consumers
//! (simulator adapter, linting, formatting) should work from this representation.

/// Unique identifier for IR nodes (nets, elements, modules, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id(pub u32);

/// A fully resolved circuit ready for simulation or analysis.
#[derive(Debug, Clone)]
pub struct Circuit {
    pub name: String,
    pub nets: Vec<Net>,
    pub elements: Vec<Element>,
    pub models: Vec<Model>,
    pub analyses: Vec<Analysis>,
    pub params: Vec<ResolvedParam>,
    /// Simulation options (e.g. GMIN, ABSTOL, RELTOL).
    pub options: Vec<(String, Value)>,
    /// Simulation temperature in °C. `None` means use default (27°C).
    pub temp: Option<f64>,
    /// Output save targets (e.g. `v(out)`, `i(R1)`).
    pub save: Vec<String>,
    /// User-defined functions.
    pub funcs: Vec<FuncDef>,
    /// Initial node voltages (`.ic`).
    pub initial_conditions: Vec<(Id, f64)>,
    /// Verbatim embedded code blocks — each entry is `(language, lines)`.
    /// `"control"` blocks are passed to the SPICE control-block interpreter.
    pub code_blocks: Vec<CodeBlock>,
}

/// A verbatim embedded code block with a language tag.
#[derive(Debug, Clone)]
pub struct CodeBlock {
    pub language: String,
    pub lines: Vec<String>,
}

/// A resolved electrical net.
#[derive(Debug, Clone)]
pub struct Net {
    pub id: Id,
    pub name: String,
    pub is_global: bool,
}

/// A resolved element instance.
#[derive(Debug, Clone)]
pub struct Element {
    pub id: Id,
    pub name: String,
    pub kind: ElementKind,
    pub connections: Vec<Connection>,
    pub params: Vec<(String, Value)>,
    pub model: Option<Id>,
    /// Source specification for voltage/current sources.
    /// `None` for non-source elements.
    pub source_spec: Option<SourceSpec>,
}

/// Connection between an element terminal and a net.
#[derive(Debug, Clone)]
pub struct Connection {
    pub terminal: String,
    pub net: Id,
}

/// Resolved element kinds (after model resolution).
#[derive(Debug, Clone)]
pub enum ElementKind {
    Resistor,
    Capacitor,
    Inductor,
    Coupling,
    VoltageSource,
    CurrentSource,
    BehavioralSource {
        /// `Voltage` or `Current` — voltage or current mode.
        mode: BehavioralMode,
        /// The expression string, e.g. `"sin(2*pi*1k*time)"`.
        spec: String,
    },
    Diode,
    Npn,
    Pnp,
    Nmos,
    Pmos,
    NJfet,
    PJfet,
    NMesfet,
    PMesfet,
    Vcvs,
    Vccs,
    Ccvs,
    Cccs,
    TransmissionLine,
    /// Coupled multiconductor transmission line (P element).
    /// Connections use terminal names `"in0"`, `"in1"`, ..., `"gnd"`,
    /// `"out0"`, `"out1"`, ... in the element's `connections` field.
    CoupledLine {
        /// Number of coupled lines (= number of in/out port pairs).
        width: usize,
    },
    /// XSPICE code model instance (A element).
    /// Scalar connections use terminal names `"c0"`, `"c1"`, ...
    /// The full structured connection list (scalar vs. array) is here.
    Xspice {
        connections: Vec<XspiceConnection>,
    },
}

/// Behavioral source mode — voltage or current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BehavioralMode {
    Voltage,
    Current,
}

/// A single XSPICE port connection at the IR level.
#[derive(Debug, Clone)]
pub enum XspiceConnection {
    /// A single scalar net.
    Scalar(Id),
    /// A bracketed array of nets.
    Array(Vec<Id>),
}

/// A resolved device model.
#[derive(Debug, Clone)]
pub struct Model {
    pub id: Id,
    pub name: String,
    pub device_type: DeviceType,
    pub params: Vec<(String, Value)>,
}

/// Device types for models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Diode,
    Npn,
    Pnp,
    Nmos,
    Pmos,
    NJfet,
    PJfet,
    NMesfet,
    PMesfet,
}

/// A resolved parameter value.
#[derive(Debug, Clone)]
pub enum Value {
    Real(f64),
    Integer(i64),
    Bool(bool),
    String(String),
}

// ---------------------------------------------------------------------------
// Source specifications (voltage/current sources)
// ---------------------------------------------------------------------------

/// AC specification for a voltage/current source: magnitude and phase.
#[derive(Debug, Clone)]
pub struct AcSpec {
    pub mag: f64,
    /// Phase in degrees. Defaults to 0.0 when not specified.
    pub phase: f64,
}

/// Transient waveform for voltage/current sources.
#[derive(Debug, Clone)]
pub enum Waveform {
    /// `PULSE(v1 v2 [td [tr [tf [pw [per]]]]])`
    Pulse {
        v1: f64,
        v2: f64,
        td: Option<f64>,
        tr: Option<f64>,
        tf: Option<f64>,
        pw: Option<f64>,
        per: Option<f64>,
    },
    /// `SIN(v0 va [freq [td [theta [phi]]]])`
    Sin {
        v0: f64,
        va: f64,
        freq: Option<f64>,
        td: Option<f64>,
        theta: Option<f64>,
        phi: Option<f64>,
    },
    /// `EXP(v1 v2 [td1 [tau1 [td2 [tau2]]]])`
    Exp {
        v1: f64,
        v2: f64,
        td1: Option<f64>,
        tau1: Option<f64>,
        td2: Option<f64>,
        tau2: Option<f64>,
    },
    /// `PWL(t1 v1 t2 v2 ...)` — piecewise linear.
    Pwl(Vec<(f64, f64)>),
    /// `SFFM(v0 va [fc [fs [md]]])`
    Sffm {
        v0: f64,
        va: f64,
        fc: Option<f64>,
        fs: Option<f64>,
        md: Option<f64>,
    },
    /// `AM(va vo fc fs [td])`
    Am {
        va: f64,
        vo: f64,
        fc: f64,
        fs: f64,
        td: Option<f64>,
    },
}

/// Source specification for voltage/current sources.
///
/// Combines DC value, AC small-signal specification, and transient waveform.
/// All fields are independently optional.
#[derive(Debug, Clone, Default)]
pub struct SourceSpec {
    pub dc: Option<f64>,
    pub ac: Option<AcSpec>,
    pub waveform: Option<Waveform>,
}

/// A resolved parameter binding.
#[derive(Debug, Clone)]
pub struct ResolvedParam {
    pub name: String,
    pub value: Value,
}

/// A user-defined function.
#[derive(Debug, Clone)]
pub struct FuncDef {
    pub name: String,
    pub args: Vec<String>,
    /// The function body as a SPICE-compatible expression string.
    pub body: String,
}

/// A resolved analysis command.
#[derive(Debug, Clone)]
pub enum Analysis {
    Op,
    Dc(DcAnalysis),
    Ac(AcAnalysis),
    Tran(TranAnalysis),
    Noise(NoiseAnalysis),
    Pz(PzAnalysis),
    Sens(SensAnalysis),
    Tf(TfAnalysis),
}

#[derive(Debug, Clone)]
pub struct DcAnalysis {
    pub sweeps: Vec<DcSweep>,
}

#[derive(Debug, Clone)]
pub struct DcSweep {
    pub source: Id,
    pub start: f64,
    pub stop: f64,
    pub step: f64,
}

#[derive(Debug, Clone)]
pub struct AcAnalysis {
    pub start: f64,
    pub stop: f64,
    pub points: u32,
    pub scale: FrequencyScale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrequencyScale {
    Decade,
    Octave,
    Linear,
}

#[derive(Debug, Clone)]
pub struct TranAnalysis {
    pub step: f64,
    pub stop: f64,
    pub start: f64,
    pub uic: bool,
    /// Maximum internal timestep. `None` means the solver picks automatically.
    pub tmax: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct NoiseAnalysis {
    pub output_net: Id,
    pub reference_net: Id,
    pub source: Id,
    pub start: f64,
    pub stop: f64,
    pub points: u32,
    pub scale: FrequencyScale,
}

#[derive(Debug, Clone)]
pub struct PzAnalysis {
    pub input_pos: Id,
    pub input_neg: Id,
    pub output_pos: Id,
    pub output_neg: Id,
    pub transfer: TransferType,
    pub analysis_type: PzType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferType {
    Voltage,
    Current,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PzType {
    Poles,
    Zeros,
    Both,
}

#[derive(Debug, Clone)]
pub struct SensAnalysis {
    pub output: String,
}

#[derive(Debug, Clone)]
pub struct TfAnalysis {
    pub output: String,
    pub source: Id,
}
