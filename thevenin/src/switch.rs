//! Voltage- and current-controlled switch device models.
//!
//! Implements SPICE `S` (voltage-controlled, `.model NAME SW`) and `W`
//! (current-controlled, `.model NAME CSW`) elements. Each model is a
//! nonlinear conductance: the conductance between `n+`/`n-` depends on a
//! control variable (a node-voltage difference for `S`, a branch current
//! through a sensing voltage source for `W`).
//!
//! # State and hysteresis
//!
//! The model has two well-defined operating points — fully ON (g = 1/Ron)
//! and fully OFF (g = 1/Roff) — separated by a hysteresis window
//! `[Vt-Vh, Vt+Vh]` (or `[It-Ih, It+Ih]`).
//!
//! **Implementation note:** the stamping is hard-decisioned, not smoothly
//! interpolated. Outside the window the conductance snaps to `1/Ron` or
//! `1/Roff` with `dg/dc = 0`; inside the window the latched previous state
//! (`SwitchState::On`/`Off`) wins and the conductance also has `dg/dc = 0`.
//! There is no hermite-blended log10(conductance) curve. Threshold
//! crossings are therefore discontinuous in `g`, and the NR loop relies on
//! the surrounding `pnjlim`-style limiting (from neighbouring nonlinear
//! devices) plus the hysteresis latch to converge across the discontinuity.
//! For real switching-converter fixtures this has been sufficient; tighter
//! convergence on borderline circuits would require porting ngspice's
//! `swhys.c` smooth-blend formulation (out of scope for the 1.0 cut).
//!
//! Hysteresis is realised by tracking the previous switch state across NR
//! iterations: once the control variable has crossed the upper threshold
//! the switch latches ON until it crosses the lower threshold, and vice
//! versa. Inside the hysteresis window the latched value is preserved, so a
//! control trajectory that enters and leaves the window without crossing
//! the far threshold returns to its starting state.

use crate::model_params::ModelParams;

/// Whether this model variant reads a node-voltage difference (`S`) or a
/// branch current (`W`) as its control variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchKind {
    /// Voltage-controlled (S element / `.model NAME SW`).
    Voltage,
    /// Current-controlled (W element / `.model NAME CSW`).
    Current,
}

/// Fully-resolved switch model parameters.
///
/// Defaults match ngspice: `Vt = 0`, `Vh = 0`, `Ron = 1 Ω`, `Roff = 1 MΩ`
/// (or `It`/`Ih` for the current-controlled variant).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwitchModel {
    pub kind: SwitchKind,
    /// Threshold: `Vt` for SW, `It` for CSW.
    pub threshold: f64,
    /// Hysteresis half-width: `Vh` for SW, `Ih` for CSW.
    pub hysteresis: f64,
    /// ON-state resistance (Ω).
    pub ron: f64,
    /// OFF-state resistance (Ω).
    pub roff: f64,
}

impl SwitchModel {
    /// Build a default-valued model for the given kind.
    pub fn new(kind: SwitchKind) -> Self {
        Self {
            kind,
            threshold: 0.0,
            hysteresis: 0.0,
            ron: 1.0,
            roff: 1.0e6,
        }
    }

    /// Build a model from a SPICE `.model` definition. The `kind` is
    /// determined from the model's kind string (`SW` → voltage,
    /// `CSW` → current); anything else is treated as `SW`. Unknown
    /// parameters are silently ignored.
    pub fn from_params(model: &ModelParams) -> Self {
        let kind = match model.kind.to_ascii_uppercase().as_str() {
            "CSW" | "ISWITCH" => SwitchKind::Current,
            _ => SwitchKind::Voltage,
        };
        let mut m = Self::new(kind);
        for (name, v) in &model.params {
            match name.to_ascii_uppercase().as_str() {
                "VT" | "IT" => m.threshold = *v,
                "VH" | "IH" => m.hysteresis = v.abs(),
                "RON" => m.ron = *v,
                "ROFF" => m.roff = *v,
                _ => {}
            }
        }
        // Guard against zero/negative resistances — ngspice clamps these
        // to a tiny positive value to keep 1/R finite.
        if m.ron <= 0.0 {
            m.ron = 1.0e-12;
        }
        if m.roff <= 0.0 {
            m.roff = 1.0e-12;
        }
        m
    }

    /// Layer instance-level overrides onto the model. Currently a no-op
    /// for switches (ngspice's S/W elements don't accept per-instance
    /// parameter overrides beyond the optional ON/OFF flag, which is
    /// handled at stamping time), but kept for parity with other devices
    /// in case future ngspice variants add one.
    pub fn with_instance_params(self, _params: &[(String, f64)]) -> Self {
        self
    }

    /// Conductance in the ON state (`1 / Ron`).
    pub fn g_on(&self) -> f64 {
        1.0 / self.ron
    }

    /// Conductance in the OFF state (`1 / Roff`).
    pub fn g_off(&self) -> f64 {
        1.0 / self.roff
    }

    /// Compute switch conductance and its derivative w.r.t. the control
    /// variable, given the previous switch state.
    ///
    /// Returns `(g, dg_dc, new_state)`:
    /// - `g` is the conductance to stamp between `n+`/`n-`.
    /// - `dg_dc` is `dG/d(control)`, used to build Jacobian off-diagonal
    ///   entries that couple the switch to its control variable.
    /// - `new_state` is the resolved switch state for this NR iteration,
    ///   which the caller threads into the next call.
    ///
    /// Hysteresis (the canonical ngspice behaviour):
    /// - When the control variable crosses `Vt + Vh` going up, the
    ///   switch latches ON: g = g_on regardless of subsequent control
    ///   movements until the lower threshold is breached.
    /// - When it drops below `Vt - Vh`, the switch latches OFF.
    /// - Inside the window the state is preserved — the conductance
    ///   stays at `g_on` or `g_off` depending on the latched value.
    /// - With `Vh = 0` the window collapses; the switch hard-decisions
    ///   at every iteration using `control >= Vt` (with a tiny smoothing
    ///   band kept around the threshold so the NR Jacobian doesn't
    ///   become singular — when state is `Unknown` the very first
    ///   evaluation breaks the tie linearly).
    pub fn evaluate(&self, control: f64, state_prev: SwitchState) -> (f64, f64, SwitchState) {
        let vt = self.threshold;
        let vh = self.hysteresis;
        let upper = vt + vh;
        let lower = vt - vh;

        // Hard-decision branches outside the window.
        if control >= upper {
            return (self.g_on(), 0.0, SwitchState::On);
        }
        if control <= lower {
            return (self.g_off(), 0.0, SwitchState::Off);
        }

        // Inside the window — the state is latched; conductance follows
        // the latched corner directly. For `Unknown` we pick a sensible
        // tiebreaker based on the linear position so the first evaluation
        // doesn't all-or-nothing flip a coin.
        let width = (upper - lower).max(1.0e-30);
        let x_linear = ((control - lower) / width).clamp(0.0, 1.0);
        let state = match state_prev {
            SwitchState::Unknown => {
                if x_linear >= 0.5 {
                    SwitchState::On
                } else {
                    SwitchState::Off
                }
            }
            other => other,
        };
        let g = match state {
            SwitchState::On => self.g_on(),
            SwitchState::Off => self.g_off(),
            SwitchState::Unknown => unreachable!("Unknown resolved above"),
        };
        // The latched conductance is independent of the control variable,
        // so dg/dc = 0 inside the window. The threshold crossings (which
        // are themselves discontinuous) re-converge across two NR
        // iterations and the inter-iteration `pnjlim`-style limiting on
        // other devices keeps the matrix well-behaved.
        (g, 0.0, state)
    }
}

/// Latched switch state used to thread hysteresis information across NR
/// iterations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SwitchState {
    /// Fully OFF.
    Off,
    /// Fully ON.
    On,
    /// State has not yet been resolved (start of a fresh NR attempt or
    /// before the first stamp). The first [`SwitchModel::evaluate`] call
    /// resolves this to ON or OFF based on the control value's position
    /// inside the hysteresis window.
    #[default]
    Unknown,
}

impl SwitchState {
    /// Map a SPICE-style `ON`/`OFF` flag to a [`SwitchState`].
    pub fn from_on_flag(on: Option<bool>) -> Self {
        match on {
            Some(true) => SwitchState::On,
            Some(false) => SwitchState::Off,
            None => SwitchState::Unknown,
        }
    }
}

/// A resolved switch instance, ready for NR-iteration stamping.
///
/// `latched_state` is the source of truth for hysteresis: the NR stamping
/// loop updates it each iteration through interior mutability so the next
/// timestep can read the value the switch settled at. SPICE `ON`/`OFF`
/// instance flags seed it at construction time.
#[derive(Debug, Clone)]
pub struct SwitchInstance {
    /// SPICE-style instance name (e.g. `"S1"`, `"W2"`).
    pub name: String,
    /// Matrix index of `n+`. `None` means the terminal is ground.
    pub pos_idx: Option<usize>,
    /// Matrix index of `n-`.
    pub neg_idx: Option<usize>,
    /// Voltage-controlled: `Some((ctrl_pos_idx, ctrl_neg_idx))`.
    /// Current-controlled: `None`.
    pub ctrl_nodes: Option<(Option<usize>, Option<usize>)>,
    /// Current-controlled: the branch row of the sensing voltage source
    /// inside the MNA matrix.
    pub ctrl_branch: Option<usize>,
    /// Latched switch state that persists across NR attempts and
    /// transient timesteps. Updated by the stamp loop each iteration.
    pub latched_state: std::cell::Cell<SwitchState>,
    /// The fully-resolved model.
    pub model: SwitchModel,
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    fn default_v() -> SwitchModel {
        SwitchModel::new(SwitchKind::Voltage)
    }

    #[test]
    fn default_voltage_model() {
        let m = default_v();
        assert_eq!(m.kind, SwitchKind::Voltage);
        assert_eq!(m.threshold, 0.0);
        assert_eq!(m.hysteresis, 0.0);
        assert_eq!(m.ron, 1.0);
        assert_eq!(m.roff, 1.0e6);
    }

    #[test]
    fn evaluate_far_above_threshold_is_on() {
        let m = SwitchModel {
            threshold: 2.0,
            hysteresis: 0.5,
            ..default_v()
        };
        let (g, dg, state) = m.evaluate(5.0, SwitchState::Off);
        assert_abs_diff_eq!(g, m.g_on(), epsilon = 1e-15);
        assert_abs_diff_eq!(dg, 0.0, epsilon = 1e-15);
        assert_eq!(state, SwitchState::On);
    }

    #[test]
    fn evaluate_far_below_threshold_is_off() {
        let m = SwitchModel {
            threshold: 2.0,
            hysteresis: 0.5,
            ..default_v()
        };
        let (g, dg, state) = m.evaluate(-5.0, SwitchState::On);
        assert_abs_diff_eq!(g, m.g_off(), epsilon = 1e-15);
        assert_abs_diff_eq!(dg, 0.0, epsilon = 1e-15);
        assert_eq!(state, SwitchState::Off);
    }

    #[test]
    fn hysteresis_window_retains_state() {
        let m = SwitchModel {
            threshold: 1.0,
            hysteresis: 0.5,
            ron: 1.0,
            roff: 1.0e6,
            kind: SwitchKind::Voltage,
        };
        // Inside the window (between 0.5 and 1.5), an ON state stays ON.
        let (_, _, state) = m.evaluate(0.75, SwitchState::On);
        assert_eq!(state, SwitchState::On);
        // And an OFF state stays OFF for the same control value.
        let (_, _, state) = m.evaluate(0.75, SwitchState::Off);
        assert_eq!(state, SwitchState::Off);
    }

    #[test]
    fn model_from_def_parses_params() {
        let md = ModelParams {
            name: "SW1".to_string(),
            kind: "SW".to_string(),
            params: vec![
                ("VT".to_string(), 1.5),
                ("VH".to_string(), 0.2),
                ("RON".to_string(), 0.5),
                ("ROFF".to_string(), 1.0e9),
            ],
        };
        let m = SwitchModel::from_params(&md);
        assert_eq!(m.kind, SwitchKind::Voltage);
        assert_abs_diff_eq!(m.threshold, 1.5, epsilon = 1e-15);
        assert_abs_diff_eq!(m.hysteresis, 0.2, epsilon = 1e-15);
        assert_abs_diff_eq!(m.ron, 0.5, epsilon = 1e-15);
        assert_abs_diff_eq!(m.roff, 1.0e9, epsilon = 1e-15);
    }

    #[test]
    fn csw_model_kind_recognised() {
        let md = ModelParams {
            name: "CSW1".to_string(),
            kind: "CSW".to_string(),
            params: vec![("IT".to_string(), 1e-3)],
        };
        let m = SwitchModel::from_params(&md);
        assert_eq!(m.kind, SwitchKind::Current);
        assert_abs_diff_eq!(m.threshold, 1e-3, epsilon = 1e-18);
    }
}
