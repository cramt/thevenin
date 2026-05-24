//! Ideal lossless transmission line (T element).
//!
//! Implements the SPICE `T` element: a 2-port lossless delay line
//! characterised by a single impedance `Z0` and propagation delay `TD`.
//!
//! Reference: ngspice `src/spicelib/devices/tra/` (the model labelled "TRA").
//!
//! # Model
//!
//! Each port adds one branch current to the MNA system (one extra row per
//! port). The branch equations encode the standard travelling-wave
//! formulation of a lossless line:
//!
//! ```text
//! V1(t) - Z0 * I1(t) = V2(t - TD) + Z0 * I2(t - TD)
//! V2(t) - Z0 * I2(t) = V1(t - TD) + Z0 * I1(t - TD)
//! ```
//!
//! In DC (no past history available), both right-hand sides collapse to
//! their current values, which after a little algebra forces `V1 = V2`
//! and `I1 = -I2` — the line is a wire at zero frequency.
//!
//! In AC at angular frequency ω, the time-shift becomes a phasor delay
//! `exp(-jωTD)`, giving
//!
//! ```text
//! V1 - Z0 * I1 = exp(-jωTD) * (V2 + Z0 * I2)
//! V2 - Z0 * I2 = exp(-jωTD) * (V1 + Z0 * I1)
//! ```
//!
//! which is the closed-form lossless-line ABCD matrix in branch-current
//! form. At ω = 0 this collapses to V1 = V2, I1 = -I2 (the same wire as
//! DC), so the OP and the AC sweep at low frequency agree.
//!
//! The transient stamp maintains a [`VecDeque`] history of
//! `(t, V1, I1, V2, I2)` samples; the excitation term is the linearly
//! interpolated value at `t - TD`. Samples older than `t - 2 * TD` are
//! pruned to bound memory.

use std::collections::VecDeque;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Static model parameters for an ideal lossless transmission line.
#[derive(Debug, Clone)]
pub struct TlineModel {
    /// Characteristic impedance Z0 in ohms.
    pub z0: f64,
    /// One-way propagation delay TD in seconds.
    pub td: f64,
    /// Optional initial conditions (`V1, I1, V2, I2`).
    pub ic: Option<[f64; 4]>,
}

impl TlineModel {
    /// Build a `TlineModel` from raw `Z0`/`TD` (with optional `F`/`NL`).
    ///
    /// Either `TD` or `F` (with optional `NL`, defaulting to 0.25
    /// wavelengths) is required. If both are given, `TD` wins to match
    /// ngspice's `traparam.c`.
    pub fn new(z0: f64, td: Option<f64>, f: Option<f64>, nl: Option<f64>) -> Self {
        let td = td.unwrap_or_else(|| {
            let nl = nl.unwrap_or(0.25);
            let f = f.unwrap_or(1.0e9);
            if f > 0.0 { nl / f } else { 0.0 }
        });
        Self { z0, td, ic: None }
    }
}

// ---------------------------------------------------------------------------
// Instance
// ---------------------------------------------------------------------------

/// A resolved T-line instance with matrix indices.
///
/// Each port adds a single branch row (`br_eq1` / `br_eq2`) to the MNA
/// system. No internal nodes are needed — the Z0 series resistance is
/// absorbed directly into the branch equation, mirroring how
/// [`crate::ltra::stamp_ltra_transient`] handles the lossless case.
#[derive(Debug, Clone)]
pub struct TlineInstance {
    pub name: String,
    pub pos1_idx: Option<usize>,
    pub neg1_idx: Option<usize>,
    pub pos2_idx: Option<usize>,
    pub neg2_idx: Option<usize>,
    /// MNA branch row for port-1 current.
    pub br_eq1: usize,
    /// MNA branch row for port-2 current.
    pub br_eq2: usize,
    pub model: TlineModel,
}

// ---------------------------------------------------------------------------
// Transient history
// ---------------------------------------------------------------------------

/// One past sample of port voltages and currents.
#[derive(Debug, Clone, Copy)]
pub struct TlineHistoryEntry {
    pub t: f64,
    pub v1: f64,
    pub i1: f64,
    pub v2: f64,
    pub i2: f64,
}

/// History buffer for one T-line instance.
///
/// Stores `(t, V1, I1, V2, I2)` samples in chronological order. The
/// transient stamp interpolates the delayed signal linearly between two
/// adjacent samples. Samples older than `t_cur - 2*TD` are pruned to
/// bound memory.
#[derive(Debug, Clone, Default)]
pub struct TlineState {
    pub history: VecDeque<TlineHistoryEntry>,
    /// Initial voltage / current at port 1 (used before history reaches t-TD).
    pub init_v1: f64,
    pub init_i1: f64,
    pub init_v2: f64,
    pub init_i2: f64,
}

impl TlineState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed the history with a single DC-op sample and remember the initial
    /// values for the delay-not-yet-reached case.
    pub fn init_from_dc(&mut self, v1: f64, i1: f64, v2: f64, i2: f64) {
        self.init_v1 = v1;
        self.init_i1 = i1;
        self.init_v2 = v2;
        self.init_i2 = i2;
        self.history.clear();
        self.history.push_back(TlineHistoryEntry {
            t: 0.0,
            v1,
            i1,
            v2,
            i2,
        });
    }

    /// Append a sample at time `t` and prune samples older than `t - 2*TD`.
    pub fn accept(&mut self, t: f64, v1: f64, i1: f64, v2: f64, i2: f64, td: f64) {
        self.history
            .push_back(TlineHistoryEntry { t, v1, i1, v2, i2 });
        // Keep enough back-history so the next interpolation at `t + h` for
        // any reasonable `h` still has a sample below `t - TD`. Pruning
        // anything older than `t_cur - 2*TD` is comfortably conservative.
        let cutoff = t - 2.0 * td;
        while self.history.len() > 2 {
            let next_is_still_old = self.history.get(1).is_some_and(|e| e.t < cutoff);
            if next_is_still_old {
                self.history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Interpolate `(V1, I1, V2, I2)` at `target_t` via linear interpolation
    /// between adjacent history samples. If `target_t <= 0` or no history
    /// has accumulated yet, the initial values are returned.
    pub fn interpolate(&self, target_t: f64) -> (f64, f64, f64, f64) {
        if self.history.is_empty() || target_t <= self.history.front().map(|e| e.t).unwrap_or(0.0) {
            return (self.init_v1, self.init_i1, self.init_v2, self.init_i2);
        }
        // Walk forward to find the first sample with t >= target_t.
        let mut prev: Option<&TlineHistoryEntry> = None;
        for entry in &self.history {
            if entry.t >= target_t {
                match prev {
                    Some(p) if entry.t > p.t => {
                        let f = (target_t - p.t) / (entry.t - p.t);
                        return (
                            p.v1 + f * (entry.v1 - p.v1),
                            p.i1 + f * (entry.i1 - p.i1),
                            p.v2 + f * (entry.v2 - p.v2),
                            p.i2 + f * (entry.i2 - p.i2),
                        );
                    }
                    _ => return (entry.v1, entry.i1, entry.v2, entry.i2),
                }
            }
            prev = Some(entry);
        }
        // Past the end of history — return the latest sample.
        let last = self.history.back().copied().unwrap_or(TlineHistoryEntry {
            t: 0.0,
            v1: self.init_v1,
            i1: self.init_i1,
            v2: self.init_v2,
            i2: self.init_i2,
        });
        (last.v1, last.i1, last.v2, last.i2)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn add_mna(system: &mut crate::sparse::LinearSystem, row: usize, col: Option<usize>, val: f64) {
    if let Some(c) = col {
        system.matrix.add(row, c, val);
    }
}

fn add_mna_opt(system: &mut crate::sparse::LinearSystem, row: Option<usize>, col: usize, val: f64) {
    if let Some(r) = row {
        system.matrix.add(r, col, val);
    }
}

/// Stamp the common per-port topology shared by DC, transient, and AC: each
/// branch current enters `pos` and exits `neg`, and the branch row's
/// left-hand side is `admit * V(pos) - admit * V(neg) - I`.
///
/// Matches the [`crate::ltra::stamp_ltra_transient`] LC pattern: no
/// internal nodes — the Z0 series impedance is absorbed into the branch
/// equation via `admit = 1/Z0`.
fn stamp_port_topology(
    system: &mut crate::sparse::LinearSystem,
    pos_idx: Option<usize>,
    neg_idx: Option<usize>,
    br_idx: usize,
    admit: f64,
) {
    add_mna(system, br_idx, pos_idx, admit);
    add_mna(system, br_idx, neg_idx, -admit);
    system.matrix.add(br_idx, br_idx, -1.0);
    add_mna_opt(system, pos_idx, br_idx, 1.0);
    add_mna_opt(system, neg_idx, br_idx, -1.0);
}

// ---------------------------------------------------------------------------
// DC stamp
// ---------------------------------------------------------------------------

/// Stamp the T-line for DC analysis.
///
/// Encodes the two DC constraints of an ideal lossless line as a pair of
/// distinct rows (matching `crate::ltra::stamp_ltra_dc`'s LC-case form,
/// which the sparse LU factors cleanly):
///
/// ```text
///   row br1:  I1 + I2 = 0                          (wire — series current balance)
///   row br2:  V(pos1) - V(neg1) - V(pos2) + V(neg2) = 0
/// ```
///
/// The branch currents enter `pos`/exit `neg` at each port (the standard
/// MNA topology contribution).
pub fn stamp_tline_dc(
    inst: &TlineInstance,
    system: &mut crate::sparse::LinearSystem,
    num_nodes: usize,
) {
    let br1 = num_nodes + inst.br_eq1;
    let br2 = num_nodes + inst.br_eq2;

    // KCL: each branch current enters its `pos` node and exits its `neg`
    // node, exactly like a voltage-source MNA contribution.
    add_mna_opt(system, inst.pos1_idx, br1, 1.0);
    add_mna_opt(system, inst.neg1_idx, br1, -1.0);
    add_mna_opt(system, inst.pos2_idx, br2, 1.0);
    add_mna_opt(system, inst.neg2_idx, br2, -1.0);

    // Branch row 1: I1 + I2 = 0 (series-current balance).
    system.matrix.add(br1, br1, 1.0);
    system.matrix.add(br1, br2, 1.0);

    // Branch row 2: V(pos1) - V(neg1) - V(pos2) + V(neg2) = 0 (wire).
    add_mna(system, br2, inst.pos1_idx, 1.0);
    add_mna(system, br2, inst.neg1_idx, -1.0);
    add_mna(system, br2, inst.pos2_idx, -1.0);
    add_mna(system, br2, inst.neg2_idx, 1.0);
}

// ---------------------------------------------------------------------------
// Transient stamp
// ---------------------------------------------------------------------------

/// Stamp the T-line for transient analysis at time `t`.
///
/// The two branch rows take the standard travelling-wave form
///
/// ```text
///   admit * V1(t) - I1(t) = admit * V2(t - TD) + I2(t - TD)
///   admit * V2(t) - I2(t) = admit * V1(t - TD) + I1(t - TD)
/// ```
///
/// Delayed values come from the linearly-interpolated history buffer; for
/// `t < TD` the initial-condition values seeded by [`TlineState::init_from_dc`]
/// (or the `IC=` parameter, when supplied) are used.
pub fn stamp_tline_transient(
    inst: &TlineInstance,
    state: &TlineState,
    system: &mut crate::sparse::LinearSystem,
    num_nodes: usize,
    cur_time: f64,
) {
    let admit = 1.0 / inst.model.z0;
    let br1 = num_nodes + inst.br_eq1;
    let br2 = num_nodes + inst.br_eq2;

    stamp_port_topology(system, inst.pos1_idx, inst.neg1_idx, br1, admit);
    stamp_port_topology(system, inst.pos2_idx, inst.neg2_idx, br2, admit);

    let target = cur_time - inst.model.td;
    let (v1_d, i1_d, v2_d, i2_d) = state.interpolate(target);

    // Excitation from the far port travelling wave.
    system.rhs[br1] += admit * v2_d + i2_d;
    system.rhs[br2] += admit * v1_d + i1_d;
}

// ---------------------------------------------------------------------------
// AC stamp
// ---------------------------------------------------------------------------

/// Stamp the T-line for AC small-signal analysis at angular frequency ω.
///
/// The branch equations in phasor form are
///
/// ```text
///   admit * V1 - I1 = exp(-jωTD) * (admit * V2 + I2)
///   admit * V2 - I2 = exp(-jωTD) * (admit * V1 + I1)
/// ```
///
/// (the lossless line reduces to a delay of TD on the travelling wave
/// `V + Z0 * I`). At ω = 0 this collapses to the DC wire constraint
/// `V1 = V2, I1 = -I2`, so the AC sweep at low frequency agrees with the
/// OP solve up to numerical noise.
pub fn stamp_tline_ac(
    inst: &TlineInstance,
    sys: &mut crate::sparse::ComplexLinearSystem,
    num_nodes: usize,
    omega: f64,
) {
    let admit = 1.0 / inst.model.z0;
    let td = inst.model.td;
    let br1 = num_nodes + inst.br_eq1;
    let br2 = num_nodes + inst.br_eq2;

    // KCL contributions: each branch current enters its pos node and exits
    // its neg node (real part — purely topological).
    add_mna_opt_complex(sys, inst.pos1_idx, br1, 1.0, 0.0);
    add_mna_opt_complex(sys, inst.neg1_idx, br1, -1.0, 0.0);
    add_mna_opt_complex(sys, inst.pos2_idx, br2, 1.0, 0.0);
    add_mna_opt_complex(sys, inst.neg2_idx, br2, -1.0, 0.0);

    // Reformulate the travelling-wave equations into sum/difference form so
    // the structure stays well-conditioned at ω → 0 (where it must reduce
    // to the DC wire constraint V1 = V2 / I1 = -I2 without a singular 2×2
    // sub-block on the current columns). Let `G = exp(-jωTD) = c - j s`.
    //
    //   Sum:  ((1-G)/Z0) * (V1 + V2) - (1+G) * (I1 + I2) = 0
    //   Diff: ((1+G)/Z0) * (V1 - V2) - (1-G) * (I1 - I2) = 0
    //
    // At ω = 0, G = 1 → row1 collapses to `I1 + I2 = 0` and row2 to
    // `V1 = V2`, exactly matching `stamp_tline_dc`'s wire form.
    let c = (omega * td).cos();
    let s = (omega * td).sin();
    // a = (1 - G) = (1 - c) + j s
    let a_re = 1.0 - c;
    let a_im = s;
    // b = (1 + G) = (1 + c) - j s
    let b_re = 1.0 + c;
    let b_im = -s;

    // Row br1 (sum equation): a*admit * (V1 + V2) - b * (I1 + I2) = 0
    let sa_re = a_re * admit;
    let sa_im = a_im * admit;
    add_complex(sys, br1, inst.pos1_idx, sa_re, sa_im);
    add_complex(sys, br1, inst.neg1_idx, -sa_re, -sa_im);
    add_complex(sys, br1, inst.pos2_idx, sa_re, sa_im);
    add_complex(sys, br1, inst.neg2_idx, -sa_re, -sa_im);
    sys.real.add(br1, br1, -b_re);
    sys.imag.add(br1, br1, -b_im);
    sys.real.add(br1, br2, -b_re);
    sys.imag.add(br1, br2, -b_im);

    // Row br2 (difference equation): b*admit * (V1 - V2) - a * (I1 - I2) = 0
    let sb_re = b_re * admit;
    let sb_im = b_im * admit;
    add_complex(sys, br2, inst.pos1_idx, sb_re, sb_im);
    add_complex(sys, br2, inst.neg1_idx, -sb_re, -sb_im);
    add_complex(sys, br2, inst.pos2_idx, -sb_re, -sb_im);
    add_complex(sys, br2, inst.neg2_idx, sb_re, sb_im);
    sys.real.add(br2, br1, -a_re);
    sys.imag.add(br2, br1, -a_im);
    sys.real.add(br2, br2, a_re);
    sys.imag.add(br2, br2, a_im);
}

fn add_complex(
    sys: &mut crate::sparse::ComplexLinearSystem,
    row: usize,
    col: Option<usize>,
    re: f64,
    im: f64,
) {
    if let Some(c) = col {
        if re != 0.0 {
            sys.real.add(row, c, re);
        }
        if im != 0.0 {
            sys.imag.add(row, c, im);
        }
    }
}

fn add_mna_opt_complex(
    sys: &mut crate::sparse::ComplexLinearSystem,
    row: Option<usize>,
    col: usize,
    re: f64,
    im: f64,
) {
    if let Some(r) = row {
        if re != 0.0 {
            sys.real.add(r, col, re);
        }
        if im != 0.0 {
            sys.imag.add(r, col, im);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn td_from_f_and_nl() {
        let m = TlineModel::new(50.0, None, Some(1e9), Some(0.5));
        assert!((m.td - 0.5e-9).abs() < 1e-18);
    }

    #[test]
    fn td_explicit_wins_over_f_nl() {
        let m = TlineModel::new(50.0, Some(2e-9), Some(1e9), Some(0.5));
        assert!((m.td - 2e-9).abs() < 1e-18);
    }

    #[test]
    fn td_defaults_when_neither_given() {
        let m = TlineModel::new(50.0, None, None, None);
        // f defaults to 1 GHz, nl defaults to 0.25 → td = 0.25 ns.
        assert!((m.td - 0.25e-9).abs() < 1e-18);
    }

    #[test]
    fn history_interpolation_linear() {
        let mut s = TlineState::new();
        s.init_from_dc(0.0, 0.0, 0.0, 0.0);
        s.accept(1.0, 1.0, 0.1, 2.0, 0.2, 1.0);
        s.accept(2.0, 2.0, 0.2, 4.0, 0.4, 1.0);
        let (v1, i1, v2, i2) = s.interpolate(1.5);
        assert!((v1 - 1.5).abs() < 1e-12);
        assert!((i1 - 0.15).abs() < 1e-12);
        assert!((v2 - 3.0).abs() < 1e-12);
        assert!((i2 - 0.3).abs() < 1e-12);
    }

    #[test]
    fn history_returns_initials_before_first_sample() {
        let mut s = TlineState::new();
        s.init_from_dc(1.0, 0.5, 2.0, -0.25);
        s.accept(5.0, 0.0, 0.0, 0.0, 0.0, 1.0);
        let (v1, i1, v2, i2) = s.interpolate(-1.0);
        assert!((v1 - 1.0).abs() < 1e-12);
        assert!((i1 - 0.5).abs() < 1e-12);
        assert!((v2 - 2.0).abs() < 1e-12);
        assert!((i2 + 0.25).abs() < 1e-12);
    }
}
