//! XSPICE code-model framework for the thevenin circuit simulator.
//!
//! XSPICE lets you define behavioural analog "code models" — devices whose
//! port currents/voltages are computed by Rust code rather than a fixed
//! equation set — and bind them to the `A` element in a netlist. This crate
//! holds the type definitions, builder API, [registry](CodeModelRegistry), and
//! MNA-stamping contract for those models, so a model can be authored (and
//! unit-tested) without pulling in the full simulator.
//!
//! A model implements the code-model trait, declares its ports and parameters,
//! and registers itself in a [`CodeModelRegistry`]; the simulator threads that
//! registry through MNA assembly and calls the model to stamp its contribution
//! at each Newton iteration.
//!
//! Compiled-in models via the registry are fully supported; dynamically loaded
//! (`.cm` shared-object) models are out of scope.

pub mod eval;
pub mod instance;
pub mod model;
pub mod registry;
pub mod stamp;
pub mod types;

pub use eval::{AnalysisMode, CmInputs, CmOutputs, PartialDerivative, PortOutput};
pub use instance::{PortConnection, XspiceInstance};
pub use model::{CodeModel, CodeModelBuilder, CodeModelDef};
pub use registry::CodeModelRegistry;
pub use stamp::{MatrixStamp, stamp_xspice_instance};
pub use types::{ParamDef, ParamType, ParamValue, PortDef, PortDirection, PortType};
