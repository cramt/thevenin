use crate::sparse::SparseLuCache;
use crate::{LinearSystem, SparseMatrixError};
use thiserror::Error;

/// NR iteration mode, matching ngspice's MODEINITJCT/MODEINITFLOAT.
///
/// On the very first NR iteration of each attempt, devices should initialize
/// junction voltages to built-in potentials (e.g. vcrit for PN junctions,
/// Vto for FETs) rather than limiting against previous iteration values.
/// This gives a physically reasonable starting point for multi-transistor
/// circuits that would otherwise converge to singular matrices from an
/// all-zeros start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NrMode {
    /// First iteration: initialize junction voltages to built-in potentials.
    /// No voltage limiting is applied (no meaningful "previous" to limit against).
    InitJct,
    /// Subsequent iterations: apply voltage limiting (pnjlim, fetlim) against
    /// the previous iteration's solution.
    Float,
}

#[derive(Error, Debug)]
pub enum NrError {
    #[error("Newton-Raphson failed to converge after {iterations} iterations")]
    NoConvergence { iterations: usize },
    #[error("failed to solve linear system: {0}")]
    SolveError(#[from] SparseMatrixError),
}

/// Newton-Raphson convergence options matching ngspice defaults.
///
/// See ngspice `src/spicelib/devices/cktinit.c` for default values.
#[derive(Debug, Clone)]
pub struct NrOptions {
    /// Absolute current tolerance (ngspice ABSTOL, default 1e-12).
    pub abstol: f64,
    /// Relative tolerance (ngspice RELTOL, default 1e-3).
    pub reltol: f64,
    /// Absolute voltage tolerance (ngspice VNTOL, default 1e-6).
    pub vntol: f64,
    /// Maximum iterations for DC operating point (ngspice ITL1, default 100).
    pub itl1: usize,
    /// Maximum iterations per step during Gmin/source stepping (ngspice ITL2, default 50).
    /// We use 200 (vs ngspice's 50) because slow-converging circuits (e.g. CMOS drivers
    /// with LAMBDA=0 MOSFETs feeding LTRA floating nodes) need more iterations to damp NR
    /// oscillations at each Gmin step.
    pub itl2: usize,
    /// Maximum iterations per transient timestep (ngspice ITL4, default 10).
    pub itl4: usize,
    /// Total iteration limit across the entire transient run (ngspice ITL5,
    /// default 0 = unlimited). When non-zero, the transient driver aborts
    /// once the cumulative NR iteration count exceeds this threshold. ngspice
    /// historically defaulted this to 5000 but modern ngspice treats it as a
    /// no-op; we keep the sentinel-zero convention so users get unlimited
    /// total iterations by default.
    pub itl5: usize,
    /// Maximum number of source steps for source stepping (ngspice ITL6 / SRCSTEPS,
    /// default 0). When 0 (default), the solver uses its built-in adaptive source
    /// stepping schedule (current behaviour). When non-zero, it caps the maximum
    /// number of outer-loop source steps taken before giving up.
    pub itl6: usize,
    /// Minimum conductance from each node to ground (ngspice GMIN, default 1e-12).
    /// Used by device models in junction conductance computations.
    pub gmin: f64,
    /// Diagonal Gmin added from every node to ground by the solver
    /// (ngspice `CKTdiagGmin`).  In ngspice this starts at 0 and is only
    /// elevated during Gmin stepping.  We default to `gmin` for backward
    /// compatibility; the DC sweep code sets it to 0 so that the only Gmin
    /// on device nodes comes from the device model equations (matching
    /// ngspice behaviour where `CKTdiagGmin` stays 0 when the initial
    /// NIiter converges).
    pub diag_gmin: f64,
    /// Charge tolerance for capacitor / inductor LTE estimation
    /// (ngspice CHGTOL, default 1e-14). Sets the floor on the charge
    /// or flux scale used when computing local truncation error during
    /// transient timestep control.
    pub chgtol: f64,
    /// Node-to-ground shunt resistance (ngspice RSHUNT, default 0 = disabled).
    /// When non-zero, the NR Jacobian assembly adds `1/rshunt` to every
    /// non-ground diagonal entry. This is ngspice's safety net for
    /// ill-conditioned matrices caused by floating nodes or dangling
    /// subcircuits — see `cktrshun.c` in ngspice. A 0 value preserves the
    /// current behaviour byte-for-byte; a finite value (e.g. 1MΩ) regularizes
    /// the matrix without materially affecting the solution.
    pub rshunt: f64,
    /// Maximum number of Gmin-stepping iterations (ngspice GMINSTEPS,
    /// default 10). Sentinel value `0` ⇒ skip Gmin stepping entirely and
    /// fall straight through to the next convergence fallback. Our Gmin
    /// stepping is adaptive (no fixed step count) so a non-zero value just
    /// enables the fallback — it does not cap the iteration count directly.
    pub gminsteps: u32,
    /// Skip the initial direct Newton-Raphson attempt (ngspice NOOPITER,
    /// default false). When true, the OP solver jumps straight to Gmin
    /// stepping without first trying a direct solve. Useful when the user
    /// knows the direct attempt will diverge on a difficult circuit.
    pub noopiter: bool,
    /// Transient truncation error multiplier (ngspice TRTOL, default 7.0).
    /// Scales the LTE tolerance budget used by transient timestep control —
    /// larger values let the timestep grow more aggressively, smaller values
    /// force tighter (more conservative) stepping. Surfaced as a `.options`
    /// knob; the LTE budget is `trtol * max(vol_tol, chg_tol)`.
    pub trtol: f64,
    /// Apply one pass of iterative refinement after each sparse LU solve
    /// (ngspice convention; not directly named in ngspice's `.options` set).
    /// Computes `r = b - A*x` and adds the correction `dx = A^{-1} r` to `x`.
    /// Cheap insurance against ill-conditioning. Default `false` to preserve
    /// existing behaviour byte-for-byte.
    pub iterative_refinement: bool,
    /// Absolute minimum pivot magnitude during sparse LU
    /// (ngspice PIVTOL, default 1e-13). Pivots whose magnitude falls below
    /// this floor would be considered unusable, forcing an off-diagonal
    /// pivot search.  faer's high-level sparse LU does not currently expose
    /// pivot thresholding, so this value is parsed and stored but treated
    /// as a no-op; a stderr warning fires when the user sets it.
    pub pivtol: f64,
    /// Relative-to-row pivot ratio during sparse LU
    /// (ngspice PIVREL, default 1e-3). Selects an off-diagonal pivot when
    /// `|diag| < pivrel * |max_in_row|`.  Same caveat as `pivtol`: parsed
    /// and stored but no-op pending faer pivot-knob support.
    pub pivrel: f64,
    /// Skip the OP re-solve after `.alter` (ngspice NOOPALTER, default
    /// false). When true, the post-`alter` analysis re-uses the previous
    /// operating point as the initial guess rather than re-running CKTop
    /// from scratch. Currently parsed and stored only — `execute_alter`
    /// mutates the IR but does not itself trigger a solve, so there is
    /// no live re-solve path to short-circuit yet. Wire the option into
    /// the alter-and-re-solve pathway once that pathway exists.
    pub noopalter: bool,
    /// Run Gmin stepping before the direct NR attempt (ngspice
    /// GMINPRIORITY, default false). When true, the OP solver tries
    /// Gmin stepping first and falls back to the direct NR solve only
    /// if Gmin stepping fails. The mirror of the default order (direct
    /// → Gmin → source) — useful for hard circuits where the direct
    /// solve diverges but you still want it as a fast-path fallback.
    pub gminpriority: bool,
    /// Default MOSFET drain area (ngspice DEFAD, default 0). Applied to
    /// MOSFET instances that omit `AD`. Matches ngspice
    /// `CKTdefaultMosAD`.
    pub defad: f64,
    /// Default MOSFET source area (ngspice DEFAS, default 0). Applied
    /// to MOSFET instances that omit `AS`. Matches ngspice
    /// `CKTdefaultMosAS`.
    pub defas: f64,
    /// Default MOSFET channel length (ngspice DEFL, default 1e-4).
    /// Applied to MOSFET instances that omit `L`. Matches ngspice
    /// `CKTdefaultMosL`.
    pub defl: f64,
    /// Default MOSFET channel width (ngspice DEFW, default 1e-4).
    /// Applied to MOSFET instances that omit `W`. Matches ngspice
    /// `CKTdefaultMosW`.
    pub defw: f64,
}

impl Default for NrOptions {
    fn default() -> Self {
        Self {
            abstol: 1e-12,
            reltol: 1e-3,
            vntol: 1e-6,
            itl1: 100,
            itl2: 200,
            itl4: 10,
            // ITL5 sentinel: 0 means "no total-iteration cap", matching modern
            // ngspice behaviour. The Thevenin transient driver only enforces
            // the cap when itl5 > 0.
            itl5: 0,
            // ITL6 sentinel: 0 means "use the built-in adaptive source-stepping
            // schedule". A positive value caps the number of outer source-step
            // iterations.
            itl6: 0,
            gmin: 1e-12,
            diag_gmin: 1e-12,
            chgtol: 1e-14,
            // RSHUNT sentinel: 0 = disabled (no shunt added). A finite
            // value adds `1/rshunt` to every non-ground diagonal entry of
            // the NR Jacobian, matching ngspice CKTrshunt.
            rshunt: 0.0,
            // GMINSTEPS: default 10 matches ngspice CKTnumGminSteps. The
            // sentinel 0 disables Gmin stepping (the OP solver skips that
            // fallback). Our stepping is adaptive, so a positive value
            // simply enables the existing behaviour.
            gminsteps: 10,
            // NOOPITER: default false preserves the current "try direct NR
            // first" behaviour. When true, the direct attempt is skipped.
            noopiter: false,
            // TRTOL: ngspice default 7.0 — the LTE budget multiplier used in
            // transient timestep control.
            trtol: 7.0,
            // Iterative refinement: off by default to preserve byte-for-byte
            // behaviour; one refinement pass is performed per solve when on.
            iterative_refinement: false,
            // PIVTOL / PIVREL: ngspice defaults (1e-13 / 1e-3). Currently
            // accepted from `.options` but applied as a no-op because faer's
            // high-level sparse LU doesn't expose pivot thresholds.
            pivtol: 1e-13,
            pivrel: 1e-3,
            // NOOPALTER: default false preserves the historical
            // "always re-solve OP after `.alter`" semantics. Parsed
            // but currently has no live re-solve path to short-circuit
            // (see field docstring).
            noopalter: false,
            // GMINPRIORITY: default false preserves the current
            // "direct NR first, Gmin stepping as fallback" ordering.
            // When true, Gmin stepping runs first and direct NR
            // becomes the fallback.
            gminpriority: false,
            // MOSFET geometry defaults: ngspice cktinit.c sets DEFL =
            // DEFW = 1e-4 and DEFAD = DEFAS = 0. Mirror those so the
            // option being absent is identical to ngspice's defaults.
            defad: 0.0,
            defas: 0.0,
            defl: 1e-4,
            defw: 1e-4,
        }
    }
}

/// Result of Newton-Raphson iteration.
#[derive(Debug)]
pub struct NrResult {
    /// Final solution vector.
    pub solution: Vec<f64>,
    /// Number of iterations performed (total across all attempts).
    pub iterations: usize,
}

/// Check convergence of NR iteration using ngspice-style criteria.
///
/// For node voltages (indices 0..num_nodes):
///   |v_new - v_old| <= reltol * max(|v_new|, |v_old|) + vntol
///
/// For branch currents (indices num_nodes..):
///   |i_new - i_old| <= reltol * max(|i_new|, |i_old|) + abstol
fn check_convergence(old: &[f64], new: &[f64], num_nodes: usize, options: &NrOptions) -> bool {
    for i in 0..old.len() {
        let diff = (new[i] - old[i]).abs();
        let tol = if i < num_nodes {
            options.reltol * new[i].abs().max(old[i].abs()) + options.vntol
        } else {
            options.reltol * new[i].abs().max(old[i].abs()) + options.abstol
        };
        if diff > tol {
            return false;
        }
    }
    true
}

/// Parameters for a single NR attempt.
///
/// ngspice has two separate Gmin mechanisms:
/// - `CKTdiagGmin` (diagonal shunt: conductance from every node to ground)
/// - `CKTgmin` (device-model gmin: minimum junction conductance seen by device models)
///
/// These are stepped independently in different convergence algorithms:
/// - `dynamic_gmin` steps only `CKTdiagGmin` (diagonal shunt)
/// - `new_gmin` steps only `CKTgmin` (device-model conductance)
struct NrAttempt {
    /// Gmin added to the matrix diagonal (every node to ground).
    /// Corresponds to ngspice `CKTdiagGmin`.
    diag_gmin: f64,
    /// Gmin passed to the load closure for device-model stamps.
    /// Corresponds to ngspice `CKTgmin` during `new_gmin` stepping.
    dev_gmin: f64,
    source_factor: f64,
    max_iters: usize,
}

/// Run NR iteration with given attempt parameters.
///
/// `first_mode` controls what mode is used for iter 0:
///   - `NrMode::InitJct`: devices initialize to built-in potentials (used for
///     the very first NR attempt of a fresh DC OP from scratch).
///   - `NrMode::Float`: devices use voltage limiting from the start (used for
///     gmin/source stepping, transient, DC sweep continuation, etc.).
///
/// Returns `Ok(NrResult)` if converged, `Err` otherwise.
fn try_nr<F>(
    options: &NrOptions,
    dim: usize,
    num_nodes: usize,
    load_system: &F,
    initial_guess: &[f64],
    attempt: &NrAttempt,
    first_mode: NrMode,
) -> Result<NrResult, NrError>
where
    F: Fn(&[f64], &mut LinearSystem, f64, f64, NrMode),
{
    try_nr_with_cache(
        options,
        dim,
        num_nodes,
        load_system,
        initial_guess,
        attempt,
        first_mode,
        None,
    )
}

/// Same as [`try_nr`], but threads an externally-owned [`SparseLuCache`]
/// through every NR iteration so the symbolic LU survives across the
/// caller's outer loop (typically the transient timestep loop or a DC
/// sweep). When `cache` is `None` a fresh cache is created and discarded
/// per call, matching the existing one-shot behaviour.
#[expect(clippy::too_many_arguments)]
fn try_nr_with_cache<F>(
    options: &NrOptions,
    dim: usize,
    num_nodes: usize,
    load_system: &F,
    initial_guess: &[f64],
    attempt: &NrAttempt,
    first_mode: NrMode,
    cache: Option<&mut SparseLuCache>,
) -> Result<NrResult, NrError>
where
    F: Fn(&[f64], &mut LinearSystem, f64, f64, NrMode),
{
    let mut solution = initial_guess.to_vec();
    let mut system = LinearSystem::new(dim);
    // NR iterations of a fixed topology share a sparsity pattern, so the
    // sparse symbolic LU computed on the first iteration can be reused
    // on every subsequent iteration via `solve_with_cache`. When the
    // caller provides a cache it survives across NR calls too (e.g. all
    // timesteps of a transient share the same circuit topology, so the
    // symbolic LU survives the entire simulation).
    let mut local_cache = SparseLuCache::new();
    let lu_cache = cache.unwrap_or(&mut local_cache);
    let mut total_iters = 0;
    for iter in 0..attempt.max_iters {
        system.matrix.clear();
        system.rhs.fill(0.0);
        let mode = if iter == 0 { first_mode } else { NrMode::Float };
        let stamp_t0 = std::time::Instant::now();
        load_system(
            &solution,
            &mut system,
            attempt.source_factor,
            attempt.dev_gmin,
            mode,
        );

        // Add diagonal Gmin from each node to ground for numerical stability.
        for i in 0..num_nodes {
            system.matrix.add(i, i, attempt.diag_gmin);
        }
        // RSHUNT: when enabled (> 0), add `1/rshunt` to every non-ground
        // diagonal entry. ngspice's safety net for ill-conditioned matrices
        // caused by floating nodes or dangling subcircuits. With the default
        // 0 sentinel this loop is a no-op, preserving current behaviour
        // byte-for-byte. See ngspice `cktrshun.c`.
        if options.rshunt > 0.0 {
            let gshunt = 1.0 / options.rshunt;
            for i in 0..num_nodes {
                system.matrix.add(i, i, gshunt);
            }
        }
        crate::sparse::record_stamp_nanos(stamp_t0.elapsed().as_nanos() as u64);

        let new_solution =
            match system.solve_with_cache_refined(lu_cache, options.iterative_refinement) {
                Ok(s) => s,
                Err(e) => {
                    return Err(NrError::SolveError(e));
                }
            };

        if new_solution.iter().any(|v| v.is_nan() || v.is_infinite()) {
            return Err(NrError::NoConvergence {
                iterations: iter + 1,
            });
        }
        total_iters = iter + 1;

        if iter > 0 && check_convergence(&solution, &new_solution, num_nodes, options) {
            return Ok(NrResult {
                solution: new_solution,
                iterations: total_iters,
            });
        }

        solution = new_solution;
    }

    Err(NrError::NoConvergence {
        iterations: total_iters,
    })
}

/// Dynamic Gmin stepping fallback (ngspice `dynamic_gmin`).
///
/// Steps the **diagonal shunt** (CKTdiagGmin) while keeping device-model gmin
/// at the elevated level too (combined approach).  Uses adaptive factor sizing
/// and backtracking on failure, matching ngspice's `dynamic_gmin()` algorithm
/// from `cktop.c`.
fn gmin_stepping<F>(
    options: &NrOptions,
    dim: usize,
    num_nodes: usize,
    load_system: &F,
    _initial_guess: &[f64],
) -> Result<NrResult, NrError>
where
    F: Fn(&[f64], &mut LinearSystem, f64, f64, NrMode),
{
    let gmin_factor_max = 10.0_f64;
    let mut factor = gmin_factor_max;
    let mut gmin = 1e-2_f64;
    let gmin_target = options.gmin.max(0.0);
    // Zero all node voltages before gmin stepping, matching ngspice
    // dynamic_gmin() cktop.c lines 182-186.  This gives a clean starting
    // point independent of the (possibly diverged) direct NR attempt.
    let mut solution = vec![0.0; dim];
    let mut last_good_solution = solution.clone();
    let mut last_good_gmin = gmin;
    let mut total_iters = 0;
    // Track whether this is the first gmin step so we can use InitJct mode,
    // matching ngspice which sets firstmode = MODEINITJCT at line 172.
    let mut first_step = true;

    while gmin >= gmin_target * 0.9 {
        let attempt = NrAttempt {
            diag_gmin: gmin,
            // Device-model gmin stays at the base level (options.gmin = 1e-12),
            // NOT elevated.  This matches ngspice's dynamic_gmin (cktop.c) which
            // only elevates CKTdiagGmin while CKTgmin remains at its default.
            // The load closure computes max(dev_gmin, options.gmin) = options.gmin.
            dev_gmin: gmin_target,
            source_factor: 1.0,
            max_iters: options.itl2,
        };
        // Use InitJct for the first step (matching ngspice), Float for
        // subsequent steps (matching ngspice continuemode transition at
        // cktop.c line 203).
        let mode = if first_step {
            NrMode::InitJct
        } else {
            NrMode::Float
        };
        match try_nr(
            options,
            dim,
            num_nodes,
            load_system,
            &solution,
            &attempt,
            mode,
        ) {
            Ok(result) => {
                total_iters += result.iterations;
                first_step = false;

                // Adapt factor based on convergence speed
                // (matches ngspice cktop.c dynamic_gmin lines 216-223)
                let quarter = options.itl2 / 4;
                if result.iterations <= quarter {
                    // Easy convergence — try bigger steps
                    factor = (factor * factor.sqrt()).min(gmin_factor_max);
                } else if result.iterations > 3 * quarter {
                    // Slow convergence — use smaller steps
                    factor = factor.sqrt().max(1.00005);
                }

                last_good_solution.clone_from(&result.solution);
                last_good_gmin = gmin;
                solution = result.solution;
                gmin /= factor;
            }
            Err(_) => {
                // Backtracking: restore last good state and try smaller step
                // (matches ngspice cktop.c dynamic_gmin lines 233-248)
                if factor < 1.00005 {
                    // Can't step any smaller — give up on gmin stepping
                    return Err(NrError::NoConvergence {
                        iterations: total_iters,
                    });
                }
                factor = factor.sqrt().sqrt();
                gmin = last_good_gmin / factor;
                solution.clone_from(&last_good_solution);
            }
        }
    }

    // Final solve with target Gmin.
    let attempt = NrAttempt {
        diag_gmin: gmin_target,
        dev_gmin: gmin_target,
        source_factor: 1.0,
        max_iters: options.itl2,
    };
    let result = try_nr(
        options,
        dim,
        num_nodes,
        load_system,
        &solution,
        &attempt,
        NrMode::Float,
    )?;
    total_iters += result.iterations;

    Ok(NrResult {
        solution: result.solution,
        iterations: total_iters,
    })
}

/// New Gmin stepping fallback (ngspice `new_gmin`).
///
/// Unlike `gmin_stepping` (which elevates the diagonal shunt), this steps only
/// the **device-model gmin** (`CKTgmin`) while keeping diagonal gmin at the
/// target value.  Device models use the elevated gmin for junction conductances,
/// providing regularization through the device's own matrix stamps rather than
/// through an external node-to-ground shunt.
///
/// This is more physically meaningful for floating-body SOI devices and multi-
/// transistor circuits where the body/internal nodes couple to the circuit only
/// through device junctions, not through diagonal shunts.
fn new_gmin_stepping<F>(
    options: &NrOptions,
    dim: usize,
    num_nodes: usize,
    load_system: &F,
    _initial_guess: &[f64],
) -> Result<NrResult, NrError>
where
    F: Fn(&[f64], &mut LinearSystem, f64, f64, NrMode),
{
    let gmin_factor_max = 10.0_f64;
    let mut factor = gmin_factor_max;
    let mut dev_gmin = 1e-2_f64;
    let gmin_target = options.gmin.max(0.0);
    // Diagonal gmin stays at the target throughout — only device gmin is stepped.
    let diag = options.diag_gmin;
    // Reset solution to zero before new_gmin stepping, matching ngspice
    // new_gmin() which zeroes CKTrhsOld and CKTstate0 (cktop.c lines 370-374).
    // Starting from the JCT initial guess can trap NR in a wrong basin for
    // bistable circuits.
    let mut solution = vec![0.0; dim];
    let mut last_good_solution = solution.clone();
    let mut last_good_dev_gmin = dev_gmin;
    let mut total_iters = 0;
    let mut first_step = true;

    while dev_gmin >= gmin_target * 0.9 {
        let attempt = NrAttempt {
            diag_gmin: diag,
            dev_gmin,
            source_factor: 1.0,
            max_iters: options.itl2,
        };
        // Use InitJct for the first step (matching ngspice new_gmin which
        // resets CKTmode to firstmode=MODEINITJCT at line 360).
        let mode = if first_step {
            first_step = false;
            NrMode::InitJct
        } else {
            NrMode::Float
        };
        match try_nr(
            options,
            dim,
            num_nodes,
            load_system,
            &solution,
            &attempt,
            mode,
        ) {
            Ok(result) => {
                total_iters += result.iterations;
                first_step = false;

                let quarter = options.itl2 / 4;
                if result.iterations <= quarter {
                    factor = (factor * factor.sqrt()).min(gmin_factor_max);
                } else if result.iterations > 3 * quarter {
                    factor = factor.sqrt().max(1.00005);
                }

                last_good_solution.clone_from(&result.solution);
                last_good_dev_gmin = dev_gmin;
                solution = result.solution;
                dev_gmin /= factor;
            }
            Err(_) => {
                if factor < 1.00005 {
                    return Err(NrError::NoConvergence {
                        iterations: total_iters,
                    });
                }
                factor = factor.sqrt().sqrt();
                dev_gmin = last_good_dev_gmin / factor;
                solution.clone_from(&last_good_solution);
            }
        }
    }

    // Final solve with target device gmin.
    let attempt = NrAttempt {
        diag_gmin: diag,
        dev_gmin: gmin_target,
        source_factor: 1.0,
        max_iters: options.itl2,
    };
    let result = try_nr(
        options,
        dim,
        num_nodes,
        load_system,
        &solution,
        &attempt,
        NrMode::Float,
    )?;
    total_iters += result.iterations;

    Ok(NrResult {
        solution: result.solution,
        iterations: total_iters,
    })
}

/// Gillespie source stepping (ngspice `gillespie_src`, cktop.c lines 481-658).
///
/// Ramps all independent sources from 0 → 100% at the **target** gmin with
/// adaptive step sizing and backtracking on failure.  No gmin reduction phase
/// — the circuit is solved at its nominal gmin throughout the ramp, avoiding
/// the gmin bifurcation that plagues floating-body SOI devices.
///
/// If the initial solve at sources=0 fails, a preliminary gmin stepping is
/// done at sources=0 first (ngspice lines 510-548).
fn source_stepping<F>(
    options: &NrOptions,
    dim: usize,
    num_nodes: usize,
    load_system: &F,
    _initial_guess: &[f64],
) -> Result<NrResult, NrError>
where
    F: Fn(&[f64], &mut LinearSystem, f64, f64, NrMode),
{
    let target_gmin = options.gmin;
    // Zero the solution before source stepping (ngspice lines 497-501).
    let mut solution = vec![0.0; dim];
    let mut total_iters = 0;

    // Step 1: solve at sources = 0 with target gmin.
    let zero_attempt = NrAttempt {
        diag_gmin: target_gmin,
        dev_gmin: target_gmin,
        source_factor: 0.0,
        max_iters: options.itl2,
    };
    match try_nr(
        options,
        dim,
        num_nodes,
        load_system,
        &solution,
        &zero_attempt,
        NrMode::InitJct,
    ) {
        Ok(result) => {
            total_iters += result.iterations;
            solution = result.solution;
        }
        Err(_) => {
            // Sources=0 failed at target gmin — do gmin stepping at sources=0
            // first (ngspice lines 510-548: ramp CKTdiagGmin from elevated to
            // gshunt at sources=0).
            let gmin_start = target_gmin.max(1e-12) * 1e10;
            let mut gmin = gmin_start;
            for _ in 0..=10 {
                let attempt = NrAttempt {
                    diag_gmin: gmin,
                    // Only elevate diagonal; device models keep base gmin.
                    dev_gmin: target_gmin,
                    source_factor: 0.0,
                    max_iters: options.itl2,
                };
                match try_nr(
                    options,
                    dim,
                    num_nodes,
                    load_system,
                    &solution,
                    &attempt,
                    NrMode::Float,
                ) {
                    Ok(result) => {
                        total_iters += result.iterations;
                        solution = result.solution;
                        gmin /= 10.0;
                    }
                    Err(_) => break,
                }
            }
        }
    }

    // Step 2: ramp sources from 0 → 100% at target gmin with adaptive step
    // size and backtracking (ngspice lines 553-641).
    let mut conv_fact = 0.0_f64;
    let mut raise = 0.001_f64;
    let mut src_fact = conv_fact + raise;
    let mut backup = solution.clone();

    // ITL6 / SRCSTEPS cap: when set, limit the number of outer source-step
    // iterations. 0 (default) means "use the adaptive schedule without
    // imposing an extra cap" — current behaviour.
    let mut src_step_count: usize = 0;
    while raise >= 1e-7 && conv_fact < 1.0 {
        if options.itl6 > 0 && src_step_count >= options.itl6 {
            break;
        }
        src_step_count += 1;
        if src_fact > 1.0 {
            src_fact = 1.0;
        }

        let attempt = NrAttempt {
            // diag_gmin = 0 during source ramp, matching ngspice gillespie_src
            // where CKTdiagGmin = CKTgshunt = 0 after the initial gmin stepping.
            // Device models see options.gmin through the load closure.
            diag_gmin: 0.0,
            dev_gmin: target_gmin,
            source_factor: src_fact,
            max_iters: options.itl2,
        };
        match try_nr(
            options,
            dim,
            num_nodes,
            load_system,
            &solution,
            &attempt,
            NrMode::Float,
        ) {
            Ok(result) => {
                total_iters += result.iterations;
                conv_fact = src_fact;
                backup.copy_from_slice(&result.solution);
                solution = result.solution;

                // Adapt step size based on convergence speed.
                let quarter = options.itl2 / 4;
                if result.iterations <= quarter {
                    raise *= 1.5;
                }
                if result.iterations > 3 * quarter {
                    raise *= 0.5;
                }
                src_fact = conv_fact + raise;
            }
            Err(_) => {
                if src_fact - conv_fact < 1e-8 {
                    break;
                }
                raise /= 10.0;
                if raise > 0.01 {
                    raise = 0.01;
                }
                src_fact = conv_fact + raise;
                solution.copy_from_slice(&backup);
            }
        }
    }

    if conv_fact < 1.0 {
        return Err(NrError::NoConvergence {
            iterations: total_iters,
        });
    }

    Ok(NrResult {
        solution,
        iterations: total_iters,
    })
}

/// Solve a nonlinear system for a single transient timestep.
///
/// First attempts direct NR with `diag_gmin`. On failure (singular matrix),
/// falls back to Gmin stepping: starts with elevated diagonal Gmin (1e-2)
/// and progressively reduces to target, matching ngspice's approach for
/// transient steps where MOSFET cutoff can leave internal nodes floating.
/// Threads an externally-owned [`SparseLuCache`] through the NR loop so
/// the sparse symbolic LU survives across timesteps. The transient
/// driver constructs one cache and passes it on every timestep — this
/// keeps the cache hit rate near 100% (the matrix topology never changes
/// across timesteps of a transient).
///
/// Pass `cache: None` for one-shot NR calls that don't need symbolic
/// reuse; a fresh cache is created internally and discarded.
pub fn transient_nr_solve_with_cache<F>(
    options: &NrOptions,
    dim: usize,
    num_nodes: usize,
    load_system: F,
    initial_guess: &[f64],
    cache: Option<&mut SparseLuCache>,
) -> Result<NrResult, NrError>
where
    F: Fn(&[f64], &mut LinearSystem, f64, f64, NrMode),
{
    // Direct NR only — no gmin/source stepping fallbacks.
    //
    // Matches ngspice dctran.c: transient timesteps call NIiter() with
    // CKTtranMaxIter (ITL4, default 10).  On failure, the caller cuts the
    // timestep (delta/8) and retries.  This is the correct strategy because
    // the previous-timestep solution is a good initial guess that only needs
    // a smaller step to stay within the convergence basin.
    //
    // ngspice sets CKTdiagGmin = 0 after DC OP convergence, but its
    // Markowitz sparse solver tolerates near-singular pivots that dense LU
    // cannot.  We use options.gmin as a minimal diagonal shunt — it's the
    // circuit's requested floor conductance and is negligible compared to
    // real device conductances, but keeps the matrix numerically non-singular
    // for dense partial-pivoting LU.
    let attempt = NrAttempt {
        diag_gmin: options.gmin,
        dev_gmin: options.gmin,
        source_factor: 1.0,
        max_iters: options.itl4,
    };
    // Transient always uses Float — we have a meaningful previous solution.
    try_nr_with_cache(
        options,
        dim,
        num_nodes,
        &load_system,
        initial_guess,
        &attempt,
        NrMode::Float,
        cache,
    )
}

/// Solve a nonlinear system using source stepping directly, bypassing direct NR
/// and Gmin stepping.
///
/// Use this for circuits with transmission lines (LTRA/TXL) combined with
/// cascaded MOSFET stages.  Without voltage-step limiting, Gmin stepping can
/// converge to spurious negative-voltage fixed points in cascaded-inverter
/// circuits.  Source stepping avoids this by ramping all independent sources
/// from zero, ensuring NR always follows the physical solution trajectory.
pub fn source_stepping_solve<F>(
    options: &NrOptions,
    dim: usize,
    num_nodes: usize,
    load_system: F,
    initial_guess: &[f64],
) -> Result<NrResult, NrError>
where
    F: Fn(&[f64], &mut LinearSystem, f64, f64, NrMode),
{
    source_stepping(options, dim, num_nodes, &load_system, initial_guess)
}

/// Newton-Raphson solve with explicit first-iteration mode.
///
/// `first_mode` controls whether the first NR iteration uses `InitJct` (for
/// fresh DC OP) or `Float` (for DC sweep continuation where the initial guess
/// is already near the solution and MODEINITJCT would corrupt device voltage
/// limiting state).
pub fn newton_raphson_solve_with_mode<F>(
    options: &NrOptions,
    dim: usize,
    num_nodes: usize,
    load_system: F,
    initial_guess: &[f64],
    first_mode: NrMode,
) -> Result<NrResult, NrError>
where
    F: Fn(&[f64], &mut LinearSystem, f64, f64, NrMode),
{
    // Helper: run a single direct NR attempt with the default diagonal
    // and device Gmin. Returns the NR result on convergence; the caller
    // decides what to fall through to on failure.
    let try_direct = || {
        let attempt = NrAttempt {
            diag_gmin: options.diag_gmin,
            dev_gmin: options.diag_gmin,
            source_factor: 1.0,
            max_iters: options.itl1,
        };
        try_nr(
            options,
            dim,
            num_nodes,
            &load_system,
            initial_guess,
            &attempt,
            first_mode,
        )
    };

    // GMINPRIORITY (ngspice GMINPRIORITY): when set, try Gmin stepping
    // first and only fall back to the direct NR solve if it fails.
    // Mirrors the default ordering. GMINSTEPS=0 disables Gmin stepping
    // entirely, so GMINPRIORITY with GMINSTEPS=0 effectively falls
    // straight through to the direct/source paths.
    if options.gminpriority {
        if options.gminsteps > 0
            && let Ok(result) = gmin_stepping(options, dim, num_nodes, &load_system, initial_guess)
        {
            return Ok(result);
        }
        if options.gminsteps > 0
            && let Ok(result) =
                new_gmin_stepping(options, dim, num_nodes, &load_system, initial_guess)
        {
            return Ok(result);
        }
        // Fallback: direct NR, unless NOOPITER is set.
        if !options.noopiter
            && let Ok(result) = try_direct()
        {
            return Ok(result);
        }
        return source_stepping(options, dim, num_nodes, &load_system, initial_guess);
    }

    // Default ordering: try direct NR first — unless NOOPITER is set, in
    // which case skip straight to Gmin stepping. Matches ngspice's NOOPITER
    // option which suppresses the initial CKTop() direct solve.
    if !options.noopiter
        && let Ok(result) = try_direct()
    {
        return Ok(result);
    }

    // Fallback 1: Dynamic Gmin stepping (diagonal shunt + device gmin elevated).
    // GMINSTEPS=0 sentinel: skip both Gmin stepping fallbacks. Matches
    // ngspice's convention where setting GMINSTEPS=0 disables CKTop's gmin
    // fallback entirely.
    if options.gminsteps > 0
        && let Ok(result) = gmin_stepping(options, dim, num_nodes, &load_system, initial_guess)
    {
        return Ok(result);
    }

    // Fallback 2: New Gmin stepping (device-model gmin elevated, diagonal at base).
    // This provides regularization through device junction conductances rather
    // than through diagonal shunts, which is more effective for circuits with
    // floating internal nodes (SOI body, VBIC thermal) that couple to the
    // circuit only through device model stamps.
    if options.gminsteps > 0
        && let Ok(result) = new_gmin_stepping(options, dim, num_nodes, &load_system, initial_guess)
    {
        return Ok(result);
    }

    // Fallback 3: Source stepping.
    source_stepping(options, dim, num_nodes, &load_system, initial_guess)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    use approx::assert_abs_diff_eq;

    /// Thermal voltage at room temperature (300K).
    const VT: f64 = 0.02585;
    /// Diode saturation current.
    const IS: f64 = 1e-14;

    /// Diode current: I = IS * (exp(V/Vt) - 1)
    fn diode_current(v: f64) -> f64 {
        IS * (safe_exp(v / VT) - 1.0)
    }

    /// Diode conductance: dI/dV = IS/Vt * exp(V/Vt)
    fn diode_conductance(v: f64) -> f64 {
        IS / VT * safe_exp(v / VT)
    }

    /// Safe exponential that clamps the argument to avoid overflow.
    fn safe_exp(x: f64) -> f64 {
        if x > 500.0 {
            (500.0_f64).exp()
        } else {
            x.exp()
        }
    }

    /// Test NR on a simple diode circuit: V1=1V, R1=1k, D1.
    ///
    /// Circuit: V1(=1V) --- R1(=1k) --- anode(D1) --- cathode(ground)
    ///
    /// MNA system (2 unknowns: V(1)=node 1 voltage, I(V1)=branch current):
    /// Node 1: V1 is connected via branch equation.
    /// Node 2 (diode anode): KCL: (V2-V1)/R1 + I_diode(V2) = 0
    ///
    /// Actually, let's use a simpler setup:
    /// 3 unknowns: V(1), V(2), I(V1)
    /// Node 1: V1 branch current + G*(V1-V2) = 0 => I_branch + G*V1 - G*V2 = 0
    /// Node 2: G*(V2-V1) + I_diode(V2) = 0 => -G*V1 + G*V2 + I_diode(V2) = 0
    /// Branch: V1 - 0 = 1.0 => V1 = 1.0
    ///
    /// Where G = 1/R = 1/1000
    #[test]
    fn test_diode_circuit_convergence() {
        let g = 1.0 / 1000.0; // R = 1k
        let v_source = 1.0;
        let dim = 3; // V(1), V(2), I(V1)
        let num_nodes = 2;
        let options = NrOptions::default();

        let load = |solution: &[f64],
                    system: &mut LinearSystem,
                    source_factor: f64,
                    _gmin: f64,
                    _mode: NrMode| {
            let v2 = solution[1]; // diode voltage

            // Resistor R1 between nodes 0 and 1 (matrix indices)
            system.matrix.add(0, 0, g);
            system.matrix.add(0, 1, -g);
            system.matrix.add(1, 0, -g);
            system.matrix.add(1, 1, g);

            // Voltage source V1: node 0 to ground, branch index 2
            system.matrix.add(0, 2, 1.0);
            system.matrix.add(2, 0, 1.0);
            system.rhs[2] = v_source * source_factor;

            // Diode D1 between node 1 (anode) and ground (cathode)
            // Companion model: I_eq + g_d * V2
            // where g_d = dI/dV at V2, I_eq = I(V2) - g_d * V2
            let g_d = diode_conductance(v2);
            let i_d = diode_current(v2);
            let i_eq = i_d - g_d * v2;

            // Stamp conductance from node 1 to ground
            system.matrix.add(1, 1, g_d);
            // Stamp equivalent current source at node 1
            system.rhs[1] -= i_eq;
        };

        let initial = vec![0.0; dim];
        let result = newton_raphson_solve_with_mode(
            &options,
            dim,
            num_nodes,
            load,
            &initial,
            NrMode::InitJct,
        )
        .unwrap();

        assert!(
            result.iterations < 20,
            "expected < 20 iterations, got {}",
            result.iterations
        );

        let v1 = result.solution[0];
        let v_diode = result.solution[1];

        // V1 should be the source voltage
        assert_abs_diff_eq!(v1, v_source, epsilon = 1e-9);

        // Diode forward voltage should be ~0.6-0.7V
        assert!(
            v_diode > 0.5 && v_diode < 0.8,
            "diode voltage {v_diode} not in expected range 0.5-0.8V"
        );

        // Verify KCL at node 2: current through R = current through diode
        let i_r = (v1 - v_diode) * g;
        let i_d = diode_current(v_diode);
        assert_abs_diff_eq!(i_r, i_d, epsilon = 1e-9);
    }

    /// Test that a purely linear system converges in 2 iterations
    /// (first iteration gets the answer, second confirms convergence).
    #[test]
    fn test_linear_system_converges_immediately() {
        let options = NrOptions::default();
        let dim = 2;
        let num_nodes = 1;

        // V1=5V, R1=1k to ground
        // V(1) = 5V, I(V1) = -5mA
        let load = |_solution: &[f64],
                    system: &mut LinearSystem,
                    source_factor: f64,
                    _gmin: f64,
                    _mode: NrMode| {
            let g = 1.0 / 1000.0;
            // Resistor from node 0 to ground
            system.matrix.add(0, 0, g);
            // Voltage source: node 0, branch 1
            system.matrix.add(0, 1, 1.0);
            system.matrix.add(1, 0, 1.0);
            system.rhs[1] = 5.0 * source_factor;
        };

        let initial = vec![0.0; dim];
        let result = newton_raphson_solve_with_mode(
            &options,
            dim,
            num_nodes,
            load,
            &initial,
            NrMode::InitJct,
        )
        .unwrap();

        assert_eq!(result.iterations, 2); // solve once, confirm on second
        assert_abs_diff_eq!(result.solution[0], 5.0, epsilon = 1e-9);
        assert_abs_diff_eq!(result.solution[1], -5e-3, epsilon = 1e-9);
    }

    /// Test convergence check function directly.
    #[test]
    fn test_convergence_check() {
        let options = NrOptions::default();

        // Identical solutions should converge.
        let a = vec![1.0, 2.0, 0.001];
        assert!(check_convergence(&a, &a, 2, &options));

        // Small voltage change within vntol.
        let b = vec![1.0, 2.0 + 1e-7, 0.001];
        assert!(check_convergence(&a, &b, 2, &options));

        // Large voltage change — should not converge.
        let c = vec![1.0, 2.1, 0.001];
        assert!(!check_convergence(&a, &c, 2, &options));

        // Small current change within abstol.
        let d = vec![1.0, 2.0, 0.001 + 1e-13];
        assert!(check_convergence(&a, &d, 2, &options));

        // Large current change — should not converge.
        let e = vec![1.0, 2.0, 0.002];
        assert!(!check_convergence(&a, &e, 2, &options));
    }

    /// ITL1 caps direct-NR iterations for the DC operating point. With a
    /// strongly nonlinear circuit and `itl1 = 1`, direct NR cannot converge
    /// in a single iteration, so `newton_raphson_solve_with_mode` should
    /// fall through to Gmin / source stepping (which still uses itl2-sized
    /// inner attempts). The point of this test is to lock in *that ITL1
    /// is actually consulted* — bumping it to a generous value lets a
    /// pathological initial guess reach the correct answer via direct NR
    /// alone.
    #[test]
    fn itl1_too_small_forces_fallback_then_succeeds_with_larger_cap() {
        // Same diode circuit as test_diode_circuit_convergence (V1=1V, R1=1k, D1).
        let g = 1.0 / 1000.0;
        let v_source = 1.0;
        let dim = 3;
        let num_nodes = 2;

        let load = |solution: &[f64],
                    system: &mut LinearSystem,
                    source_factor: f64,
                    _gmin: f64,
                    _mode: NrMode| {
            let v2 = solution[1];
            system.matrix.add(0, 0, g);
            system.matrix.add(0, 1, -g);
            system.matrix.add(1, 0, -g);
            system.matrix.add(1, 1, g);
            system.matrix.add(0, 2, 1.0);
            system.matrix.add(2, 0, 1.0);
            system.rhs[2] = v_source * source_factor;
            let g_d = diode_conductance(v2);
            let i_d = diode_current(v2);
            let i_eq = i_d - g_d * v2;
            system.matrix.add(1, 1, g_d);
            system.rhs[1] -= i_eq;
        };

        // With ITL1=1, direct NR can't converge — the diode needs several
        // NR iterations. The solver should still succeed (via gmin/source
        // fallback) and the returned solution should be physical.
        let tight = NrOptions {
            itl1: 1,
            ..NrOptions::default()
        };
        let initial = vec![0.0; dim];
        let result =
            newton_raphson_solve_with_mode(&tight, dim, num_nodes, load, &initial, NrMode::InitJct)
                .expect("fallback should succeed even with itl1=1");
        let v_diode = result.solution[1];
        assert!(
            (0.5..0.8).contains(&v_diode),
            "diode voltage {v_diode} not physical after gmin fallback"
        );

        // With ITL1=500, direct NR converges in a handful of iterations.
        let generous = NrOptions {
            itl1: 500,
            ..NrOptions::default()
        };
        let result = newton_raphson_solve_with_mode(
            &generous,
            dim,
            num_nodes,
            load,
            &initial,
            NrMode::InitJct,
        )
        .expect("direct NR with generous ITL1 converges");
        let v_diode = result.solution[1];
        assert!(
            (0.5..0.8).contains(&v_diode),
            "diode voltage {v_diode} not physical with generous ITL1"
        );
        assert!(
            result.iterations < 30,
            "direct NR should converge fast on this circuit, got {} iters",
            result.iterations
        );
    }

    /// Test that Gmin stepping can solve a circuit that direct NR cannot
    /// easily handle (e.g., diode with high source voltage).
    #[test]
    fn test_diode_high_voltage_converges() {
        let g = 1.0 / 100.0; // R = 100 ohms
        let v_source = 10.0; // Higher voltage
        let dim = 3;
        let num_nodes = 2;
        let options = NrOptions::default();

        let load = |solution: &[f64],
                    system: &mut LinearSystem,
                    source_factor: f64,
                    _gmin: f64,
                    _mode: NrMode| {
            let v2 = solution[1];

            // Resistor
            system.matrix.add(0, 0, g);
            system.matrix.add(0, 1, -g);
            system.matrix.add(1, 0, -g);
            system.matrix.add(1, 1, g);

            // Voltage source
            system.matrix.add(0, 2, 1.0);
            system.matrix.add(2, 0, 1.0);
            system.rhs[2] = v_source * source_factor;

            // Diode companion model
            let g_d = diode_conductance(v2);
            let i_d = diode_current(v2);
            let i_eq = i_d - g_d * v2;
            system.matrix.add(1, 1, g_d);
            system.rhs[1] -= i_eq;
        };

        let initial = vec![0.0; dim];
        let result = newton_raphson_solve_with_mode(
            &options,
            dim,
            num_nodes,
            load,
            &initial,
            NrMode::InitJct,
        )
        .unwrap();

        let v_diode = result.solution[1];
        // With 10V source and 100 ohm resistor, diode voltage should still be ~0.6-0.8V
        assert!(
            v_diode > 0.5 && v_diode < 0.9,
            "diode voltage {v_diode} not in expected range"
        );

        // Verify: I_R = I_D (within NR convergence tolerance)
        let i_r = (result.solution[0] - v_diode) * g;
        let i_d = diode_current(v_diode);
        assert_abs_diff_eq!(i_r, i_d, epsilon = 1e-4);
    }
}
