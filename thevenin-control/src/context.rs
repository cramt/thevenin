//! Simulation context for `.control` block execution.
//!
//! Holds the netlist, simulation results (plots), variables, and output.

use std::collections::HashMap;

use thevenin_types::{Netlist, SimPlot, SimVector};

/// Simulation context — mutable state during .control execution.
pub struct SimContext {
    /// The parsed netlist (may be mutated by `alter`).
    pub netlist: Netlist,
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
}

impl SimContext {
    /// Create a new context from a netlist.
    pub fn new(netlist: Netlist) -> Self {
        Self {
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
        }
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
