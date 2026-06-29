//! Transient waveform evaluation for independent voltage/current sources.
//!
//! Implements PULSE, SIN, EXP, PWL, SFFM, and AM waveforms matching ngspice
//! semantics (see `ngspice-upstream/src/spicelib/devices/vsrc/vsrcload.c`).

use std::f64::consts::PI;

use cirq_ir::Waveform;

/// Parameters needed for waveform default value computation.
#[derive(Debug, Clone, Copy)]
pub struct TranParams {
    /// The timestep from .tran (used as default for rise/fall times).
    pub tstep: f64,
    /// The final simulation time (used as default for SIN freq, SFFM freqs).
    pub tstop: f64,
}

/// Evaluate a waveform at a given time.
///
/// Returns the instantaneous value of the source at time `t`.
/// Uses `tran` parameters for default values when waveform parameters are omitted.
pub fn evaluate(wf: &Waveform, t: f64, tran: &TranParams) -> f64 {
    match wf {
        Waveform::Pulse {
            v1,
            v2,
            td,
            tr,
            tf,
            pw,
            per,
        } => eval_pulse(
            *v1,
            *v2,
            td.unwrap_or(0.0),
            tr.unwrap_or(tran.tstep).max(tran.tstep),
            tf.unwrap_or(tran.tstep).max(tran.tstep),
            pw.unwrap_or(tran.tstop).max(0.0),
            *per,
            t,
            tran.tstep,
        ),
        Waveform::Sin {
            v0,
            va,
            freq,
            td,
            theta,
            phi,
        } => eval_sin(
            *v0,
            *va,
            freq.unwrap_or(if tran.tstop > 0.0 {
                1.0 / tran.tstop
            } else {
                1.0
            }),
            td.unwrap_or(0.0),
            theta.unwrap_or(0.0),
            phi.unwrap_or(0.0),
            t,
        ),
        Waveform::Exp {
            v1,
            v2,
            td1,
            tau1,
            td2,
            tau2,
        } => eval_exp(
            *v1,
            *v2,
            td1.unwrap_or(tran.tstep),
            tau1.unwrap_or(tran.tstep).max(tran.tstep),
            td2.unwrap_or(td1.unwrap_or(tran.tstep) + tran.tstep),
            tau2.unwrap_or(tran.tstep).max(tran.tstep),
            t,
        ),
        Waveform::Pwl(points) => eval_pwl(points, t),
        Waveform::Sffm { v0, va, fc, fs, md } => {
            let fc_val = fc.unwrap_or(if tran.tstop > 0.0 {
                5.0 / tran.tstop
            } else {
                5.0
            });
            let fs_val = fs.unwrap_or(if tran.tstop > 0.0 {
                500.0 / tran.tstop
            } else {
                500.0
            });
            // Clamp modulation depth to [0, fc/fs].
            let max_md = if fs_val > 0.0 { fc_val / fs_val } else { 0.0 };
            let md_val = md.unwrap_or(0.0).clamp(0.0, max_md);
            eval_sffm(*v0, *va, fc_val, fs_val, md_val, t)
        }
        Waveform::Am { va, vo, fc, fs, td } => eval_am(*va, *vo, *fc, *fs, td.unwrap_or(0.0), t),
        // `cirq_ir::Waveform` is `#[non_exhaustive]`; an unknown future
        // variant contributes zero until it gets an explicit evaluator.
        _ => 0.0,
    }
}

// ---- Individual waveform evaluators ----

/// PULSE waveform.
///
/// Parameters: v1, v2, td, tr, tf, pw, per
/// Before td: v1
/// Rise: td to td+tr (linear from v1 to v2)
/// High: td+tr to td+tr+pw (v2)
/// Fall: td+tr+pw to td+tr+pw+tf (linear from v2 to v1)
/// After: v1 (repeats if periodic)
#[expect(clippy::too_many_arguments)]
fn eval_pulse(
    v1: f64,
    v2: f64,
    td: f64,
    tr: f64,
    tf: f64,
    pw: f64,
    per: Option<f64>,
    t: f64,
    tstep: f64,
) -> f64 {
    if t < td {
        return v1;
    }

    // Compute the period.
    let period = per.unwrap_or(tr + pw + tf).max(tr + pw + tf).max(tstep);

    // Fold time into the current period.
    let mut time = t - td;
    if period > 0.0 && time >= period {
        time -= period * (time / period).floor();
    }

    if time < tr {
        // Rising edge.
        v1 + (v2 - v1) * time / tr
    } else if time < tr + pw {
        // Pulse width (high).
        v2
    } else if time < tr + pw + tf {
        // Falling edge.
        v2 + (v1 - v2) * (time - tr - pw) / tf
    } else {
        // Rest of period (low).
        v1
    }
}

/// SIN waveform.
///
/// Before td: v0 + va * sin(phi)
/// After td: v0 + va * sin(2*pi*freq*(t-td) + phi) * exp(-theta*(t-td))
fn eval_sin(v0: f64, va: f64, freq: f64, td: f64, theta: f64, phi_deg: f64, t: f64) -> f64 {
    let phi_rad = phi_deg * PI / 180.0;

    if t <= td {
        v0 + va * phi_rad.sin()
    } else {
        let dt = t - td;
        let damping = if theta != 0.0 {
            (-theta * dt).exp()
        } else {
            1.0
        };
        v0 + va * (2.0 * PI * freq * dt + phi_rad).sin() * damping
    }
}

/// EXP waveform.
///
/// t <= td1: v1
/// td1 < t <= td2: v1 + (v2-v1)*(1 - exp(-(t-td1)/tau1))
/// t > td2: above + (v1-v2)*(1 - exp(-(t-td2)/tau2))
fn eval_exp(v1: f64, v2: f64, td1: f64, tau1: f64, td2: f64, tau2: f64, t: f64) -> f64 {
    if t <= td1 {
        v1
    } else if t <= td2 {
        v1 + (v2 - v1) * (1.0 - (-(t - td1) / tau1).exp())
    } else {
        v1 + (v2 - v1) * (1.0 - (-(t - td1) / tau1).exp())
            + (v1 - v2) * (1.0 - (-(t - td2) / tau2).exp())
    }
}

/// PWL (piecewise linear) waveform.
///
/// Linear interpolation between time-value pairs.
/// Before first point: first value. After last point: last value.
fn eval_pwl(points: &[(f64, f64)], t: f64) -> f64 {
    if points.is_empty() {
        return 0.0;
    }

    // Before first point.
    if t <= points[0].0 {
        return points[0].1;
    }

    // After last point — hold last value.
    if t >= points[points.len() - 1].0 {
        return points[points.len() - 1].1;
    }

    // Find the segment containing t.
    for i in 1..points.len() {
        if t <= points[i].0 {
            let (t0, v0) = points[i - 1];
            let (t1, v1) = points[i];
            let dt = t1 - t0;
            if dt <= 0.0 {
                return v1;
            }
            return v0 + (v1 - v0) * (t - t0) / dt;
        }
    }

    points[points.len() - 1].1
}

/// SFFM (single-frequency FM) waveform.
///
/// v0 + va * sin(2*pi*fc*t + md*sin(2*pi*fs*t))
fn eval_sffm(v0: f64, va: f64, fc: f64, fs: f64, md: f64, t: f64) -> f64 {
    v0 + va * (2.0 * PI * fc * t + md * (2.0 * PI * fs * t).sin()).sin()
}

/// AM (amplitude modulation) waveform.
///
/// Before td: 0
/// After td: va * (vo + sin(2*pi*fs*t)) * sin(2*pi*fc*t)
///
/// Note: ngspice AM parameters are (VO, VMO, VMA, FM, FC, TD, ...) but
/// thevenin-types parses as (va, vo, fc, fs, td) — mapping:
/// va = amplitude, vo = offset, fc = carrier freq, fs = signal freq
fn eval_am(va: f64, vo: f64, fc: f64, fs: f64, td: f64, t: f64) -> f64 {
    if t < td {
        0.0
    } else {
        va * (vo + (2.0 * PI * fs * t).sin()) * (2.0 * PI * fc * t).sin()
    }
}

/// Collect breakpoint times from a waveform within [0, tstop].
///
/// Breakpoints are times where the waveform has a discontinuity in value or
/// first derivative (e.g., PULSE edges, PWL corners, EXP transitions).
/// The transient engine uses these to force timestep boundaries at these points.
pub fn breakpoints(wf: &Waveform, tran: &TranParams) -> Vec<f64> {
    let tstop = tran.tstop;
    let mut bps = Vec::new();

    match wf {
        Waveform::Pulse {
            td,
            tr,
            tf,
            pw,
            per,
            ..
        } => {
            let td_val = td.unwrap_or(0.0);
            let tr_val = tr.unwrap_or(tran.tstep).max(tran.tstep);
            let tf_val = tf.unwrap_or(tran.tstep).max(tran.tstep);
            let pw_val = pw.unwrap_or(tran.tstop).max(0.0);
            let period = per
                .unwrap_or(tr_val + pw_val + tf_val)
                .max(tr_val + pw_val + tf_val)
                .max(tran.tstep);

            // Breakpoints within one period relative to td:
            // td, td+tr, td+tr+pw, td+tr+pw+tf
            let edges = [0.0, tr_val, tr_val + pw_val, tr_val + pw_val + tf_val];

            let mut k = 0u64;
            loop {
                let base = td_val + k as f64 * period;
                if base > tstop {
                    break;
                }
                for &edge in &edges {
                    let t = base + edge;
                    if t >= 0.0 && t <= tstop {
                        bps.push(t);
                    }
                }
                k += 1;
                // Safety limit to avoid infinite loop with tiny periods.
                if k > 1_000_000 {
                    break;
                }
            }
        }
        Waveform::Pwl(points) => {
            for (t, _) in points {
                if *t >= 0.0 && *t <= tstop {
                    bps.push(*t);
                }
            }
        }
        Waveform::Exp { td1, td2, .. } => {
            let td1_val = td1.unwrap_or(tran.tstep);
            let td2_val = td2.unwrap_or(td1_val + tran.tstep);
            if td1_val >= 0.0 && td1_val <= tstop {
                bps.push(td1_val);
            }
            if td2_val >= 0.0 && td2_val <= tstop {
                bps.push(td2_val);
            }
        }
        Waveform::Sin { td, .. } => {
            let td_val = td.unwrap_or(0.0);
            if td_val > 0.0 && td_val <= tstop {
                bps.push(td_val);
            }
        }
        Waveform::Am { td, .. } => {
            let td_val = td.unwrap_or(0.0);
            if td_val > 0.0 && td_val <= tstop {
                bps.push(td_val);
            }
        }
        Waveform::Sffm { .. } => {
            // SFFM is smooth everywhere, no breakpoints.
        }
        // `cirq_ir::Waveform` is `#[non_exhaustive]`; an unknown future
        // variant contributes no breakpoints until handled explicitly.
        _ => {}
    }

    bps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    bps.dedup_by(|a, b| (*a - *b).abs() < 1e-18);
    bps
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    fn tran() -> TranParams {
        TranParams {
            tstep: 1e-6,
            tstop: 1e-3,
        }
    }

    #[test]
    fn test_pulse_basic() {
        let wf = Waveform::Pulse {
            v1: 0.0,
            v2: 5.0,
            td: Some(0.0),
            tr: Some(1e-6),
            tf: Some(1e-6),
            pw: Some(10e-6),
            per: Some(20e-6),
        };
        let tp = tran();

        // Before delay: v1
        assert_eq!(evaluate(&wf, -1e-6, &tp), 0.0);

        // Middle of rise
        let v = evaluate(&wf, 0.5e-6, &tp);
        assert!((v - 2.5).abs() < 0.01, "mid-rise: {v}");

        // At pulse high
        assert_eq!(evaluate(&wf, 5e-6, &tp), 5.0);

        // At pulse end (start of fall)
        let v = evaluate(&wf, 11.5e-6, &tp);
        assert!((v - 2.5).abs() < 0.01, "mid-fall: {v}");

        // After fall (low)
        assert_eq!(evaluate(&wf, 15e-6, &tp), 0.0);

        // Next period: high again
        assert_eq!(evaluate(&wf, 25e-6, &tp), 5.0);
    }

    #[test]
    fn test_sin_basic() {
        let wf = Waveform::Sin {
            v0: 0.0,
            va: 1.0,
            freq: Some(1000.0),
            td: Some(0.0),
            theta: Some(0.0),
            phi: Some(0.0),
        };
        let tp = tran();

        // At t=0: sin(0) = 0
        assert!((evaluate(&wf, 0.0, &tp)).abs() < 1e-10);

        // At t=0.25ms: sin(pi/2) = 1
        let v = evaluate(&wf, 0.25e-3, &tp);
        assert!((v - 1.0).abs() < 1e-10, "quarter period: {v}");

        // At t=0.5ms: sin(pi) = 0
        let v = evaluate(&wf, 0.5e-3, &tp);
        assert!(v.abs() < 1e-10, "half period: {v}");

        // At t=0.75ms: sin(3*pi/2) = -1
        let v = evaluate(&wf, 0.75e-3, &tp);
        assert!((v + 1.0).abs() < 1e-10, "3/4 period: {v}");
    }

    #[test]
    fn test_sin_with_offset_and_phase() {
        let wf = Waveform::Sin {
            v0: 2.5,
            va: 1.0,
            freq: Some(1000.0),
            td: None,
            theta: None,
            phi: Some(90.0), // 90 degrees
        };
        let tp = tran();

        // At t=0 (before td=0): v0 + va*sin(90°) = 2.5 + 1.0 = 3.5
        let v = evaluate(&wf, 0.0, &tp);
        assert!((v - 3.5).abs() < 1e-10, "t=0 with phase: {v}");
    }

    #[test]
    fn test_sin_with_damping() {
        let wf = Waveform::Sin {
            v0: 0.0,
            va: 1.0,
            freq: Some(1000.0),
            td: Some(0.0),
            theta: Some(1000.0), // Heavy damping
            phi: None,
        };
        let tp = tran();

        // At t=0.25ms: sin(pi/2)*exp(-1000*0.00025) = 1 * exp(-0.25)
        let v = evaluate(&wf, 0.25e-3, &tp);
        let expected = (-0.25_f64).exp();
        assert!((v - expected).abs() < 1e-6, "damped: {v} vs {expected}");
    }

    #[test]
    fn test_exp_basic() {
        let wf = Waveform::Exp {
            v1: 0.0,
            v2: 5.0,
            td1: Some(0.0),
            tau1: Some(1e-3),
            td2: Some(5e-3),
            tau2: Some(1e-3),
        };
        let tp = tran();

        // At t=0: v1
        assert_eq!(evaluate(&wf, 0.0, &tp), 0.0);

        // At t=1ms: v1 + (v2-v1)*(1-exp(-1)) = 5*(1-0.368) ≈ 3.16
        let v = evaluate(&wf, 1e-3, &tp);
        let expected = 5.0 * (1.0 - (-1.0_f64).exp());
        assert!((v - expected).abs() < 1e-6, "rise: {v} vs {expected}");

        // At t=6ms (1ms after td2): should be decaying back
        let v = evaluate(&wf, 6e-3, &tp);
        assert!(v < 5.0, "should be decaying: {v}");
    }

    #[test]
    fn test_pwl_basic() {
        let wf = Waveform::Pwl(vec![(0.0, 0.0), (1e-3, 5.0), (2e-3, 5.0), (3e-3, 0.0)]);
        let tp = tran();

        assert_eq!(evaluate(&wf, 0.0, &tp), 0.0);
        assert!((evaluate(&wf, 0.5e-3, &tp) - 2.5).abs() < 1e-10);
        assert_eq!(evaluate(&wf, 1e-3, &tp), 5.0);
        assert_eq!(evaluate(&wf, 1.5e-3, &tp), 5.0);
        assert!((evaluate(&wf, 2.5e-3, &tp) - 2.5).abs() < 1e-10);
        assert_eq!(evaluate(&wf, 3e-3, &tp), 0.0);
        // After last point: hold
        assert_eq!(evaluate(&wf, 5e-3, &tp), 0.0);
    }

    #[test]
    fn test_sffm_basic() {
        let wf = Waveform::Sffm {
            v0: 1.0,
            va: 2.0,
            fc: Some(1000.0),
            fs: Some(100.0),
            md: Some(5.0),
        };
        let tp = tran();

        // At t=0: v0 + va*sin(0 + md*sin(0)) = 1 + 2*sin(0) = 1.0
        let v = evaluate(&wf, 0.0, &tp);
        assert!((v - 1.0).abs() < 1e-10, "sffm at t=0: {v}");
    }

    #[test]
    fn test_am_basic() {
        let wf = Waveform::Am {
            va: 1.0,
            vo: 1.0,
            fc: 10000.0,
            fs: 1000.0,
            td: Some(0.0),
        };
        let tp = tran();

        // At t=0: va*(vo + sin(0))*sin(0) = 0
        let v = evaluate(&wf, 0.0, &tp);
        assert!(v.abs() < 1e-10, "am at t=0: {v}");
    }

    #[test]
    fn test_sin_matches_analytical() {
        // Acceptance criteria: SIN source output samples match analytical sin() within 1e-6
        let freq = 1000.0;
        let va = 3.3;
        let v0 = 1.65;
        let wf = Waveform::Sin {
            v0,
            va,
            freq: Some(freq),
            td: Some(0.0),
            theta: Some(0.0),
            phi: Some(0.0),
        };
        let tp = tran();

        // Check at many time points
        for i in 0..100 {
            let t = i as f64 * 1e-5; // 0 to 1ms
            let expected = v0 + va * (2.0 * PI * freq * t).sin();
            let actual = evaluate(&wf, t, &tp);
            assert!(
                (actual - expected).abs() < 1e-6,
                "at t={t:.6e}: expected={expected:.10}, got={actual:.10}"
            );
        }
    }
}
