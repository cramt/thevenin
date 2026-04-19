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
    Diode,
    Npn,
    Pnp,
    Nmos,
    Pmos,
    NJfet,
    PJfet,
    Vcvs,
    Vccs,
    Ccvs,
    Cccs,
    TransmissionLine,
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

/// A resolved parameter binding.
#[derive(Debug, Clone)]
pub struct ResolvedParam {
    pub name: String,
    pub value: Value,
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
