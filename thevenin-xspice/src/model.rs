//! Code model definition and builder API.

use std::any::Any;

use crate::eval::{CmInputs, CmOutputs};
use crate::types::{ParamDef, ParamType, ParamValue, PortDef, PortDirection, PortType};

/// Trait for implementing XSPICE code models with typed per-instance state.
///
/// The state type is an associated type, so implementations get `&mut State`
/// directly — no downcasting needed. Type erasure happens at the registry
/// boundary via [`CodeModelDef`].
pub trait CodeModel: Send + Sync + 'static {
    /// Per-instance mutable state type.
    type State: 'static;

    /// Create initial state for a new instance.
    fn create_state(&self) -> Self::State;

    /// Evaluate the code model for one Newton-Raphson iteration.
    fn evaluate(&self, inputs: &CmInputs, state: &mut Self::State) -> CmOutputs;
}

/// Object-safe type-erased version of [`CodeModel`], used internally.
trait ErasedCodeModel: Send + Sync {
    fn create_state_erased(&self) -> Box<dyn Any>;
    fn evaluate_erased(&self, inputs: &CmInputs, state: &mut dyn Any) -> CmOutputs;
}

impl<T: CodeModel> ErasedCodeModel for T {
    fn create_state_erased(&self) -> Box<dyn Any> {
        Box::new(self.create_state())
    }

    fn evaluate_erased(&self, inputs: &CmInputs, state: &mut dyn Any) -> CmOutputs {
        let state = state
            .downcast_mut::<T::State>()
            .expect("XSPICE state type mismatch (bug in registry/instance wiring)");
        self.evaluate(inputs, state)
    }
}

/// A type-erased XSPICE code model definition, ready for registry storage.
///
/// Wraps a [`CodeModel`] impl with port/parameter metadata. The state type
/// is erased to `Box<dyn Any>` internally, but users implementing [`CodeModel`]
/// never see `dyn Any`.
pub struct CodeModelDef {
    /// Model type name (e.g., "d_gain"). Matched case-insensitively against
    /// `.model` kind.
    pub name: String,
    /// Port definitions in connection order.
    pub ports: Vec<PortDef>,
    /// Parameter definitions with defaults.
    pub params: Vec<ParamDef>,
    erased: Box<dyn ErasedCodeModel>,
}

impl CodeModelDef {
    /// Wrap a [`CodeModel`] impl with metadata into a type-erased definition.
    pub fn new<M: CodeModel>(
        name: impl Into<String>,
        ports: Vec<PortDef>,
        params: Vec<ParamDef>,
        model: M,
    ) -> Self {
        Self {
            name: name.into(),
            ports,
            params,
            erased: Box::new(model),
        }
    }

    /// Create initial per-instance state (type-erased).
    pub fn create_state(&self) -> Box<dyn Any> {
        self.erased.create_state_erased()
    }

    /// Evaluate the code model with type-erased state.
    pub fn evaluate(&self, inputs: &CmInputs, state: &mut dyn Any) -> CmOutputs {
        self.erased.evaluate_erased(inputs, state)
    }
}

impl std::fmt::Debug for CodeModelDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeModelDef")
            .field("name", &self.name)
            .field("ports", &self.ports)
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

type EvalFn<S> = Box<dyn Fn(&CmInputs, &mut S) -> CmOutputs + Send + Sync>;

/// A closure-based [`CodeModel`] implementation created by [`CodeModelBuilder`].
struct ClosureCodeModel<S: 'static> {
    create: Box<dyn Fn() -> S + Send + Sync>,
    eval: EvalFn<S>,
}

impl<S: 'static> CodeModel for ClosureCodeModel<S> {
    type State = S;

    fn create_state(&self) -> S {
        (self.create)()
    }

    fn evaluate(&self, inputs: &CmInputs, state: &mut S) -> CmOutputs {
        (self.eval)(inputs, state)
    }
}

/// Ergonomic builder for constructing [`CodeModelDef`] instances from closures.
///
/// # Example
/// ```
/// use thevenin_xspice::*;
///
/// let model = CodeModelBuilder::new("my_gain")
///     .port("in", PortDirection::In, PortType::Voltage)
///     .port("out", PortDirection::Out, PortType::Current)
///     .param_real("gain", 1.0)
///     .build(|inputs, _state: &mut ()| {
///         let v_in = inputs.port_values[0];
///         let gain = inputs.params[0].as_real().unwrap_or(1.0);
///         let mut out = CmOutputs::new();
///         out.set_output(1, gain * v_in);
///         out.set_partial(1, 0, gain);
///         out
///     });
/// ```
pub struct CodeModelBuilder<S: 'static = ()> {
    name: String,
    ports: Vec<PortDef>,
    params: Vec<ParamDef>,
    create_state: Box<dyn Fn() -> S + Send + Sync>,
}

impl CodeModelBuilder<()> {
    /// Create a new builder with the given model type name and no state.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ports: Vec::new(),
            params: Vec::new(),
            create_state: Box::new(|| ()),
        }
    }
}

impl<S: 'static> CodeModelBuilder<S> {
    /// Add a port definition.
    pub fn port(mut self, name: &str, direction: PortDirection, port_type: PortType) -> Self {
        self.ports.push(PortDef {
            name: name.to_string(),
            direction,
            port_type,
        });
        self
    }

    /// Add a real-valued parameter with a default.
    pub fn param_real(mut self, name: &str, default: f64) -> Self {
        self.params.push(ParamDef {
            name: name.to_string(),
            param_type: ParamType::Real,
            default: ParamValue::Real(default),
        });
        self
    }

    /// Add an integer parameter with a default.
    pub fn param_integer(mut self, name: &str, default: i64) -> Self {
        self.params.push(ParamDef {
            name: name.to_string(),
            param_type: ParamType::Integer,
            default: ParamValue::Integer(default),
        });
        self
    }

    /// Add a boolean parameter with a default.
    pub fn param_boolean(mut self, name: &str, default: bool) -> Self {
        self.params.push(ParamDef {
            name: name.to_string(),
            param_type: ParamType::Boolean,
            default: ParamValue::Boolean(default),
        });
        self
    }

    /// Set the state type and factory, changing the builder's state type parameter.
    pub fn state<S2: 'static>(
        self,
        factory: impl Fn() -> S2 + Send + Sync + 'static,
    ) -> CodeModelBuilder<S2> {
        CodeModelBuilder {
            name: self.name,
            ports: self.ports,
            params: self.params,
            create_state: Box::new(factory),
        }
    }

    /// Build the code model definition with the given evaluation closure.
    pub fn build<F>(self, evaluate: F) -> CodeModelDef
    where
        F: Fn(&CmInputs, &mut S) -> CmOutputs + Send + Sync + 'static,
    {
        let model = ClosureCodeModel {
            create: self.create_state,
            eval: Box::new(evaluate),
        };
        CodeModelDef::new(self.name, self.ports, self.params, model)
    }
}
