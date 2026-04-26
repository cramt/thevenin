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
//!
//! All aggregate types accept optional `FROM=` / `TO=` range constraints.
//! Crossing-based types accept `RISE=n`, `FALL=n`, or `CROSS=n` qualifiers.

use thevenin_types::{Item, MeasureSpec, Netlist, SimPlot, SimResult, SimVector};

/// Evaluate all `.meas` directives in the netlist against simulation results.
///
/// Appends a `"measurements"` plot to the result containing one scalar vector
/// per successful measurement. Measurements that fail (e.g. referencing a
/// missing vector or unsupported syntax) are silently skipped.
pub fn evaluate_measurements(netlist: &Netlist, result: &mut SimResult) {
    let specs: Vec<&MeasureSpec> = netlist
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Meas(spec) = item {
                Some(spec)
            } else {
                None
            }
        })
        .collect();

    if specs.is_empty() {
        return;
    }

    let mut meas_vecs = Vec::new();

    for spec in &specs {
        // Find the matching analysis plot.
        let plot = result.plots.iter().find(|p| {
            p.name
                .to_lowercase()
                .starts_with(&spec.analysis_type.to_lowercase())
        });

        let Some(plot) = plot else {
            continue;
        };

        if let Some(value) = evaluate_single_measurement(spec, plot) {
            meas_vecs.push(SimVector::real(spec.name.clone(), vec![value]));
        }
    }

    if !meas_vecs.is_empty() {
        result.plots.push(SimPlot {
            name: "measurements".to_string(),
            vecs: meas_vecs,
        });
    }
}

/// Evaluate a single `.meas` specification against a simulation plot.
///
/// Returns `Some(value)` on success, `None` if the measurement can't be
/// evaluated (missing vector, unsupported syntax, etc.).
fn evaluate_single_measurement(spec: &MeasureSpec, plot: &SimPlot) -> Option<f64> {
    let tokens = tokenize_meas_spec(&spec.spec);
    if tokens.is_empty() {
        return None;
    }

    let keyword = tokens[0].to_uppercase();
    match keyword.as_str() {
        "MAX" => eval_aggregate(&tokens[1..], plot, |vals| {
            vals.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        }),
        "MIN" => eval_aggregate(&tokens[1..], plot, |vals| {
            vals.iter().copied().fold(f64::INFINITY, f64::min)
        }),
        "AVG" => eval_aggregate(&tokens[1..], plot, |vals| {
            if vals.is_empty() {
                return 0.0;
            }
            vals.iter().sum::<f64>() / vals.len() as f64
        }),
        "RMS" => eval_aggregate(&tokens[1..], plot, |vals| {
            if vals.is_empty() {
                return 0.0;
            }
            let sum_sq: f64 = vals.iter().map(|v| v * v).sum();
            (sum_sq / vals.len() as f64).sqrt()
        }),
        "PP" => eval_aggregate(&tokens[1..], plot, |vals| {
            let max = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
            max - min
        }),
        "INTEG" => eval_integral(&tokens[1..], plot),
        "FIND" => eval_find(&tokens[1..], plot),
        "WHEN" => eval_when(&tokens[1..], plot),
        "TRIG" => eval_trig_targ(&tokens[1..], plot),
        "DERIV" => eval_deriv(&tokens[1..], plot),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Aggregate measurements (MAX, MIN, AVG, RMS, PP)
// ---------------------------------------------------------------------------

/// Parsed vector reference with optional FROM/TO range.
struct VecRefAndRange<'a> {
    name: &'a str,
    from: Option<f64>,
    to: Option<f64>,
}

/// Parse a measurement vector reference like `v(out)` or `i(r1)`, optionally
/// with FROM/TO range constraints.
fn parse_vec_ref_and_range(tokens: &[String]) -> Option<VecRefAndRange<'_>> {
    if tokens.is_empty() {
        return None;
    }
    let vec_name = &tokens[0];
    let mut from = None;
    let mut to = None;
    let mut i = 1;

    while i < tokens.len() {
        let upper = tokens[i].to_uppercase();
        if upper.starts_with("FROM=") {
            from = parse_si_value(upper.strip_prefix("FROM=").unwrap_or(""));
            i += 1;
        } else if upper.starts_with("TO=") {
            to = parse_si_value(upper.strip_prefix("TO=").unwrap_or(""));
            i += 1;
        } else if upper == "FROM" && i + 1 < tokens.len() {
            from = parse_si_value(&tokens[i + 1]);
            i += 2;
        } else if upper == "TO" && i + 1 < tokens.len() {
            to = parse_si_value(&tokens[i + 1]);
            i += 2;
        } else {
            break;
        }
    }

    Some(VecRefAndRange {
        name: vec_name.as_str(),
        from,
        to,
    })
}

/// Evaluate an aggregate measurement (MAX, MIN, AVG, RMS, PP) with optional
/// FROM/TO range.
fn eval_aggregate(tokens: &[String], plot: &SimPlot, f: impl Fn(&[f64]) -> f64) -> Option<f64> {
    let vr = parse_vec_ref_and_range(tokens)?;
    let vec = plot.vector(vr.name)?;
    let data = vec.data.as_real();

    let filtered = filter_by_range(plot, data, vr.from, vr.to);
    if filtered.is_empty() {
        return None;
    }

    Some(f(&filtered))
}

/// Filter data by FROM/TO range using the first (sweep) vector.
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

// ---------------------------------------------------------------------------
// INTEG measurement
// ---------------------------------------------------------------------------

/// Evaluate INTEG measurement using trapezoidal integration.
fn eval_integral(tokens: &[String], plot: &SimPlot) -> Option<f64> {
    let vr = parse_vec_ref_and_range(tokens)?;
    let vec = plot.vector(vr.name)?;
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

        if let Some(f) = vr.from
            && x1 < f
        {
            continue;
        }
        if let Some(t) = vr.to
            && x0 > t
        {
            break;
        }

        integral += (data[i - 1] + data[i]) * 0.5 * (x1 - x0);
    }

    Some(integral)
}

// ---------------------------------------------------------------------------
// FIND measurement (AT= or WHEN)
// ---------------------------------------------------------------------------

/// Evaluate FIND measurement.
///
/// Two forms:
/// - `FIND v(out) AT=5u` — value at a specific sweep point
/// - `FIND v(out) WHEN v(clk)=0.5 [RISE=n|FALL=n|CROSS=n]` — value at crossing
fn eval_find(tokens: &[String], plot: &SimPlot) -> Option<f64> {
    if tokens.is_empty() {
        return None;
    }

    let vec_name = &tokens[0];
    let rest = &tokens[1..];

    // Check for AT= form.
    for token in rest {
        let upper = token.to_uppercase();
        if upper.starts_with("AT=") {
            let at_val = parse_si_value(upper.strip_prefix("AT=").unwrap_or(""))?;
            return find_value_at_sweep(plot, vec_name, at_val);
        }
    }

    // Check for WHEN form: FIND v(out) WHEN v(clk)=0.5 [RISE=n|FALL=n|CROSS=n]
    let when_pos = rest.iter().position(|t| t.to_uppercase() == "WHEN")?;
    let when_tokens = &rest[when_pos + 1..];
    let crossing_time = eval_when_inner(when_tokens, plot)?;
    find_value_at_sweep(plot, vec_name, crossing_time)
}

/// Interpolate a vector's value at a specific sweep point.
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

/// Linearly interpolate `y_data` at the point where `x_data == target`.
fn interpolate_at(x_data: &[f64], y_data: &[f64], target: f64) -> Option<f64> {
    for i in 1..x_data.len() {
        if (x_data[i - 1] <= target && x_data[i] >= target)
            || (x_data[i - 1] >= target && x_data[i] <= target)
        {
            let span = x_data[i] - x_data[i - 1];
            if span.abs() < 1e-30 {
                return Some(y_data[i]);
            }
            let frac = (target - x_data[i - 1]) / span;
            return Some(y_data[i - 1] + frac * (y_data[i] - y_data[i - 1]));
        }
    }

    // Exact endpoint match.
    if let Some(&last) = x_data.last()
        && (last - target).abs() < 1e-15
    {
        return y_data.last().copied();
    }

    None
}

// ---------------------------------------------------------------------------
// WHEN measurement — find sweep value at threshold crossing
// ---------------------------------------------------------------------------

/// Which crossing direction to match.
#[derive(Debug, Clone, Copy)]
enum CrossingType {
    /// Any direction.
    Cross(u32),
    /// Rising (signal goes from below to above threshold).
    Rise(u32),
    /// Falling (signal goes from above to below threshold).
    Fall(u32),
}

impl CrossingType {
    /// The 1-based occurrence count.
    fn count(self) -> u32 {
        match self {
            CrossingType::Cross(n) | CrossingType::Rise(n) | CrossingType::Fall(n) => n,
        }
    }
}

/// A parsed crossing specification: `v(out)=0.5 [RISE=1|FALL=1|CROSS=1]`
/// or `v(out)=v(ref) [RISE=1|FALL=1|CROSS=1]`.
struct CrossingSpec<'a> {
    /// Signal vector name (left side of `=`).
    signal: &'a str,
    /// Threshold: either a constant or another vector name.
    threshold: Threshold<'a>,
    /// Which crossing to report.
    crossing: CrossingType,
    /// Optional FROM bound.
    from: Option<f64>,
    /// Optional TO bound.
    to: Option<f64>,
}

enum Threshold<'a> {
    Constant(f64),
    Vector(&'a str),
}

/// Parse a crossing spec from tokens like `v(out)=0.5 RISE=1 FROM=0 TO=10u`.
///
/// The first token must contain `=` (e.g. `v(out)=0.5` or `v(out)=v(ref)`).
fn parse_crossing_spec<'a>(tokens: &'a [String]) -> Option<CrossingSpec<'a>> {
    if tokens.is_empty() {
        return None;
    }

    // First token: "v(out)=0.5" or "v(out)=v(ref)"
    let (signal, thresh_str) = tokens[0].split_once('=')?;
    if signal.is_empty() || thresh_str.is_empty() {
        return None;
    }

    let threshold = if let Some(val) = parse_si_value(thresh_str) {
        Threshold::Constant(val)
    } else {
        // Treat as vector reference (e.g. v(ref)).
        Threshold::Vector(thresh_str)
    };

    let mut crossing = CrossingType::Cross(1);
    let mut from = None;
    let mut to = None;

    for token in &tokens[1..] {
        let upper = token.to_uppercase();
        if let Some(n_str) = upper.strip_prefix("RISE=") {
            if let Ok(n) = n_str.parse::<u32>() {
                crossing = CrossingType::Rise(n.max(1));
            }
        } else if let Some(n_str) = upper.strip_prefix("FALL=") {
            if let Ok(n) = n_str.parse::<u32>() {
                crossing = CrossingType::Fall(n.max(1));
            }
        } else if let Some(n_str) = upper.strip_prefix("CROSS=") {
            if let Ok(n) = n_str.parse::<u32>() {
                crossing = CrossingType::Cross(n.max(1));
            }
        } else if let Some(v_str) = upper.strip_prefix("FROM=") {
            from = parse_si_value(v_str);
        } else if let Some(v_str) = upper.strip_prefix("TO=") {
            to = parse_si_value(v_str);
        }
    }

    Some(CrossingSpec {
        signal,
        threshold,
        crossing,
        from,
        to,
    })
}

/// Evaluate a WHEN measurement: find the sweep value where a signal crosses
/// a threshold.
///
/// `WHEN v(out)=0.5 [RISE=n|FALL=n|CROSS=n] [FROM=x] [TO=y]`
fn eval_when(tokens: &[String], plot: &SimPlot) -> Option<f64> {
    eval_when_inner(tokens, plot)
}

/// Inner implementation for WHEN — also used by FIND WHEN and TRIG/TARG.
fn eval_when_inner(tokens: &[String], plot: &SimPlot) -> Option<f64> {
    let spec = parse_crossing_spec(tokens)?;
    let sweep = plot.vecs.first()?;
    let sweep_data = sweep.data.as_real();

    let signal_vec = plot.vector(spec.signal)?;
    let signal_data = signal_vec.data.as_real();

    if signal_data.len() != sweep_data.len() || signal_data.len() < 2 {
        return None;
    }

    // Build the difference signal: signal - threshold.
    let diff: Vec<f64> = match spec.threshold {
        Threshold::Constant(val) => signal_data.iter().map(|&s| s - val).collect(),
        Threshold::Vector(ref_name) => {
            let ref_vec = plot.vector(ref_name)?;
            let ref_data = ref_vec.data.as_real();
            if ref_data.len() != signal_data.len() {
                return None;
            }
            signal_data
                .iter()
                .zip(ref_data.iter())
                .map(|(&s, &r)| s - r)
                .collect()
        }
    };

    find_crossing(sweep_data, &diff, spec.crossing, spec.from, spec.to)
}

/// Find the sweep value where `diff` crosses zero, matching the requested
/// crossing type and occurrence count.
fn find_crossing(
    sweep: &[f64],
    diff: &[f64],
    crossing_type: CrossingType,
    from: Option<f64>,
    to: Option<f64>,
) -> Option<f64> {
    let target_count = crossing_type.count();
    let mut count = 0u32;

    for i in 1..diff.len() {
        let x0 = sweep[i - 1];
        let x1 = sweep[i];

        // Skip intervals entirely before FROM or after TO.
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

        // Check for zero crossing in this interval.
        let d0 = diff[i - 1];
        let d1 = diff[i];

        // No crossing if same sign (and neither is exactly zero at a boundary).
        if d0 * d1 > 0.0 {
            continue;
        }
        // Both exactly zero — skip (not a crossing).
        if d0 == 0.0 && d1 == 0.0 {
            continue;
        }

        let is_rising = d0 < 0.0 && d1 >= 0.0 || d0 <= 0.0 && d1 > 0.0;
        let is_falling = d0 > 0.0 && d1 <= 0.0 || d0 >= 0.0 && d1 < 0.0;

        let matches = match crossing_type {
            CrossingType::Cross(_) => is_rising || is_falling,
            CrossingType::Rise(_) => is_rising,
            CrossingType::Fall(_) => is_falling,
        };

        if !matches {
            continue;
        }

        // Interpolate the exact crossing point.
        let crossing_x = if (d1 - d0).abs() < 1e-30 {
            x0
        } else {
            let frac = -d0 / (d1 - d0);
            x0 + frac * (x1 - x0)
        };

        // Verify interpolated point is within FROM/TO bounds.
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
        if count == target_count {
            return Some(crossing_x);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// TRIG/TARG measurement — delay between two crossings
// ---------------------------------------------------------------------------

/// A parsed TRIG or TARG clause.
struct TrigTarg<'a> {
    /// `None` for `AT=value` form.
    signal: Option<&'a str>,
    val: f64,
    crossing: CrossingType,
}

/// Parse a TRIG or TARG clause from tokens.
///
/// Forms:
/// - `v(in) VAL=0.5 RISE=1` — signal crossing
/// - `AT=1n` — fixed sweep point
fn parse_trig_targ<'a>(tokens: &'a [String]) -> Option<(TrigTarg<'a>, usize)> {
    if tokens.is_empty() {
        return None;
    }

    let upper0 = tokens[0].to_uppercase();

    // AT= form (fixed time).
    if let Some(v_str) = upper0.strip_prefix("AT=") {
        let val = parse_si_value(v_str)?;
        return Some((
            TrigTarg {
                signal: None,
                val,
                crossing: CrossingType::Cross(1),
            },
            1,
        ));
    }

    // Signal form: signal_name VAL=x [RISE=n|FALL=n|CROSS=n]
    let signal = tokens[0].as_str();
    let mut val = None;
    let mut crossing = CrossingType::Cross(1);
    let mut consumed = 1;

    for token in &tokens[1..] {
        let upper = token.to_uppercase();
        if upper == "TARG" {
            // Hit the next clause — stop.
            break;
        }
        consumed += 1;
        if let Some(v_str) = upper.strip_prefix("VAL=") {
            val = parse_si_value(v_str);
        } else if let Some(n_str) = upper.strip_prefix("RISE=") {
            if let Ok(n) = n_str.parse::<u32>() {
                crossing = CrossingType::Rise(n.max(1));
            }
        } else if let Some(n_str) = upper.strip_prefix("FALL=") {
            if let Ok(n) = n_str.parse::<u32>() {
                crossing = CrossingType::Fall(n.max(1));
            }
        } else if let Some(n_str) = upper.strip_prefix("CROSS=") {
            if let Ok(n) = n_str.parse::<u32>() {
                crossing = CrossingType::Cross(n.max(1));
            }
        } else if upper.starts_with("TD=") {
            // TD (trigger delay) — ignored for now, consumed.
        }
    }

    Some((
        TrigTarg {
            signal: Some(signal),
            val: val?,
            crossing,
        },
        consumed,
    ))
}

/// Resolve a TRIG or TARG clause to a sweep value.
fn resolve_trig_targ(tt: &TrigTarg<'_>, plot: &SimPlot) -> Option<f64> {
    match tt.signal {
        None => Some(tt.val), // AT= form
        Some(sig_name) => {
            let sweep = plot.vecs.first()?;
            let sweep_data = sweep.data.as_real();
            let sig_vec = plot.vector(sig_name)?;
            let sig_data = sig_vec.data.as_real();

            if sig_data.len() != sweep_data.len() || sig_data.len() < 2 {
                return None;
            }

            let diff: Vec<f64> = sig_data.iter().map(|&s| s - tt.val).collect();
            find_crossing(sweep_data, &diff, tt.crossing, None, None)
        }
    }
}

/// Evaluate a TRIG/TARG delay measurement.
///
/// `TRIG v(in) VAL=0.5 RISE=1 TARG v(out) VAL=0.5 RISE=1`
/// `TRIG AT=1n TARG v(out) VAL=0.5 RISE=1`
///
/// Returns TARG_time - TRIG_time.
fn eval_trig_targ(tokens: &[String], plot: &SimPlot) -> Option<f64> {
    // tokens[0..] is after the initial "TRIG" keyword (already consumed).
    let (trig, trig_consumed) = parse_trig_targ(tokens)?;

    // Find "TARG" keyword in remaining tokens.
    let rest = &tokens[trig_consumed..];
    let targ_pos = rest.iter().position(|t| t.to_uppercase() == "TARG")?;
    let targ_tokens = &rest[targ_pos + 1..];
    let (targ, _) = parse_trig_targ(targ_tokens)?;

    let trig_val = resolve_trig_targ(&trig, plot)?;
    let targ_val = resolve_trig_targ(&targ, plot)?;

    Some(targ_val - trig_val)
}

// ---------------------------------------------------------------------------
// DERIV measurement — derivative at a point
// ---------------------------------------------------------------------------

/// Evaluate DERIV measurement.
///
/// `DERIV v(out) AT=5u` — numerical derivative at a sweep point.
/// `DERIV v(out) WHEN v(clk)=0.5 RISE=1` — derivative at a crossing point.
fn eval_deriv(tokens: &[String], plot: &SimPlot) -> Option<f64> {
    if tokens.is_empty() {
        return None;
    }

    let vec_name = &tokens[0];
    let vec = plot.vector(vec_name)?;
    let data = vec.data.as_real();
    let sweep = plot.vecs.first()?;
    let sweep_data = sweep.data.as_real();

    if data.len() != sweep_data.len() || data.len() < 2 {
        return None;
    }

    let rest = &tokens[1..];

    // Find the sweep point (AT= or WHEN).
    let at_val = if let Some(at_token) = rest.iter().find(|t| t.to_uppercase().starts_with("AT=")) {
        parse_si_value(at_token.to_uppercase().strip_prefix("AT=").unwrap_or(""))?
    } else if let Some(when_pos) = rest.iter().position(|t| t.to_uppercase() == "WHEN") {
        eval_when_inner(&rest[when_pos + 1..], plot)?
    } else {
        return None;
    };

    // Find the interval and compute numerical derivative.
    for i in 1..sweep_data.len() {
        if (sweep_data[i - 1] <= at_val && sweep_data[i] >= at_val)
            || (sweep_data[i - 1] >= at_val && sweep_data[i] <= at_val)
        {
            let dx = sweep_data[i] - sweep_data[i - 1];
            if dx.abs() < 1e-30 {
                return None;
            }
            return Some((data[i] - data[i - 1]) / dx);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tokenizer and SI value parser
// ---------------------------------------------------------------------------

/// Tokenize a measurement spec string, respecting parenthesized expressions.
///
/// `"MAX v(out) FROM=1u TO=5u"` → `["MAX", "v(out)", "FROM=1u", "TO=5u"]`
fn tokenize_meas_spec(spec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut paren_depth = 0u32;

    for ch in spec.chars() {
        match ch {
            '(' => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(ch);
            }
            ' ' | '\t' if paren_depth == 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Parse a numeric value with optional SI suffix.
fn parse_si_value(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Try direct parse first.
    if let Ok(v) = s.parse::<f64>() {
        return Some(v);
    }

    // Strip SI suffix (longest match first to handle "meg" before "m").
    let suffixes: &[(&str, f64)] = &[
        ("meg", 1e6),
        ("t", 1e12),
        ("g", 1e9),
        ("k", 1e3),
        ("m", 1e-3),
        ("u", 1e-6),
        ("n", 1e-9),
        ("p", 1e-12),
        ("f", 1e-15),
        ("a", 1e-18),
    ];

    let lower = s.to_lowercase();
    for &(suffix, mult) in suffixes {
        if let Some(num_str) = lower.strip_suffix(suffix)
            && let Ok(v) = num_str.parse::<f64>()
        {
            return Some(v * mult);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a transient plot with a time sweep and signal vectors.
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

    fn meas(spec: &str) -> MeasureSpec {
        MeasureSpec {
            name: "test_meas".to_string(),
            analysis_type: "tran".to_string(),
            spec: spec.to_string(),
        }
    }

    // -- Tokenizer -----------------------------------------------------------

    #[test]
    fn tokenize_simple() {
        let tokens = tokenize_meas_spec("MAX v(out)");
        assert_eq!(tokens, vec!["MAX", "v(out)"]);
    }

    #[test]
    fn tokenize_with_range() {
        let tokens = tokenize_meas_spec("MIN v(out) FROM=1u TO=5u");
        assert_eq!(tokens, vec!["MIN", "v(out)", "FROM=1u", "TO=5u"]);
    }

    #[test]
    fn tokenize_trig_targ() {
        let tokens = tokenize_meas_spec("TRIG v(in) VAL=0.5 RISE=1 TARG v(out) VAL=0.5 RISE=1");
        assert_eq!(
            tokens,
            vec![
                "TRIG", "v(in)", "VAL=0.5", "RISE=1", "TARG", "v(out)", "VAL=0.5", "RISE=1"
            ]
        );
    }

    #[test]
    fn tokenize_when_with_equals() {
        let tokens = tokenize_meas_spec("WHEN v(out)=0.5 RISE=2");
        assert_eq!(tokens, vec!["WHEN", "v(out)=0.5", "RISE=2"]);
    }

    // -- SI parser -----------------------------------------------------------

    #[test]
    fn parse_si_values() {
        assert!((parse_si_value("5u").unwrap() - 5e-6).abs() < 1e-18);
        assert!((parse_si_value("1k").unwrap() - 1e3).abs() < 1e-9);
        assert!((parse_si_value("2.5meg").unwrap() - 2.5e6).abs() < 1.0);
        assert!((parse_si_value("100n").unwrap() - 100e-9).abs() < 1e-18);
        assert!((parse_si_value("3.3").unwrap() - 3.3).abs() < 1e-15);
    }

    // -- MAX / MIN / AVG / RMS / PP ------------------------------------------

    #[test]
    fn max_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![1.0, 3.0, 2.0, 0.5])],
        );
        let val = evaluate_single_measurement(&meas("MAX v(out)"), &plot).unwrap();
        assert!((val - 3.0).abs() < 1e-12);
    }

    #[test]
    fn min_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![1.0, 3.0, 2.0, 0.5])],
        );
        let val = evaluate_single_measurement(&meas("MIN v(out)"), &plot).unwrap();
        assert!((val - 0.5).abs() < 1e-12);
    }

    #[test]
    fn pp_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![1.0, 3.0, 2.0, 0.5])],
        );
        let val = evaluate_single_measurement(&meas("PP v(out)"), &plot).unwrap();
        assert!((val - 2.5).abs() < 1e-12); // 3.0 - 0.5
    }

    #[test]
    fn avg_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![2.0, 2.0, 2.0, 2.0])],
        );
        let val = evaluate_single_measurement(&meas("AVG v(out)"), &plot).unwrap();
        assert!((val - 2.0).abs() < 1e-12);
    }

    #[test]
    fn max_with_from_to_range() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![("v(out)", vec![10.0, 1.0, 5.0, 2.0, 8.0])],
        );
        // MAX in [1.0, 3.0] range → should be 5.0 (at t=2.0)
        let val = evaluate_single_measurement(&meas("MAX v(out) FROM=1.0 TO=3.0"), &plot).unwrap();
        assert!((val - 5.0).abs() < 1e-12);
    }

    // -- FIND AT -------------------------------------------------------------

    #[test]
    fn find_at_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![0.0, 2.0, 4.0, 6.0])],
        );
        let val = evaluate_single_measurement(&meas("FIND v(out) AT=1.5"), &plot).unwrap();
        assert!((val - 3.0).abs() < 1e-12);
    }

    // -- INTEG ---------------------------------------------------------------

    #[test]
    fn integral_measurement() {
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("i(r1)", vec![2.0, 2.0, 2.0, 2.0])],
        );
        let val = evaluate_single_measurement(&meas("INTEG i(r1)"), &plot).unwrap();
        assert!((val - 6.0).abs() < 1e-12);
    }

    // -- WHEN ----------------------------------------------------------------

    #[test]
    fn when_rising_crossing() {
        // Signal ramps from 0 to 4 over t=0..4.
        // Crosses 2.0 at t=2.0.
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![("v(out)", vec![0.0, 1.0, 2.0, 3.0, 4.0])],
        );
        let val = evaluate_single_measurement(&meas("WHEN v(out)=2.0"), &plot).unwrap();
        assert!((val - 2.0).abs() < 1e-12);
    }

    #[test]
    fn when_interpolated_crossing() {
        // Signal: 0, 0, 1 at t=0, 1, 2.
        // Crosses 0.5 between t=1 and t=2 → t=1.5.
        let plot = tran_plot(vec![0.0, 1.0, 2.0], vec![("v(out)", vec![0.0, 0.0, 1.0])]);
        let val = evaluate_single_measurement(&meas("WHEN v(out)=0.5"), &plot).unwrap();
        assert!((val - 1.5).abs() < 1e-12);
    }

    #[test]
    fn when_rise_2() {
        // Signal oscillates: 0, 1, 0, 1, 0
        // Crosses 0.5 rising at t=0.5 (1st), t=2.5 (2nd).
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![("v(out)", vec![0.0, 1.0, 0.0, 1.0, 0.0])],
        );
        let val = evaluate_single_measurement(&meas("WHEN v(out)=0.5 RISE=2"), &plot).unwrap();
        assert!((val - 2.5).abs() < 1e-12);
    }

    #[test]
    fn when_fall_1() {
        // Signal: 1, 0, 1, 0
        // Falls through 0.5 at t=0.5 (1st fall).
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![1.0, 0.0, 1.0, 0.0])],
        );
        let val = evaluate_single_measurement(&meas("WHEN v(out)=0.5 FALL=1"), &plot).unwrap();
        assert!((val - 0.5).abs() < 1e-12);
    }

    #[test]
    fn when_cross_3() {
        // Signal: 0, 1, 0, 1, 0
        // Crosses 0.5: rise@0.5, fall@1.5, rise@2.5, fall@3.5
        // CROSS=3 → 3rd crossing at t=2.5
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![("v(out)", vec![0.0, 1.0, 0.0, 1.0, 0.0])],
        );
        let val = evaluate_single_measurement(&meas("WHEN v(out)=0.5 CROSS=3"), &plot).unwrap();
        assert!((val - 2.5).abs() < 1e-12);
    }

    #[test]
    fn when_two_signals_cross() {
        // v(a) ramps up, v(b) ramps down. They cross at t=2.0.
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![
                ("v(a)", vec![0.0, 1.0, 2.0, 3.0, 4.0]),
                ("v(b)", vec![4.0, 3.0, 2.0, 1.0, 0.0]),
            ],
        );
        let val = evaluate_single_measurement(&meas("WHEN v(a)=v(b)"), &plot).unwrap();
        assert!((val - 2.0).abs() < 1e-12);
    }

    #[test]
    fn when_with_from_to() {
        // Signal: 0, 1, 0, 1, 0
        // Crossings at 0.5, 1.5, 2.5, 3.5
        // With FROM=2.0, first visible crossing is at 2.5
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![("v(out)", vec![0.0, 1.0, 0.0, 1.0, 0.0])],
        );
        let val =
            evaluate_single_measurement(&meas("WHEN v(out)=0.5 CROSS=1 FROM=2.0"), &plot).unwrap();
        assert!((val - 2.5).abs() < 1e-12);
    }

    // -- FIND WHEN -----------------------------------------------------------

    #[test]
    fn find_when_crossing() {
        // v(clk) crosses 0.5 rising at t=0.5.
        // v(data) at t=0.5 = interpolated between 10 and 20 → 15.
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0],
            vec![
                ("v(clk)", vec![0.0, 1.0, 0.0]),
                ("v(data)", vec![10.0, 20.0, 30.0]),
            ],
        );
        let val = evaluate_single_measurement(&meas("FIND v(data) WHEN v(clk)=0.5 RISE=1"), &plot)
            .unwrap();
        assert!((val - 15.0).abs() < 1e-12);
    }

    // -- TRIG / TARG ---------------------------------------------------------

    #[test]
    fn trig_targ_signal_to_signal() {
        // v(in) crosses 0.5 rising at t=0.5.
        // v(out) crosses 0.5 rising at t=1.5.
        // Delay = 1.5 - 0.5 = 1.0
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![
                ("v(in)", vec![0.0, 1.0, 1.0, 1.0]),
                ("v(out)", vec![0.0, 0.0, 1.0, 1.0]),
            ],
        );
        let val = evaluate_single_measurement(
            &meas("TRIG v(in) VAL=0.5 RISE=1 TARG v(out) VAL=0.5 RISE=1"),
            &plot,
        )
        .unwrap();
        assert!((val - 1.0).abs() < 1e-12);
    }

    #[test]
    fn trig_at_targ_signal() {
        // TRIG AT=1.0, v(out) crosses 0.5 rising at t=2.5.
        // Delay = 2.5 - 1.0 = 1.5
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![0.0, 0.0, 0.0, 1.0])],
        );
        let val =
            evaluate_single_measurement(&meas("TRIG AT=1.0 TARG v(out) VAL=0.5 RISE=1"), &plot)
                .unwrap();
        assert!((val - 1.5).abs() < 1e-12);
    }

    #[test]
    fn trig_targ_fall() {
        // v(in) falls through 0.5 at t=0.5 (starts at 1, drops to 0).
        // v(out) falls through 0.5 at t=2.5 (starts at 1, stays, drops).
        // Delay = 2.5 - 0.5 = 2.0
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![
                ("v(in)", vec![1.0, 0.0, 0.0, 0.0]),
                ("v(out)", vec![1.0, 1.0, 1.0, 0.0]),
            ],
        );
        let val = evaluate_single_measurement(
            &meas("TRIG v(in) VAL=0.5 FALL=1 TARG v(out) VAL=0.5 FALL=1"),
            &plot,
        )
        .unwrap();
        assert!((val - 2.0).abs() < 1e-12);
    }

    // -- DERIV ---------------------------------------------------------------

    #[test]
    fn deriv_at_point() {
        // v(out) = t² approximated at integer points: 0, 1, 4, 9
        // derivative at t=1.5: between t=1 and t=2, dy/dx = (4-1)/(2-1) = 3.0
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0, 3.0],
            vec![("v(out)", vec![0.0, 1.0, 4.0, 9.0])],
        );
        let val = evaluate_single_measurement(&meas("DERIV v(out) AT=1.5"), &plot).unwrap();
        assert!((val - 3.0).abs() < 1e-12);
    }

    #[test]
    fn deriv_at_when() {
        // v(clk) crosses 0.5 at t=0.5.
        // v(out) slope in that interval (t=0..1): (2-0)/(1-0) = 2.0.
        let plot = tran_plot(
            vec![0.0, 1.0, 2.0],
            vec![
                ("v(out)", vec![0.0, 2.0, 6.0]),
                ("v(clk)", vec![0.0, 1.0, 0.0]),
            ],
        );
        let val = evaluate_single_measurement(&meas("DERIV v(out) WHEN v(clk)=0.5 RISE=1"), &plot)
            .unwrap();
        assert!((val - 2.0).abs() < 1e-12);
    }

    // -- Edge cases ----------------------------------------------------------

    #[test]
    fn when_no_crossing_returns_none() {
        let plot = tran_plot(vec![0.0, 1.0, 2.0], vec![("v(out)", vec![0.0, 0.1, 0.2])]);
        let val = evaluate_single_measurement(&meas("WHEN v(out)=5.0"), &plot);
        assert!(val.is_none());
    }

    #[test]
    fn trig_targ_missing_targ_returns_none() {
        let plot = tran_plot(vec![0.0, 1.0], vec![("v(out)", vec![0.0, 1.0])]);
        // Missing TARG clause.
        let val = evaluate_single_measurement(&meas("TRIG v(out) VAL=0.5 RISE=1"), &plot);
        assert!(val.is_none());
    }

    #[test]
    fn unknown_keyword_returns_none() {
        let plot = tran_plot(vec![0.0, 1.0], vec![("v(out)", vec![0.0, 1.0])]);
        let val = evaluate_single_measurement(&meas("BOGUS v(out)"), &plot);
        assert!(val.is_none());
    }
}
