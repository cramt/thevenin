//! XSPICE code model framework for the thevenin circuit simulator.
//!
//! This crate provides the type definitions, builder API, registry, and MNA
//! stamping logic for user-defined XSPICE analog code models. Users depend on
//! this crate to define models without pulling in the full simulator.

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
