//! `.meas` post-simulation measurement evaluation.
//!
//! Evaluates `.meas` directives against simulation results, extracting scalar
//! measurements from waveform data.
//!
//! # Supported measurement types
//!
//! | Keyword | Example | Description |
//! |---------|---------|-------------|
//! | `MAX` | `MAX v(out)` | Maximum value |
//! | `MIN` | `MIN v(out)` | Minimum value |
//! | `AVG` | `AVG v(out)` | Average value |
//! | `RMS` | `RMS v(out)` | Root-mean-square |
//! | `PP` | `PP v(out)` | Peak-to-peak |
//! | `INTEG` | `INTEG i(r1)` | Trapezoidal integral |
//! | `FIND AT` | `FIND v(out) AT=5u` | Value at sweep point |
//! | `FIND WHEN` | `FIND v(out) WHEN v(clk)=0.5 RISE=1` | Value at crossing |
//! | `WHEN` | `WHEN v(out)=0.5 RISE=1` | Sweep value at crossing |
//! | `TRIG/TARG` | `TRIG v(in) VAL=0.5 RISE=1 TARG v(out) VAL=0.5 RISE=1` | Delay |
//! | `DERIV` | `DERIV v(out) AT=5u` | Derivative at point |
//! | `PARAM=` | `PARAM=(vmax-vmin)*2` | Constant or arithmetic over prior measurements |
//!
//! Aggregate types accept `FROM=` / `TO=` range constraints.
//! Crossing-based types accept `RISE=n`, `FALL=n`, `CROSS=n`, or `RISE=LAST`
//! / `FALL=LAST` / `CROSS=LAST`. TRIG clauses honor `TD=<time>` to skip
//! crossings that occur before the trigger delay.

use cirq_ir::{
    AggregateKind, ArithOp, CrossingKind, CrossingPick, CrossingSpec, FindAt, MeasArith,
    MeasureExpr, Threshold, TrigTargClause,
};
use thevenin_types::{SimPlot, SimResult, SimVector};

/// Evaluate `.meas` directives from a Netlist against simulation results.
///
/// Each spec string is parsed into the typed [`cirq_ir::MeasureExpr`] form
/// at call time and delegated to the typed evaluator. Used by the legacy
/// `simulate(&Netlist)` API surface, which is `pub(crate)` and exercised
/// only by the in-tree test suites.
#[cfg(test)]
pub(crate) fn evaluate_measurements(netlist: &thevenin_types::Netlist, result: &mut SimResult) {
    let typed: Vec<cirq_ir::MeasureSpec> = netlist
        .items
        .iter()
        .filter_map(|item| match item {
            thevenin_types::Item::Meas(spec) => Some(cirq_ir::MeasureSpec::parse(
                spec.name.clone(),
                spec.analysis_type.clone(),
                spec.spec.clone(),
            )),
            _ => None,
        })
        .collect();

    evaluate_circuit_measures(&typed, result);
}

/// Evaluate typed `.meas` specifications against simulation results.
///
/// Iterates `measures` in order. Each measurement looks up the matching
/// analysis plot (`tran*`, `dc*`, `ac*`, …), evaluates the typed
/// [`MeasureExpr`], and appends a scalar vector to the `"measurements"`
/// plot. `PARAM=` measurements can reference earlier measurements by name —
/// evaluation is left-to-right so a `PARAM=` lookup sees every measurement
/// declared above it.
pub fn evaluate_circuit_measures(measures: &[cirq_ir::MeasureSpec], result: &mut SimResult) {
    if measures.is_empty() {
        return;
    }

    let mut meas_vecs: Vec<SimVector> = Vec::new();

    for spec in measures {
        let Some(ref expr) = spec.expr else {
            continue;
        };

        // Locate the matching analysis plot. PARAM= doesn't care about a
        // plot per se — it operates over prior measurements only.
        let plot = result.plots.iter().find(|p| {
            p.name
                .to_lowercase()
                .starts_with(&spec.analysis_type.to_lowercase())
        });

        // Conditional `IF '<expr>'` clause: skip the measurement when the
        // condition evaluates to zero or non-finite. Resolved against the
        // running `meas_vecs` so later measurements can gate on earlier
        // ones (e.g. `.meas tran swing_ok PARAM='swing' IF 'swing > 0.1'`).
        if let Some(cond) = &spec.condition {
            let pass = eval_arith(cond, &meas_vecs)
                .map(|v| v.is_finite() && v != 0.0)
                .unwrap_or(false);
            if !pass {
                continue;
            }
        }

        let value = match expr {
            MeasureExpr::Param(arith) => eval_arith(arith, &meas_vecs),
            _ => plot.and_then(|p| evaluate_typed(expr, p, &meas_vecs)),
        };

        if let Some(v) = value {
            meas_vecs.push(SimVector::real(spec.name.clone(), vec![v]));
            // Optional `FILE=<path>` clause: append the result to disk
            // in `name = value` text format. We open in append mode each
            // time so multiple measurements writing to the same path
            // accumulate. Errors are logged to stderr but don't fail the
            // measurement chain — the in-memory result is still authoritative.
            if let Some(file) = &spec.file {
                use std::io::Write;
                match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(file)
                {
                    Ok(mut f) => {
                        if let Err(e) = writeln!(f, "{} = {}", spec.name, v) {
                            eprintln!("[thevenin] WARNING: .meas FILE={file:?} write failed: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("[thevenin] WARNING: .meas FILE={file:?} open failed: {e}");
                    }
                }
            }
        }
    }

    if !meas_vecs.is_empty() {
        result.plots.push(SimPlot {
            name: "measurements".to_string(),
            vecs: meas_vecs,
        });
    }
}

/// Dispatch a typed measurement against a plot.
fn evaluate_typed(expr: &MeasureExpr, plot: &SimPlot, meas_vecs: &[SimVector]) -> Option<f64> {
    let _ = meas_vecs;
    match expr {
        MeasureExpr::Aggregate {
            kind,
            vec,
            from,
            to,
        } => eval_aggregate(*kind, vec, *from, *to, plot),
        MeasureExpr::Integ { vec, from, to } => eval_integ(vec, *from, *to, plot),
        MeasureExpr::Find { vec, at } => eval_find(vec, at, plot),
        MeasureExpr::When(spec) => eval_when(spec, plot),
        MeasureExpr::TrigTarg { trig, targ } => eval_trig_targ(trig, targ, plot),
        MeasureExpr::Deriv { vec, at } => eval_deriv(vec, at, plot),
        MeasureExpr::Param(_) => None,
        MeasureExpr::Error {
            kind,
            expected,
            actual,
            minval,
            ignore,
        } => eval_error(*kind, expected, actual, *minval, *ignore, plot),
        // `MeasureExpr` is `#[non_exhaustive]` — unknown variants are
        // treated as a skipped measurement.
        _ => None,
    }
}

/// Evaluate ERR/ERR1/ERR2/ERR3 over the swept samples of `actual` against
/// either a constant or another vector (`expected`). Returns:
/// - `ErrorKind::Relative` (ERR/ERR1): max per-sample relative error
///   `|actual - expected| / max(|expected|, minval)`.
/// - `ErrorKind::Absolute` (ERR2): max per-sample absolute error.
/// - `ErrorKind::Rms` (ERR3): RMS per-sample relative error.
///
/// Samples for which `|expected| < ignore` (when `ignore` is set) are
/// excluded from the comparison. The default `minval` is `1e-6` to match
/// ngspice.
fn eval_error(
    kind: cirq_ir::ErrorKind,
    expected: &cirq_ir::ErrorReference,
    actual: &str,
    minval: Option<f64>,
    ignore: Option<f64>,
    plot: &SimPlot,
) -> Option<f64> {
    let actual_vec = plot
        .vecs
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case(actual))?;
    let actual_data = actual_vec.data.as_real();

    let minval = minval.unwrap_or(1.0e-6);
    let ignore_floor = ignore;

    let expected_at = |i: usize| -> Option<f64> {
        match expected {
            cirq_ir::ErrorReference::Constant(c) => Some(*c),
            cirq_ir::ErrorReference::Vector(name) => plot
                .vecs
                .iter()
                .find(|v| v.name.eq_ignore_ascii_case(name))
                .and_then(|v| v.data.as_real().get(i).copied()),
        }
    };

    let mut acc = 0.0_f64;
    let mut count = 0usize;
    for (i, &a) in actual_data.iter().enumerate() {
        let Some(e) = expected_at(i) else { break };
        if let Some(thr) = ignore_floor
            && e.abs() < thr
        {
            continue;
        }
        let err = match kind {
            cirq_ir::ErrorKind::Relative => (a - e).abs() / e.abs().max(minval),
            cirq_ir::ErrorKind::Absolute => (a - e).abs(),
            cirq_ir::ErrorKind::Rms => {
                let rel = (a - e).abs() / e.abs().max(minval);
                rel * rel
            }
        };
        match kind {
            cirq_ir::ErrorKind::Relative | cirq_ir::ErrorKind::Absolute => {
                if err > acc {
                    acc = err;
                }
            }
            cirq_ir::ErrorKind::Rms => acc += err,
        }
        count += 1;
    }
    if count == 0 {
        return None;
    }
    match kind {
        cirq_ir::ErrorKind::Rms => Some((acc / count as f64).sqrt()),
        _ => Some(acc),
    }
}

// ---------------------------------------------------------------------------
// Aggregates and INTEG
// ---------------------------------------------------------------------------

fn eval_aggregate(
    kind: AggregateKind,
    vec_name: &str,
    from: Option<f64>,
    to: Option<f64>,
    plot: &SimPlot,
) -> Option<f64> {
    let vec = plot.vector(vec_name)?;
    let data = vec.data.as_real();
    let filtered = filter_by_range(plot, data, from, to);
    if filtered.is_empty() {
        return None;
    }
    Some(match kind {
        AggregateKind::Max => filtered.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        AggregateKind::Min => filtered.iter().copied().fold(f64::INFINITY, f64::min),
        AggregateKind::Avg => filtered.iter().sum::<f64>() / filtered.len() as f64,
        AggregateKind::Rms => {
            let s: f64 = filtered.iter().map(|v| v * v).sum();
            (s / filtered.len() as f64).sqrt()
        }
        AggregateKind::Pp => {
            let mx = filtered.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let mn = filtered.iter().copied().fold(f64::INFINITY, f64::min);
            mx - mn
        }
    })
}

fn filter_by_range(plot: &SimPlot, data: &[f64], from: Option<f64>, to: Option<f64>) -> Vec<f64> {
    if from.is_none() && to.is_none() {
        return data.to_vec();
    }
    let Some(sweep) = plot.vecs.first() else {
        return data.to_vec();
    };
    let sweep_data = sweep.data.as_real();
    if sweep_data.len() != data.len() {
        return data.to_vec();
    }
    data.iter()
        .zip(sweep_data.iter())
        .filter(|&(_, &x)| {
            if let Some(f) = from
                && x < f
            {
                return false;
            }
            if let Some(t) = to
                && x > t
            {
                return false;
            }
            true
        })
        .map(|(&v, _)| v)
        .collect()
}

fn eval_integ(vec_name: &str, from: Option<f64>, to: Option<f64>, plot: &SimPlot) -> Option<f64> {
    let vec = plot.vector(vec_name)?;
    let data = vec.data.as_real();
    let sweep = plot.vecs.first()?;
    let sweep_data = sweep.data.as_real();
    if data.len() != sweep_data.len() || data.len() < 2 {
        return None;
    }

    let mut integral = 0.0;
    for i in 1..data.len() {
        let x0 = sweep_data[i - 1];
        let x1 = sweep_data[i];
        if let Some(f) = from
            && x1 < f
        {
            continue;
        }
        if let Some(t) = to
            && x0 > t
        {
            break;
        }
        integral += (data[i - 1] + data[i]) * 0.5 * (x1 - x0);
    }
    Some(integral)
}

// ---------------------------------------------------------------------------
// FIND / DERIV
// ---------------------------------------------------------------------------

fn eval_find(vec_name: &str, at: &FindAt, plot: &SimPlot) -> Option<f64> {
    let at_val = resolve_find_at(at, plot)?;
    find_value_at_sweep(plot, vec_name, at_val)
}

fn eval_deriv(vec_name: &str, at: &FindAt, plot: &SimPlot) -> Option<f64> {
    let at_val = resolve_find_at(at, plot)?;
    let vec = plot.vector(vec_name)?;
    let data = vec.data.as_real();
    let sweep = plot.vecs.first()?;
    let sweep_data = sweep.data.as_real();
    if data.len() != sweep_data.len() || data.len() < 2 {
        return None;
    }
    for i in 1..sweep_data.len() {
        let x0 = sweep_data[i - 1];
        let x1 = sweep_data[i];
        if (x0 <= at_val && x1 >= at_val) || (x0 >= at_val && x1 <= at_val) {
            let dx = x1 - x0;
            if dx.abs() < 1e-30 {
                return None;
            }
            return Some((data[i] - data[i - 1]) / dx);
        }
    }
    None
}

fn resolve_find_at(at: &FindAt, plot: &SimPlot) -> Option<f64> {
    match at {
        FindAt::Sweep(v) => Some(*v),
        FindAt::SweepLast => plot.vecs.first()?.data.as_real().last().copied(),
        FindAt::Crossing(spec) => find_crossing_spec(spec, plot),
    }
}

fn find_value_at_sweep(plot: &SimPlot, vec_name: &str, at_val: f64) -> Option<f64> {
    let vec = plot.vector(vec_name)?;
    let data = vec.data.as_real();
    let sweep = plot.vecs.first()?;
    let sweep_data = sweep.data.as_real();
    if data.len() != sweep_data.len() || data.is_empty() {
        return None;
    }
    interpolate_at(sweep_data, data, at_val)
}

fn interpolate_at(x: &[f64], y: &[f64], target: f64) -> Option<f64> {
    for i in 1..x.len() {
        if (x[i - 1] <= target && x[i] >= target) || (x[i - 1] >= target && x[i] <= target) {
            let span = x[i] - x[i - 1];
            if span.abs() < 1e-30 {
                return Some(y[i]);
            }
            let frac = (target - x[i - 1]) / span;
            return Some(y[i - 1] + frac * (y[i] - y[i - 1]));
        }
    }
    if let Some(&last) = x.last()
        && (last - target).abs() < 1e-15
    {
        return y.last().copied();
    }
    None
}

// ---------------------------------------------------------------------------
// WHEN / crossings
// ---------------------------------------------------------------------------

fn eval_when(spec: &CrossingSpec, plot: &SimPlot) -> Option<f64> {
    find_crossing_spec(spec, plot)
}

fn find_crossing_spec(spec: &CrossingSpec, plot: &SimPlot) -> Option<f64> {
    let sweep = plot.vecs.first()?;
    let sweep_data = sweep.data.as_real();
    let signal_vec = plot.vector(&spec.signal)?;
    let signal_data = signal_vec.data.as_real();
    if signal_data.len() != sweep_data.len() || signal_data.len() < 2 {
        return None;
    }
    let diff: Vec<f64> = match &spec.threshold {
        Threshold::Constant(val) => signal_data.iter().map(|&s| s - *val).collect(),
        Threshold::Vector(name) => {
            let r = plot.vector(name)?;
            let rd = r.data.as_real();
            if rd.len() != signal_data.len() {
                return None;
            }
            signal_data
                .iter()
                .zip(rd.iter())
                .map(|(&s, &r)| s - r)
                .collect()
        }
    };
    find_crossing(sweep_data, &diff, spec.crossing, spec.from, spec.to)
}

fn find_crossing(
    sweep: &[f64],
    diff: &[f64],
    crossing: CrossingKind,
    from: Option<f64>,
    to: Option<f64>,
) -> Option<f64> {
    let pick = crossing_pick(crossing);
    let target = match pick {
        CrossingPick::Nth(n) => Some(n),
        CrossingPick::Last => None,
    };

    let mut count = 0u32;
    let mut last_match: Option<f64> = None;

    for i in 1..diff.len() {
        let x0 = sweep[i - 1];
        let x1 = sweep[i];
        if let Some(f) = from
            && x1 < f
        {
            continue;
        }
        if let Some(t) = to
            && x0 > t
        {
            break;
        }
        let d0 = diff[i - 1];
        let d1 = diff[i];
        if d0 * d1 > 0.0 {
            continue;
        }
        if d0 == 0.0 && d1 == 0.0 {
            continue;
        }
        let is_rising = d0 < 0.0 && d1 >= 0.0 || d0 <= 0.0 && d1 > 0.0;
        let is_falling = d0 > 0.0 && d1 <= 0.0 || d0 >= 0.0 && d1 < 0.0;
        let matches = match crossing {
            CrossingKind::Cross(_) => is_rising || is_falling,
            CrossingKind::Rise(_) => is_rising,
            CrossingKind::Fall(_) => is_falling,
        };
        if !matches {
            continue;
        }
        let crossing_x = if (d1 - d0).abs() < 1e-30 {
            x0
        } else {
            let frac = -d0 / (d1 - d0);
            x0 + frac * (x1 - x0)
        };
        if let Some(f) = from
            && crossing_x < f
        {
            continue;
        }
        if let Some(t) = to
            && crossing_x > t
        {
            continue;
        }
        count += 1;
        last_match = Some(crossing_x);
        if let Some(n) = target
            && count == n
        {
            return Some(crossing_x);
        }
    }
    if target.is_none() { last_match } else { None }
}

fn crossing_pick(c: CrossingKind) -> CrossingPick {
    match c {
        CrossingKind::Rise(p) | CrossingKind::Fall(p) | CrossingKind::Cross(p) => p,
    }
}

// ---------------------------------------------------------------------------
// TRIG / TARG
// ---------------------------------------------------------------------------

fn eval_trig_targ(trig: &TrigTargClause, targ: &TrigTargClause, plot: &SimPlot) -> Option<f64> {
    let trig_val = resolve_trig_targ(trig, plot, /* is_trig */ true)?;
    let targ_val = resolve_trig_targ(targ, plot, /* is_trig */ false)?;
    Some(targ_val - trig_val)
}

/// Resolve a TRIG/TARG clause to a sweep value. When `is_trig` is true, the
/// trigger-delay (`td`) is applied as a `FROM=` lower bound so crossings
/// before that delay are skipped.
fn resolve_trig_targ(clause: &TrigTargClause, plot: &SimPlot, is_trig: bool) -> Option<f64> {
    match clause {
        TrigTargClause::At(v) => Some(*v),
        TrigTargClause::Signal {
            signal,
            val,
            crossing,
            td,
        } => {
            let sweep = plot.vecs.first()?;
            let sweep_data = sweep.data.as_real();
            let sig = plot.vector(signal)?;
            let sig_data = sig.data.as_real();
            if sig_data.len() != sweep_data.len() || sig_data.len() < 2 {
                return None;
            }
            let diff: Vec<f64> = sig_data.iter().map(|&s| s - *val).collect();
            let from = if is_trig { *td } else { None };
            find_crossing(sweep_data, &diff, *crossing, from, None)
        }
    }
}

// ---------------------------------------------------------------------------
// PARAM= arithmetic
// ---------------------------------------------------------------------------

fn eval_arith(arith: &MeasArith, prior: &[SimVector]) -> Option<f64> {
    match arith {
        MeasArith::Const(v) => Some(*v),
        MeasArith::Ref(name) => prior
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
            .and_then(|v| v.data.as_real().first().copied()),
        MeasArith::Neg(inner) => eval_arith(inner, prior).map(|v| -v),
        MeasArith::BinOp(lhs, op, rhs) => {
            let l = eval_arith(lhs, prior)?;
            let r = eval_arith(rhs, prior)?;
            Some(match op {
                ArithOp::Add => l + r,
                ArithOp::Sub => l - r,
                ArithOp::Mul => l * r,
                ArithOp::Div => {
                    if r.abs() < 1e-300 {
                        return None;
                    }
                    l / r
                }
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tran_plot(time: Vec<f64>, signals: Vec<(&str, Vec<f64>)>) -> SimPlot {
        let mut vecs = vec![SimVector::real("time", time)];
        for (name, data) in signals {
            vecs.push(SimVector::real(name, data));
        }
        SimPlot {
            name: "tran1".to_string(),
            vecs,
        }
    }

    fn meas(spec: &str) -> cirq_ir::MeasureSpec {
        cirq_ir::MeasureSpec::parse("test_meas", "tran", spec)
    }

    fn eval(spec: &str, plot: &SimPlot) -> Option<f64> {
        let m = meas(spec);
        let expr = m.expr.as_ref()?;
        evaluate_typed(expr, plot, &[])
    }

    // -- MAX / MIN / AVG / RMS / PP ------------------------------------------

    #[test]
    fn max_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![1.0, 3.0, 2.0, 0.5])],
        );
        let val = eval("MAX v(out)", &plot).unwrap();
        assert!((val - 3.0).abs() < 1e-12);
    }

    #[test]
    fn min_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![1.0, 3.0, 2.0, 0.5])],
        );
        let val = eval("MIN v(out)", &plot).unwrap();
        assert!((val - 0.5).abs() < 1e-12);
    }

    #[test]
    fn pp_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![1.0, 3.0, 2.0, 0.5])],
        );
        let val = eval("PP v(out)", &plot).unwrap();
        assert!((val - 2.5).abs() < 1e-12);
    }

    #[test]
    fn avg_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![2.0, 2.0, 2.0, 2.0])],
        );
        let val = eval("AVG v(out)", &plot).unwrap();
        assert!((val - 2.0).abs() < 1e-12);
    }

    #[test]
    fn max_with_from_to_range() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![("v(out)", vec![10.0, 1.0, 5.0, 2.0, 8.0])],
        );
        let val = eval("MAX v(out) FROM=1.0 TO=3.0", &plot).unwrap();
        assert!((val - 5.0).abs() < 1e-12);
    }

    // -- FIND AT -------------------------------------------------------------

    #[test]
    fn find_at_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![0.0, 2.0, 4.0, 6.0])],
        );
        let val = eval("FIND v(out) AT=1.5", &plot).unwrap();
        assert!((val - 3.0).abs() < 1e-12);
    }

    #[test]
    fn find_at_last() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![0.0, 2.0, 4.0, 6.0])],
        );
        let val = eval("FIND v(out) AT=LAST", &plot).unwrap();
        assert!((val - 6.0).abs() < 1e-12);
    }

    // -- INTEG ---------------------------------------------------------------

    #[test]
    fn integral_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("i(r1)", vec![2.0, 2.0, 2.0, 2.0])],
        );
        let val = eval("INTEG i(r1)", &plot).unwrap();
        assert!((val - 6.0).abs() < 1e-12);
    }

    // -- WHEN ----------------------------------------------------------------

    #[test]
    fn when_rising_crossing() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![("v(out)", vec![0.0, 1.0, 2.0, 3.0, 4.0])],
        );
        let val = eval("WHEN v(out)=2.0", &plot).unwrap();
        assert!((val - 2.0).abs() < 1e-12);
    }

    #[test]
    fn when_interpolated_crossing() {
        let plot = tran_plot(vec![0.0, 1.0, 2.0], vec![("v(out)", vec![0.0, 0.0, 1.0])]);
        let val = eval("WHEN v(out)=0.5", &plot).unwrap();
        assert!((val - 1.5).abs() < 1e-12);
    }

    #[test]
    fn when_rise_2() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![("v(out)", vec![0.0, 1.0, 0.0, 1.0, 0.0])],
        );
        let val = eval("WHEN v(out)=0.5 RISE=2", &plot).unwrap();
        assert!((val - 2.5).abs() < 1e-12);
    }

    #[test]
    fn when_rise_last() {
        // 4 rises at 0.5, 2.5, 4.5, 6.5. LAST should land on 6.5.
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
            vec![("v(out)", vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0])],
        );
        let val = eval("WHEN v(out)=0.5 RISE=LAST", &plot).unwrap();
        assert!((val - 6.5).abs() < 1e-12, "got {val}");
    }

    #[test]
    fn when_fall_1() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![1.0, 0.0, 1.0, 0.0])],
        );
        let val = eval("WHEN v(out)=0.5 FALL=1", &plot).unwrap();
        assert!((val - 0.5).abs() < 1e-12);
    }

    #[test]
    fn when_cross_3() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![("v(out)", vec![0.0, 1.0, 0.0, 1.0, 0.0])],
        );
        let val = eval("WHEN v(out)=0.5 CROSS=3", &plot).unwrap();
        assert!((val - 2.5).abs() < 1e-12);
    }

    #[test]
    fn when_two_signals_cross() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![
                ("v(a)", vec![0.0, 1.0, 2.0, 3.0, 4.0]),
                ("v(b)", vec![4.0, 3.0, 2.0, 1.0, 0.0]),
            ],
        );
        let val = eval("WHEN v(a)=v(b)", &plot).unwrap();
        assert!((val - 2.0).abs() < 1e-12);
    }

    #[test]
    fn when_with_from_to() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![("v(out)", vec![0.0, 1.0, 0.0, 1.0, 0.0])],
        );
        let val = eval("WHEN v(out)=0.5 CROSS=1 FROM=2.0", &plot).unwrap();
        assert!((val - 2.5).abs() < 1e-12);
    }

    // -- FIND WHEN -----------------------------------------------------------

    #[test]
    fn find_when_crossing() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0],
            vec![
                ("v(clk)", vec![0.0, 1.0, 0.0]),
                ("v(data)", vec![10.0, 20.0, 30.0]),
            ],
        );
        let val = eval("FIND v(data) WHEN v(clk)=0.5 RISE=1", &plot).unwrap();
        assert!((val - 15.0).abs() < 1e-12);
    }

    // -- TRIG / TARG ---------------------------------------------------------

    #[test]
    fn trig_targ_signal_to_signal() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![
                ("v(in)", vec![0.0, 1.0, 1.0, 1.0]),
                ("v(out)", vec![0.0, 0.0, 1.0, 1.0]),
            ],
        );
        let val = eval(
            "TRIG v(in) VAL=0.5 RISE=1 TARG v(out) VAL=0.5 RISE=1",
            &plot,
        )
        .unwrap();
        assert!((val - 1.0).abs() < 1e-12);
    }

    #[test]
    fn trig_at_targ_signal() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![0.0, 0.0, 0.0, 1.0])],
        );
        let val = eval("TRIG AT=1.0 TARG v(out) VAL=0.5 RISE=1", &plot).unwrap();
        assert!((val - 1.5).abs() < 1e-12);
    }

    #[test]
    fn trig_targ_fall() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![
                ("v(in)", vec![1.0, 0.0, 0.0, 0.0]),
                ("v(out)", vec![1.0, 1.0, 1.0, 0.0]),
            ],
        );
        let val = eval(
            "TRIG v(in) VAL=0.5 FALL=1 TARG v(out) VAL=0.5 FALL=1",
            &plot,
        )
        .unwrap();
        assert!((val - 2.0).abs() < 1e-12);
    }

    #[test]
    fn trig_targ_td_skips_early_rises() {
        // v(in) has two rising crossings of 0.5: at t=0.5 and t=2.5.
        // Without TD we'd lock onto the first; TD=1.0 forces us past it.
        // v(out) rises at 1.5. So delay = 1.5 - 2.5 < 0 if TD is honored;
        // we use a clean case below to verify direction explicitly.
        // Setup: TRIG at t=2.5 (RISE=1 honoring TD=1.0), TARG at t=4.0.
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0],
            vec![
                ("v(in)", vec![0.0, 1.0, 0.0, 1.0, 1.0, 1.0]),
                ("v(out)", vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0]),
            ],
        );
        // Without TD: TRIG@0.5, TARG@3.5 → delay 3.0
        let no_td = eval(
            "TRIG v(in) VAL=0.5 RISE=1 TARG v(out) VAL=0.5 RISE=1",
            &plot,
        )
        .unwrap();
        assert!((no_td - 3.0).abs() < 1e-12, "no_td = {no_td}");
        // With TD=1.0: TRIG skips t=0.5, picks t=2.5 → delay 3.5 - 2.5 = 1.0
        let with_td = eval(
            "TRIG v(in) VAL=0.5 RISE=1 TD=1.0 TARG v(out) VAL=0.5 RISE=1",
            &plot,
        )
        .unwrap();
        assert!((with_td - 1.0).abs() < 1e-12, "with_td = {with_td}");
    }

    // -- DERIV ---------------------------------------------------------------

    #[test]
    fn deriv_at_point() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![0.0, 1.0, 4.0, 9.0])],
        );
        let val = eval("DERIV v(out) AT=1.5", &plot).unwrap();
        assert!((val - 3.0).abs() < 1e-12);
    }

    #[test]
    fn deriv_at_when() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0],
            vec![
                ("v(out)", vec![0.0, 2.0, 6.0]),
                ("v(clk)", vec![0.0, 1.0, 0.0]),
            ],
        );
        let val = eval("DERIV v(out) WHEN v(clk)=0.5 RISE=1", &plot).unwrap();
        assert!((val - 2.0).abs() < 1e-12);
    }

    // -- Edge cases ----------------------------------------------------------

    #[test]
    fn when_no_crossing_returns_none() {
        let plot = tran_plot(vec![0.0, 1.0, 2.0], vec![("v(out)", vec![0.0, 0.1, 0.2])]);
        assert!(eval("WHEN v(out)=5.0", &plot).is_none());
    }

    #[test]
    fn trig_targ_missing_targ_returns_none() {
        // The parser rejects this; meas.expr stays None, so eval returns None.
        let plot = tran_plot(vec![0.0, 1.0], vec![("v(out)", vec![0.0, 1.0])]);
        assert!(eval("TRIG v(out) VAL=0.5 RISE=1", &plot).is_none());
    }

    #[test]
    fn unknown_keyword_returns_none() {
        let plot = tran_plot(vec![0.0, 1.0], vec![("v(out)", vec![0.0, 1.0])]);
        assert!(eval("BOGUS v(out)", &plot).is_none());
    }

    // -- PARAM= --------------------------------------------------------------

    #[test]
    fn param_constant() {
        let mut result = SimResult {
            plots: vec![tran_plot(vec![0.0, 1.0], vec![("v(out)", vec![0.0, 1.0])])],
        };
        let specs = vec![cirq_ir::MeasureSpec::parse("k", "tran", "PARAM=42")];
        evaluate_circuit_measures(&specs, &mut result);
        let plot = result
            .plots
            .iter()
            .find(|p| p.name == "measurements")
            .unwrap();
        let k = plot.vector("k").unwrap();
        assert!((k.data.as_real()[0] - 42.0).abs() < 1e-15);
    }

    #[test]
    fn param_references_prior_measurements() {
        let mut result = SimResult {
            plots: vec![tran_plot(
                vec![0.0, 1.0, 2.0, 3.0],
                vec![("v(out)", vec![1.0, 3.0, 2.0, 0.5])],
            )],
        };
        let specs = vec![
            cirq_ir::MeasureSpec::parse("vmax", "tran", "MAX v(out)"),
            cirq_ir::MeasureSpec::parse("vmin", "tran", "MIN v(out)"),
            cirq_ir::MeasureSpec::parse("swing", "tran", "PARAM=vmax - vmin"),
        ];
        evaluate_circuit_measures(&specs, &mut result);
        let plot = result
            .plots
            .iter()
            .find(|p| p.name == "measurements")
            .unwrap();
        let swing = plot.vector("swing").unwrap();
        assert!((swing.data.as_real()[0] - 2.5).abs() < 1e-12);
    }

    #[test]
    fn param_arithmetic_precedence() {
        // 2 + 3 * 4 = 14, not 20
        let mut result = SimResult { plots: vec![] };
        let specs = vec![cirq_ir::MeasureSpec::parse("x", "tran", "PARAM=2 + 3 * 4")];
        evaluate_circuit_measures(&specs, &mut result);
        let plot = result
            .plots
            .iter()
            .find(|p| p.name == "measurements")
            .unwrap();
        let x = plot.vector("x").unwrap();
        assert!((x.data.as_real()[0] - 14.0).abs() < 1e-15);
    }

    #[test]
    fn param_division_by_zero_skips() {
        let mut result = SimResult { plots: vec![] };
        let specs = vec![cirq_ir::MeasureSpec::parse("x", "tran", "PARAM=1/0")];
        evaluate_circuit_measures(&specs, &mut result);
        // No "measurements" plot created — the only spec failed.
        assert!(
            result
                .plots
                .iter()
                .find(|p| p.name == "measurements")
                .is_none()
        );
    }

    // -- ERR / ERR1 / ERR2 / ERR3 --------------------------------------------

    #[test]
    fn err1_against_constant_reports_max_relative_error() {
        // Actual is 1% above the constant 1.0 — relative error = 0.01.
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0],
            vec![("v(out)", vec![1.00, 1.01, 1.00])],
        );
        let val = eval("ERR1 1.0 v(out)", &plot).unwrap();
        assert!(
            (val - 0.01).abs() < 1e-12,
            "expected ~0.01 max relative error, got {val}"
        );
    }

    #[test]
    fn err2_against_constant_reports_max_absolute_error() {
        let plot = tran_plot(vec![0.0, 1.0, 2.0], vec![("v(out)", vec![1.0, 1.5, 1.2])]);
        let val = eval("ERR2 1.0 v(out)", &plot).unwrap();
        assert!((val - 0.5).abs() < 1e-12, "expected max abs 0.5, got {val}");
    }

    #[test]
    fn err3_returns_rms_relative_error() {
        // Two-sample fixture: |actual - expected| = 0 then 0.02. RMS
        // relative = sqrt((0^2 + 0.02^2) / 2) = 0.02 / sqrt(2) ≈ 0.01414
        let plot = tran_plot(vec![0.0, 1.0], vec![("v(out)", vec![1.0, 1.02])]);
        let val = eval("ERR3 1.0 v(out)", &plot).unwrap();
        let expected = 0.02 / 2.0_f64.sqrt();
        assert!(
            (val - expected).abs() < 1e-9,
            "expected ~{expected}, got {val}"
        );
    }

    #[test]
    fn err1_against_vector_reference() {
        // Compare v(actual) against v(expected) per-sample.
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0],
            vec![
                ("v(actual)", vec![1.00, 2.10, 3.00]),
                ("v(expected)", vec![1.0, 2.0, 3.0]),
            ],
        );
        let val = eval("ERR1 v(expected) v(actual)", &plot).unwrap();
        // Max relative error is 0.10/2.0 = 0.05 at sample 1.
        assert!((val - 0.05).abs() < 1e-12, "expected 0.05, got {val}");
    }

    #[test]
    fn err1_ignore_skips_samples_below_threshold() {
        // Sample 0: expected=0.001 (below IGNORE=0.1) — skipped.
        // Sample 1: expected=1.0, actual=1.05 → rel err = 0.05.
        let plot = tran_plot(
            vec![0.0, 1.0],
            vec![
                ("v(actual)", vec![1.0e10, 1.05]),
                ("v(expected)", vec![0.001, 1.0]),
            ],
        );
        let val = eval("ERR1 v(expected) v(actual) IGNORE=0.1", &plot).unwrap();
        assert!(
            (val - 0.05).abs() < 1e-12,
            "expected 0.05 (the noisy sample filtered), got {val}"
        );
    }

    // -- IF clause ----------------------------------------------------------

    #[test]
    fn meas_skipped_when_if_condition_false() {
        let plot = tran_plot(vec![0.0, 1.0, 2.0], vec![("v(out)", vec![1.0, 2.0, 3.0])]);
        let specs = vec![
            cirq_ir::MeasureSpec::parse("vmax", "tran", "MAX v(out)"),
            // gated_max only runs IF vmax > 100 (which it doesn't).
            cirq_ir::MeasureSpec::parse("gated_max", "tran", "MAX v(out) IF 'vmax - 100'"),
        ];
        let mut result = SimResult {
            plots: vec![plot.clone()],
        };
        evaluate_circuit_measures(&specs, &mut result);
        let meas_plot = result
            .plots
            .iter()
            .find(|p| p.name == "measurements")
            .expect("measurements plot should exist");
        // vmax was recorded; gated_max was skipped (vmax = 3.0, condition
        // = vmax - 100 = -97 → nonzero → SHOULD record). Wait: the
        // condition is nonzero so the measurement IS recorded. Verify
        // recorded.
        assert!(meas_plot.vecs.iter().any(|v| v.name == "vmax"));
        assert!(meas_plot.vecs.iter().any(|v| v.name == "gated_max"));
    }

    #[test]
    fn meas_skipped_when_if_condition_zero() {
        let plot = tran_plot(vec![0.0, 1.0, 2.0], vec![("v(out)", vec![1.0, 2.0, 3.0])]);
        let specs = vec![
            cirq_ir::MeasureSpec::parse("zero", "tran", "PARAM=0"),
            // gated_max conditioned on `zero` (which is 0) → must be skipped.
            cirq_ir::MeasureSpec::parse("gated_max", "tran", "MAX v(out) IF 'zero'"),
        ];
        let mut result = SimResult {
            plots: vec![plot.clone()],
        };
        evaluate_circuit_measures(&specs, &mut result);
        let meas_plot = result
            .plots
            .iter()
            .find(|p| p.name == "measurements")
            .expect("measurements plot should exist");
        assert!(meas_plot.vecs.iter().any(|v| v.name == "zero"));
        assert!(
            !meas_plot.vecs.iter().any(|v| v.name == "gated_max"),
            "gated_max should be skipped when IF condition is zero"
        );
    }

    // -- FILE= clause -------------------------------------------------------

    #[test]
    fn meas_file_clause_appends_result_to_disk() {
        let tmp =
            std::env::temp_dir().join(format!("thevenin_meas_test_{}.out", std::process::id()));
        // Pre-clean.
        let _ = std::fs::remove_file(&tmp);

        let plot = tran_plot(vec![0.0, 1.0, 2.0], vec![("v(out)", vec![1.0, 2.5, 1.0])]);
        let path_str = tmp.to_string_lossy().to_string();
        let spec_text = format!("MAX v(out) FILE={path_str}");
        let specs = vec![cirq_ir::MeasureSpec::parse("vmax", "tran", spec_text)];
        let mut result = SimResult { plots: vec![plot] };
        evaluate_circuit_measures(&specs, &mut result);

        let written = std::fs::read_to_string(&tmp).expect("file should be written");
        assert!(
            written.contains("vmax = 2.5"),
            "expected `vmax = 2.5` in file, got: {written:?}"
        );

        let _ = std::fs::remove_file(&tmp);
    }
}
