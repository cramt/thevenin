//! Execution-time language registry for Cirq embedded code blocks.
//!
//! Cirq's `code "lang" { … }` blocks ([`cirq_ir::CodeBlock`]) carry a free-form
//! language tag. This module is the **execution** half of the language
//! registry: it maps a tag to a [`LanguageHandler`] that runs the block's body
//! against the live simulation context.
//!
//! The **compile-time** half lives in `cirq_frontend::LanguageRegistry`, which
//! rejects unregistered tags during compilation. A host that registers extra
//! handlers here should mirror the tags into the frontend registry so that
//! validation and execution stay in sync — [`LanguageRegistry::tags`] returns
//! exactly the set to hand to `cirq_frontend::LanguageRegistry::with_languages`.
//!
//! The default registry handles only `"control"` (the ngspice `.control`
//! interpreter), so out of the box behaviour is unchanged.
//!
//! # Handler contract
//!
//! A [`LanguageHandler`] receives:
//! - `lines` — the verbatim block body, one entry per source line.
//! - `parsed` — the IR's pre-parsed typed AST when the IR understood the
//!   language at construction time (today only `"control"`); otherwise `None`,
//!   and the handler parses `lines` itself.
//! - `ctx` — the live [`SimContext`], shared across every block in the circuit.
//!
//! A handler communicates results purely through side effects on `ctx`:
//! appending to [`SimContext::plots`], writing to [`SimContext::output`], and
//! setting [`SimContext::exit_code`] to request that execution stop. It returns
//! `Err(message)` on a hard failure.

use std::collections::HashMap;

use cirq_ir::control::{Statement, parse_control_block};

use crate::context::SimContext;
use crate::exec;

/// A handler that executes one embedded code block against the simulation
/// context. See the [module docs](self) for the contract.
pub trait LanguageHandler {
    /// Execute `lines` (or `parsed`, when available) against `ctx`.
    fn execute(
        &self,
        lines: &[String],
        parsed: Option<&[Statement]>,
        ctx: &mut SimContext,
    ) -> Result<(), String>;
}

/// Built-in handler for the `"control"` language — the ngspice `.control`
/// interpreter. Prefers the IR's pre-parsed AST, falling back to parsing the
/// raw lines for blocks constructed without [`cirq_ir::CodeBlock::from_lines`].
pub struct ControlHandler;

impl LanguageHandler for ControlHandler {
    fn execute(
        &self,
        lines: &[String],
        parsed: Option<&[Statement]>,
        ctx: &mut SimContext,
    ) -> Result<(), String> {
        let fallback;
        let stmts: &[Statement] = match parsed {
            Some(p) => p,
            None => {
                fallback = parse_control_block(lines)?;
                &fallback
            }
        };
        exec::execute(stmts, ctx)
    }
}

/// A map from `code "lang"` tag to its [`LanguageHandler`].
///
/// The [`Default`] registry handles only `"control"`. Hosts add their own
/// languages (`js`, `python`, a custom DSL, …) with [`register`](Self::register).
pub struct LanguageRegistry {
    handlers: HashMap<String, Box<dyn LanguageHandler>>,
}

impl Default for LanguageRegistry {
    /// Handles only the built-in `"control"` language.
    fn default() -> Self {
        Self::with_control()
    }
}

impl LanguageRegistry {
    /// An empty registry that handles no languages.
    pub fn empty() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// A registry with only the built-in `"control"` handler — the default
    /// behaviour of the simulator.
    pub fn with_control() -> Self {
        let mut reg = Self::empty();
        reg.register("control", Box::new(ControlHandler));
        reg
    }

    /// Register a handler for `tag`, replacing any existing handler.
    pub fn register(&mut self, tag: impl Into<String>, handler: Box<dyn LanguageHandler>) {
        self.handlers.insert(tag.into(), handler);
    }

    /// Look up the handler for `tag`, if registered.
    pub fn handler(&self, tag: &str) -> Option<&dyn LanguageHandler> {
        self.handlers.get(tag).map(Box::as_ref)
    }

    /// Whether a handler is registered for `tag`.
    pub fn contains(&self, tag: &str) -> bool {
        self.handlers.contains_key(tag)
    }

    /// The registered language tags. Pass these to
    /// `cirq_frontend::LanguageRegistry::with_languages` to keep compile-time
    /// validation in sync with the handlers registered here.
    pub fn tags(&self) -> impl Iterator<Item = &str> {
        self.handlers.keys().map(String::as_str)
    }
}
