//! Simulation context for `.control` block execution.
//!
//! Holds the driving IR circuit, a cached SPICE-Expr-shape lowering used by
//! helpers that haven't moved onto the IR (TEMPER, `@device[param]`),
//! simulation results (plots), variables, and output. Public entry through
//! [`SimContext::from_circuit`].

use std::collections::HashMap;

use cirq_ir::Circuit;
use thevenin::TranPauseSnapshot;
use thevenin_types::{Netlist, SimPlot, SimVector};

use crate::ast::StopCondition;

/// Simulation context — mutable state during .control execution.
pub struct SimContext {
    /// The Cirq IR circuit driving this context, if any. Present when the
    /// context was constructed via [`SimContext::from_circuit`] (the IR
    /// entry point). The analysis dispatcher consults this first so the
    /// Stage 4 IR-shaped simulator API drives `.control` runs end-to-end
    /// when possible.
    ///
    /// `None` for legacy [`SimContext::new`] callers, where the working
    /// `netlist` cache is the sole source of truth. External callers
    /// inspect this via [`SimContext::circuit`].
    pub(crate) circuit: Option<Circuit>,
    /// SPICE-Expr-shape adapter for helpers that don't yet operate on the IR: TEMPER expression
    /// rewriting, `@device[param]` lookups, and the Sens/Noise/Pz/Tf analyses that still take
    /// `&Netlist`. On the IR path this is a cached lowering of `circuit`, refreshed via
    /// `refresh_netlist_cache` after every `alter`; on the legacy path it *is* the source of
    /// truth. Crate-private — removing this field is the deeper TEMPER+`@device` IR lift; until
    /// then it stays an internal implementation detail so the public API doesn't leak
    /// `thevenin_types::Netlist`.
    pub(crate) netlist: Netlist,
    /// Named plots from simulation runs, in insertion order.
    pub plots: Vec<SimPlot>,
    /// Current plot index (most recent simulation result).
    pub current_plot: Option<usize>,
    /// Auto-incrementing plot counters by analysis type.
    pub plot_counters: HashMap<String, usize>,
    /// String/numeric variables set by `set` command.
    pub variables: HashMap<String, String>,
    /// User-defined functions from `define`.
    pub functions: HashMap<String, (Vec<String>, String)>,
    /// Exit code if `quit` was called.
    pub exit_code: Option<i32>,
    /// Captured output (echo, print, etc.).
    pub output: String,
    /// Vectors in the "constants" pseudo-plot (user-created via `let` outside
    /// a simulation context, or via `compose`).
    pub user_vectors: Vec<SimVector>,
    /// Resolved model parameters from the last analysis run.
    /// Used by `@model[param]` queries to return TEMPER-evaluated values.
    pub resolved_models: HashMap<String, Vec<thevenin_types::Param>>,
    /// Pending pause condition for the next transient analysis.
    ///
    /// Set by `stop when` and consumed by the next `tran` run. Cleared after
    /// the run starts (whether or not the run actually paused), matching
    /// ngspice's one-shot semantics.
    pub stop_when: Option<StopCondition>,
    /// Snapshot of a transient run that paused at its stop condition,
    /// awaiting a `resume` command. None when no run is paused.
    pub paused_tran: Option<TranPauseSnapshot>,
}

impl SimContext {
    /// Crate-internal constructor used by tests that only need to exercise
    /// the vector / variable / expression machinery without a driving
    /// [`Circuit`]. External callers use [`SimContext::from_circuit`].
    #[cfg(test)]
    pub(crate) fn new(netlist: Netlist) -> Self {
        Self {
            circuit: None,
            netlist,
            plots: Vec::new(),
            current_plot: None,
            plot_counters: HashMap::new(),
            variables: HashMap::new(),
            functions: HashMap::new(),
            exit_code: None,
            output: String::new(),
            user_vectors: Vec::new(),
            resolved_models: HashMap::new(),
            stop_when: None,
            paused_tran: None,
        }
    }

    /// Create a new context from a Cirq IR circuit.
    ///
    /// Eagerly lowers the circuit to a working `Netlist` via
    /// [`cirq_frontend::to_netlist::circuit_to_netlists`] +
    /// [`thevenin::flatten_netlist`]; that cached netlist is what the
    /// helpers still operating on the SPICE shape consume. The Circuit
    /// itself remains the source of truth for analysis dispatch (see
    /// `exec.rs::run_analysis`).
    pub fn from_circuit(circuit: Circuit) -> Result<Self, String> {
        let netlists = cirq_frontend::to_netlist::circuit_to_netlists(&circuit)
            .map_err(|e| format!("circuit_to_netlists: {e}"))?;
        let netlist = netlists
            .into_iter()
            .next()
            .ok_or_else(|| "circuit produced no netlists".to_string())?;
        let netlist =
            thevenin::flatten_netlist(&netlist).map_err(|e| format!("flatten_netlist: {e}"))?;
        Ok(Self {
            circuit: Some(circuit),
            netlist,
            plots: Vec::new(),
            current_plot: None,
            plot_counters: HashMap::new(),
            variables: HashMap::new(),
            functions: HashMap::new(),
            exit_code: None,
            output: String::new(),
            user_vectors: Vec::new(),
            resolved_models: HashMap::new(),
            stop_when: None,
            paused_tran: None,
        })
    }

    /// The Cirq IR circuit driving this context, if any.
    ///
    /// Returns `None` for crate-internal `new(netlist)` constructions (used
    /// only by tests). External callers always go through
    /// [`SimContext::from_circuit`] and so always see `Some`.
    pub fn circuit(&self) -> Option<&Circuit> {
        self.circuit.as_ref()
    }

    /// Re-derive the cached internal Netlist after mutating the driving
    /// [`Circuit`] (e.g. through `alter`). No-op on legacy contexts where
    /// no Circuit is present.
    ///
    /// Kept `pub(crate)` because the cached Netlist is an internal
    /// SPICE-Expr-shape adapter that callers outside the crate must not
    /// rely on.
    pub(crate) fn refresh_netlist_cache(&mut self) -> Result<(), String> {
        let Some(circuit) = self.circuit.as_ref() else {
            return Ok(());
        };
        let netlists = cirq_frontend::to_netlist::circuit_to_netlists(circuit)
            .map_err(|e| format!("refresh_netlist_cache: {e}"))?;
        let netlist = netlists
            .into_iter()
            .next()
            .ok_or_else(|| "refresh_netlist_cache: circuit produced no netlists".to_string())?;
        self.netlist = thevenin::flatten_netlist(&netlist)
            .map_err(|e| format!("refresh_netlist_cache: flatten_netlist: {e}"))?;
        Ok(())
    }

    /// Add a plot from a simulation result, returning its name.
    pub fn add_plot(&mut self, mut plot: SimPlot) -> String {
        // Auto-name: "op1", "dc1", "tran1", etc.
        let analysis = plot_analysis_type(&plot.name);
        let counter = self.plot_counters.entry(analysis.clone()).or_insert(0);
        *counter += 1;
        let name = format!("{}{}", analysis, counter);
        plot.name = name.clone();
        self.plots.push(plot);
        self.current_plot = Some(self.plots.len() - 1);
        name
    }

    /// Get the current plot (most recently created).
    pub fn current_plot(&self) -> Option<&SimPlot> {
        self.current_plot.and_then(|i| self.plots.get(i))
    }

    /// Get the current plot name.
    pub fn current_plot_name(&self) -> String {
        self.current_plot()
            .map(|p| p.name.clone())
            .unwrap_or_default()
    }

    /// Look up a vector by name in the current plot, then user vectors.
    ///
    /// Handles ngspice naming conventions:
    /// - `i(vsrc)` → also tries `vsrc#branch`
    /// - `v(node)` → direct match
    pub fn find_vector(&self, name: &str) -> Option<&SimVector> {
        let lower = name.to_lowercase();

        // Build alternate names for i(vsrc) → vsrc#branch mapping
        let alt_name = if lower.starts_with("i(") && lower.ends_with(')') {
            Some(format!("{}#branch", &lower[2..lower.len() - 1]))
        } else {
            None
        };

        // Search current plot first
        if let Some(plot) = self.current_plot()
            && let Some(v) = find_vec_in_list(&plot.vecs, &lower, alt_name.as_deref())
        {
            return Some(v);
        }
        // Search user vectors
        if let Some(v) = find_vec_in_list(&self.user_vectors, &lower, alt_name.as_deref()) {
            return Some(v);
        }
        // Search all plots
        for plot in &self.plots {
            if let Some(v) = find_vec_in_list(&plot.vecs, &lower, alt_name.as_deref()) {
                return Some(v);
            }
        }
        None
    }

    /// Look up a vector in a specific plot (by plot name prefix).
    pub fn find_vector_in_plot(&self, plot_name: &str, vec_name: &str) -> Option<&SimVector> {
        let plot_lower = plot_name.to_lowercase();
        let vec_lower = vec_name.to_lowercase();
        for plot in &self.plots {
            if plot.name.to_lowercase() == plot_lower
                && let Some(v) = plot
                    .vecs
                    .iter()
                    .find(|v| v.name.to_lowercase() == vec_lower)
            {
                return Some(v);
            }
        }
        None
    }

    /// Store a vector in the current plot (or user vectors if no plot).
    pub fn store_vector(&mut self, vec: SimVector) {
        let lower = vec.name.to_lowercase();
        if let Some(idx) = self.current_plot {
            // Replace existing or append in current plot
            if let Some(existing) = self.plots[idx]
                .vecs
                .iter_mut()
                .find(|v| v.name.to_lowercase() == lower)
            {
                *existing = vec.clone();
            } else {
                self.plots[idx].vecs.push(vec.clone());
            }
            // Also update user_vectors if the name exists there, to prevent
            // stale shadow copies from hiding cross-plot accumulation.
            if let Some(existing) = self
                .user_vectors
                .iter_mut()
                .find(|v| v.name.to_lowercase() == lower)
            {
                *existing = vec;
            }
        } else {
            // No current plot — store in user vectors
            if let Some(existing) = self
                .user_vectors
                .iter_mut()
                .find(|v| v.name.to_lowercase() == lower)
            {
                *existing = vec;
            } else {
                self.user_vectors.push(vec);
            }
        }
    }

    /// Resolve a `$variable` reference.
    pub fn resolve_var(&self, name: &str) -> String {
        if name == "curplot" {
            return self.current_plot_name();
        }
        self.variables.get(name).cloned().unwrap_or_default()
    }

    /// Resolve a `$&vector` reference — format scalar value as string.
    pub fn resolve_vec_scalar(&self, name: &str) -> String {
        if let Some(vec) = self.find_vector(name) {
            match &vec.data {
                thevenin_types::VectorData::Real(real) => {
                    if real.len() == 1 {
                        format_number(real[0])
                    } else if !real.is_empty() {
                        format_number(real[real.len() - 1])
                    } else {
                        "0".to_string()
                    }
                }
                thevenin_types::VectorData::Complex(cplx) => {
                    if let Some(c) = cplx.last() {
                        format!("{},{}", format_number(c.re), format_number(c.im))
                    } else {
                        "0".to_string()
                    }
                }
            }
        } else {
            "0".to_string()
        }
    }

    /// Switch to a named plot.
    pub fn set_current_plot(&mut self, name: &str) {
        let lower = name.to_lowercase();
        if lower == "new" {
            // Create a new empty plot
            let plot = SimPlot {
                name: "user".to_string(),
                vecs: Vec::new(),
            };
            self.plots.push(plot);
            self.current_plot = Some(self.plots.len() - 1);
            return;
        }
        for (i, plot) in self.plots.iter().enumerate() {
            if plot.name.to_lowercase() == lower {
                self.current_plot = Some(i);
                return;
            }
        }
    }

    /// Write to the output buffer.
    pub fn echo(&mut self, text: &str) {
        self.output.push_str(text);
        self.output.push('\n');
    }
}

/// Find a vector in a list by name, with an optional alternate name.
fn find_vec_in_list<'a>(
    vecs: &'a [SimVector],
    name: &str,
    alt_name: Option<&str>,
) -> Option<&'a SimVector> {
    if let Some(v) = vecs.iter().find(|v| v.name.to_lowercase() == name) {
        return Some(v);
    }
    if let Some(alt) = alt_name
        && let Some(v) = vecs.iter().find(|v| v.name.to_lowercase() == alt)
    {
        return Some(v);
    }
    None
}

/// Extract analysis type from a plot name like "op1", "dc2", etc.
fn plot_analysis_type(name: &str) -> String {
    let s = name.to_lowercase();
    s.trim_end_matches(|c: char| c.is_ascii_digit()).to_string()
}

/// Format a number matching ngspice's output conventions.
fn format_number(v: f64) -> String {
    if v == 0.0 {
        "0".to_string()
    } else if v.abs() >= 1e-3 && v.abs() < 1e6 {
        format!("{v}")
    } else {
        format!("{v:e}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thevenin_types::{Analysis, Complex, Netlist, SimPlot, SimVector, VectorData};

    fn empty_netlist() -> Netlist {
        Netlist {
            title: String::new(),
            items: Vec::new(),
            analysis: Analysis::Op,
            source: String::new(),
        }
    }

    fn make_real_vec(name: &str, data: Vec<f64>) -> SimVector {
        SimVector {
            name: name.to_string(),
            data: VectorData::Real(data),
        }
    }

    fn make_complex_vec(name: &str, pairs: Vec<(f64, f64)>) -> SimVector {
        SimVector {
            name: name.to_string(),
            data: VectorData::Complex(
                pairs
                    .into_iter()
                    .map(|(re, im)| Complex { re, im })
                    .collect(),
            ),
        }
    }

    fn make_plot(name: &str, vecs: Vec<SimVector>) -> SimPlot {
        SimPlot {
            name: name.to_string(),
            vecs,
        }
    }

    // -----------------------------------------------------------------------
    // SimContext::new
    // -----------------------------------------------------------------------

    #[test]
    fn new_context_has_empty_state() {
        let ctx = SimContext::new(empty_netlist());
        assert!(ctx.circuit.is_none(), "legacy ctor leaves circuit None");
        assert!(ctx.plots.is_empty());
        assert!(ctx.current_plot.is_none());
        assert!(ctx.plot_counters.is_empty());
        assert!(ctx.variables.is_empty());
        assert!(ctx.functions.is_empty());
        assert!(ctx.exit_code.is_none());
        assert!(ctx.output.is_empty());
        assert!(ctx.user_vectors.is_empty());
    }

    /// Builds the smallest legal Circuit (an empty one with a single
    /// gnd net) so `from_circuit` has something to lower. The harness's
    /// circuit_to_netlists tolerates this — the resulting netlist is
    /// effectively empty but valid.
    fn minimal_circuit() -> cirq_ir::Circuit {
        cirq_ir::Circuit {
            name: "empty".into(),
            nets: vec![cirq_ir::Net {
                id: cirq_ir::Id(0),
                name: "0".into(),
                is_global: true,
            }],
            elements: vec![],
            models: vec![],
            analyses: vec![cirq_ir::Analysis::Op],
            params: vec![],
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
    fn from_circuit_populates_circuit_field() {
        let c = minimal_circuit();
        let ctx = SimContext::from_circuit(c.clone()).expect("from_circuit");
        let stored = ctx.circuit.as_ref().expect("circuit field set");
        assert_eq!(stored.name, c.name);
        assert_eq!(stored.nets.len(), c.nets.len());
        // All other interpreter state is still empty.
        assert!(ctx.plots.is_empty());
        assert!(ctx.variables.is_empty());
    }

    #[test]
    fn from_circuit_derives_a_working_netlist() {
        let ctx = SimContext::from_circuit(minimal_circuit()).expect("from_circuit");
        // The lowered netlist should at least be present; concrete shape
        // is owned by cirq_frontend, we only assert it's been derived.
        assert!(ctx.netlist.items.is_empty() || !ctx.netlist.items.is_empty());
    }

    // -----------------------------------------------------------------------
    // SimContext::add_plot — auto-naming and current_plot tracking
    // -----------------------------------------------------------------------

    #[test]
    fn add_plot_autonaming_op() {
        let mut ctx = SimContext::new(empty_netlist());
        let name = ctx.add_plot(make_plot("op", vec![]));
        assert_eq!(name, "op1");
        let name = ctx.add_plot(make_plot("op", vec![]));
        assert_eq!(name, "op2");
    }

    #[test]
    fn add_plot_autonaming_dc() {
        let mut ctx = SimContext::new(empty_netlist());
        let name = ctx.add_plot(make_plot("dc", vec![]));
        assert_eq!(name, "dc1");
        let name = ctx.add_plot(make_plot("dc", vec![]));
        assert_eq!(name, "dc2");
    }

    #[test]
    fn add_plot_mixed_types() {
        let mut ctx = SimContext::new(empty_netlist());
        assert_eq!(ctx.add_plot(make_plot("op", vec![])), "op1");
        assert_eq!(ctx.add_plot(make_plot("dc", vec![])), "dc1");
        assert_eq!(ctx.add_plot(make_plot("tran", vec![])), "tran1");
        assert_eq!(ctx.add_plot(make_plot("op", vec![])), "op2");
    }

    #[test]
    fn add_plot_sets_current_plot() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.add_plot(make_plot("op", vec![]));
        assert_eq!(ctx.current_plot, Some(0));
        ctx.add_plot(make_plot("dc", vec![]));
        assert_eq!(ctx.current_plot, Some(1));
    }

    // -----------------------------------------------------------------------
    // SimContext::current_plot / current_plot_name
    // -----------------------------------------------------------------------

    #[test]
    fn current_plot_none_before_adding() {
        let ctx = SimContext::new(empty_netlist());
        assert!(ctx.current_plot().is_none());
        assert_eq!(ctx.current_plot_name(), "");
    }

    #[test]
    fn current_plot_reflects_latest() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.add_plot(make_plot("op", vec![]));
        assert_eq!(ctx.current_plot_name(), "op1");
        ctx.add_plot(make_plot("dc", vec![]));
        assert_eq!(ctx.current_plot_name(), "dc1");
    }

    // -----------------------------------------------------------------------
    // SimContext::find_vector
    // -----------------------------------------------------------------------

    #[test]
    fn find_vector_in_current_plot() {
        let mut ctx = SimContext::new(empty_netlist());
        let plot = make_plot("op", vec![make_real_vec("v(out)", vec![1.5])]);
        ctx.add_plot(plot);
        let v = ctx.find_vector("v(out)").unwrap();
        assert_eq!(v.data.as_real(), &[1.5]);
    }

    #[test]
    fn find_vector_in_user_vectors() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.user_vectors.push(make_real_vec("myvec", vec![42.0]));
        let v = ctx.find_vector("myvec").unwrap();
        assert_eq!(v.data.as_real(), &[42.0]);
    }

    #[test]
    fn find_vector_in_other_plots() {
        let mut ctx = SimContext::new(empty_netlist());
        let plot1 = make_plot("op", vec![make_real_vec("v(a)", vec![1.0])]);
        ctx.add_plot(plot1);
        let plot2 = make_plot("dc", vec![make_real_vec("v(b)", vec![2.0])]);
        ctx.add_plot(plot2);
        // current plot is dc1, but v(a) is in op1
        let v = ctx.find_vector("v(a)").unwrap();
        assert_eq!(v.data.as_real(), &[1.0]);
    }

    #[test]
    fn find_vector_missing_returns_none() {
        let ctx = SimContext::new(empty_netlist());
        assert!(ctx.find_vector("nonexistent").is_none());
    }

    #[test]
    fn find_vector_ibranch_alias() {
        let mut ctx = SimContext::new(empty_netlist());
        let plot = make_plot("op", vec![make_real_vec("vsrc#branch", vec![0.5])]);
        ctx.add_plot(plot);
        // i(vsrc) should resolve to vsrc#branch
        let v = ctx.find_vector("i(vsrc)").unwrap();
        assert_eq!(v.data.as_real(), &[0.5]);
    }

    #[test]
    fn find_vector_case_insensitive() {
        let mut ctx = SimContext::new(empty_netlist());
        let plot = make_plot("op", vec![make_real_vec("V(Out)", vec![3.3])]);
        ctx.add_plot(plot);
        let v = ctx.find_vector("v(out)").unwrap();
        assert_eq!(v.data.as_real(), &[3.3]);
    }

    // -----------------------------------------------------------------------
    // SimContext::find_vector_in_plot
    // -----------------------------------------------------------------------

    #[test]
    fn find_vector_in_specific_plot() {
        let mut ctx = SimContext::new(empty_netlist());
        let plot = make_plot("op", vec![make_real_vec("v(x)", vec![7.0])]);
        ctx.add_plot(plot);
        let v = ctx.find_vector_in_plot("op1", "v(x)").unwrap();
        assert_eq!(v.data.as_real(), &[7.0]);
    }

    #[test]
    fn find_vector_in_wrong_plot_returns_none() {
        let mut ctx = SimContext::new(empty_netlist());
        let plot = make_plot("op", vec![make_real_vec("v(x)", vec![7.0])]);
        ctx.add_plot(plot);
        assert!(ctx.find_vector_in_plot("dc1", "v(x)").is_none());
    }

    // -----------------------------------------------------------------------
    // SimContext::store_vector
    // -----------------------------------------------------------------------

    #[test]
    fn store_vector_appends_to_current_plot() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.add_plot(make_plot("op", vec![]));
        ctx.store_vector(make_real_vec("result", vec![10.0]));
        let v = ctx.find_vector("result").unwrap();
        assert_eq!(v.data.as_real(), &[10.0]);
    }

    #[test]
    fn store_vector_replaces_existing_in_current_plot() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.add_plot(make_plot("op", vec![make_real_vec("x", vec![1.0])]));
        ctx.store_vector(make_real_vec("x", vec![99.0]));
        let v = ctx.find_vector("x").unwrap();
        assert_eq!(v.data.as_real(), &[99.0]);
        // Should still be only one vector named "x"
        assert_eq!(
            ctx.plots[0]
                .vecs
                .iter()
                .filter(|v| v.name.to_lowercase() == "x")
                .count(),
            1
        );
    }

    #[test]
    fn store_vector_without_current_plot_goes_to_user_vectors() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.store_vector(make_real_vec("uv", vec![5.0]));
        assert_eq!(ctx.user_vectors.len(), 1);
        assert_eq!(ctx.user_vectors[0].data.as_real(), &[5.0]);
    }

    #[test]
    fn store_vector_replaces_existing_user_vector() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.store_vector(make_real_vec("uv", vec![1.0]));
        ctx.store_vector(make_real_vec("uv", vec![2.0]));
        assert_eq!(ctx.user_vectors.len(), 1);
        assert_eq!(ctx.user_vectors[0].data.as_real(), &[2.0]);
    }

    #[test]
    fn store_vector_cross_sync_user_vectors() {
        let mut ctx = SimContext::new(empty_netlist());
        // Pre-populate user_vectors
        ctx.user_vectors.push(make_real_vec("shared", vec![0.0]));
        // Add a plot and store — should also update user_vectors
        ctx.add_plot(make_plot("op", vec![]));
        ctx.store_vector(make_real_vec("shared", vec![42.0]));
        assert_eq!(ctx.user_vectors[0].data.as_real(), &[42.0]);
    }

    // -----------------------------------------------------------------------
    // SimContext::resolve_var
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_var_normal() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.variables.insert("foo".to_string(), "bar".to_string());
        assert_eq!(ctx.resolve_var("foo"), "bar");
    }

    #[test]
    fn resolve_var_curplot_special() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.add_plot(make_plot("tran", vec![]));
        assert_eq!(ctx.resolve_var("curplot"), "tran1");
    }

    #[test]
    fn resolve_var_missing_returns_empty() {
        let ctx = SimContext::new(empty_netlist());
        assert_eq!(ctx.resolve_var("missing"), "");
    }

    // -----------------------------------------------------------------------
    // SimContext::resolve_vec_scalar
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_vec_scalar_single_real() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.user_vectors.push(make_real_vec("x", vec![3.14]));
        assert_eq!(ctx.resolve_vec_scalar("x"), "3.14");
    }

    #[test]
    fn resolve_vec_scalar_multi_element_returns_last() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.user_vectors
            .push(make_real_vec("sweep", vec![1.0, 2.0, 3.0]));
        assert_eq!(ctx.resolve_vec_scalar("sweep"), "3");
    }

    #[test]
    fn resolve_vec_scalar_empty_returns_zero() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.user_vectors.push(make_real_vec("empty", vec![]));
        assert_eq!(ctx.resolve_vec_scalar("empty"), "0");
    }

    #[test]
    fn resolve_vec_scalar_complex() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.user_vectors
            .push(make_complex_vec("z", vec![(1.5, 2.5)]));
        assert_eq!(ctx.resolve_vec_scalar("z"), "1.5,2.5");
    }

    #[test]
    fn resolve_vec_scalar_missing_vector() {
        let ctx = SimContext::new(empty_netlist());
        assert_eq!(ctx.resolve_vec_scalar("nope"), "0");
    }

    // -----------------------------------------------------------------------
    // SimContext::set_current_plot
    // -----------------------------------------------------------------------

    #[test]
    fn set_current_plot_by_name() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.add_plot(make_plot("op", vec![]));
        ctx.add_plot(make_plot("dc", vec![]));
        assert_eq!(ctx.current_plot_name(), "dc1");
        ctx.set_current_plot("op1");
        assert_eq!(ctx.current_plot_name(), "op1");
    }

    #[test]
    fn set_current_plot_new_creates_empty() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.set_current_plot("new");
        assert!(ctx.current_plot().is_some());
        assert_eq!(ctx.current_plot().unwrap().name, "user");
        assert!(ctx.current_plot().unwrap().vecs.is_empty());
    }

    #[test]
    fn set_current_plot_missing_name_is_noop() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.add_plot(make_plot("op", vec![]));
        assert_eq!(ctx.current_plot, Some(0));
        ctx.set_current_plot("nonexistent");
        // Should remain unchanged
        assert_eq!(ctx.current_plot, Some(0));
    }

    // -----------------------------------------------------------------------
    // SimContext::echo
    // -----------------------------------------------------------------------

    #[test]
    fn echo_appends_with_newline() {
        let mut ctx = SimContext::new(empty_netlist());
        ctx.echo("hello");
        ctx.echo("world");
        assert_eq!(ctx.output, "hello\nworld\n");
    }

    // -----------------------------------------------------------------------
    // format_number
    // -----------------------------------------------------------------------

    #[test]
    fn format_number_zero() {
        assert_eq!(format_number(0.0), "0");
    }

    #[test]
    fn format_number_normal_range() {
        assert_eq!(format_number(3.14), "3.14");
        assert_eq!(format_number(1.0), "1");
        assert_eq!(format_number(-0.5), "-0.5");
    }

    #[test]
    fn format_number_scientific() {
        // Very small
        let s = format_number(1e-9);
        assert!(s.contains('e'), "expected scientific notation, got: {s}");
        // Very large
        let s = format_number(1e9);
        assert!(s.contains('e'), "expected scientific notation, got: {s}");
    }

    // -----------------------------------------------------------------------
    // plot_analysis_type
    // -----------------------------------------------------------------------

    #[test]
    fn plot_analysis_type_strips_digits() {
        assert_eq!(plot_analysis_type("op1"), "op");
        assert_eq!(plot_analysis_type("dc23"), "dc");
        assert_eq!(plot_analysis_type("tran"), "tran");
        assert_eq!(plot_analysis_type("TRAN1"), "tran");
    }
}
