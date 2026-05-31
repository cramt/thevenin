//! Executor for `.control` block statements.

use thevenin::{TranOutcome, TranStartState};
use thevenin_types::{SimPlot, SimResult, SimVector};

use crate::ast::{AlterValue, EchoFragment, Statement, StopCondition};
use crate::context::SimContext;
use crate::vecexpr::{eval_condition, eval_vec_expr};
use cirq_ir::control::MAX_LOOP_ITERS;

/// Result of executing a `.control` block.
pub struct ControlResult {
    /// Merged simulation result for output formatting.
    pub sim_result: SimResult,
    /// Exit code from `quit` (0 = success).
    pub exit_code: i32,
    /// Captured output text (echo, print).
    pub output: String,
}

/// Execute a list of parsed control statements.
pub fn execute(stmts: &[Statement], ctx: &mut SimContext) -> Result<(), String> {
    for stmt in stmts {
        if ctx.exit_code.is_some() {
            return Ok(());
        }
        execute_one(stmt, ctx)?;
    }
    Ok(())
}

fn execute_one(stmt: &Statement, ctx: &mut SimContext) -> Result<(), String> {
    match stmt {
        Statement::Comment => Ok(()),

        Statement::Echo(fragments) => {
            let text = resolve_echo(fragments, ctx);
            ctx.echo(&text);
            Ok(())
        }

        Statement::Quit(code) => {
            ctx.exit_code = Some(code.unwrap_or(0));
            Ok(())
        }

        Statement::Set(pairs) => {
            for (key, val) in pairs {
                if let Some(v) = val {
                    // Resolve variable references in value
                    let resolved = interpolate_vars(v, ctx);
                    ctx.variables.insert(key.clone(), resolved);
                } else {
                    ctx.variables.insert(key.clone(), String::new());
                }
            }
            Ok(())
        }

        Statement::Setplot(name) => {
            let resolved = interpolate_vars(name, ctx);
            ctx.set_current_plot(&resolved);
            Ok(())
        }

        Statement::Define { name, args, body } => {
            ctx.functions
                .insert(name.to_lowercase(), (args.clone(), body.clone()));
            Ok(())
        }

        Statement::Let { name, expr } => {
            let resolved_expr = interpolate_vars(expr, ctx);
            let val = eval_vec_expr(&resolved_expr, ctx)?;
            let complex = if val.imag.is_empty() {
                Vec::new()
            } else {
                let len = val.data.len().max(val.imag.len());
                (0..len)
                    .map(|i| {
                        let re = val.data.get(i).copied().unwrap_or(0.0);
                        let im = val.imag.get(i).copied().unwrap_or(0.0);
                        thevenin_types::Complex { re, im }
                    })
                    .collect()
            };
            let vec = if complex.is_empty() {
                SimVector::real(name.clone(), val.data)
            } else {
                SimVector::complex(name.clone(), complex)
            };
            ctx.store_vector(vec);
            Ok(())
        }

        Statement::Compose { name, value_exprs } => {
            let mut values = Vec::new();
            for expr_str in value_exprs {
                let resolved = interpolate_vars(expr_str, ctx);
                let val = eval_vec_expr(&resolved, ctx)?;
                // compose creates a vector from scalar values
                values.push(val.as_scalar());
            }
            let vec = SimVector::real(name.clone(), values);
            ctx.store_vector(vec);
            Ok(())
        }

        Statement::If {
            cond,
            body,
            else_body,
        } => {
            let resolved_cond = interpolate_vars(cond, ctx);
            let result = eval_condition(&resolved_cond, ctx)?;
            if result {
                execute(body, ctx)?;
            } else {
                execute(else_body, ctx)?;
            }
            Ok(())
        }

        Statement::Foreach { var, values, body } => {
            for val in values {
                let resolved = interpolate_vars(val, ctx);
                ctx.variables.insert(var.clone(), resolved);
                execute(body, ctx)?;
                if ctx.exit_code.is_some() {
                    return Ok(());
                }
            }
            Ok(())
        }

        Statement::While { cond, body } => {
            let mut iters = 0usize;
            loop {
                let resolved = interpolate_vars(cond, ctx);
                if !eval_condition(&resolved, ctx)? {
                    break;
                }
                execute(body, ctx)?;
                if ctx.exit_code.is_some() {
                    return Ok(());
                }
                iters += 1;
                if iters >= MAX_LOOP_ITERS {
                    return Err(format!(
                        "while: exceeded MAX_LOOP_ITERS ({MAX_LOOP_ITERS}) — runaway loop?"
                    ));
                }
            }
            Ok(())
        }

        Statement::Repeat { count, body } => {
            // Evaluate count once at entry, matching ngspice's semantics.
            let resolved = interpolate_vars(count, ctx);
            let val = eval_vec_expr(&resolved, ctx)?;
            let n_raw = val.as_scalar();
            // Truncate toward zero so non-integer expressions round
            // predictably; n <= 0 ⇒ zero iterations.
            let n = if n_raw <= 0.0 { 0 } else { n_raw as usize };
            let n_capped = n.min(MAX_LOOP_ITERS);
            if n > MAX_LOOP_ITERS {
                return Err(format!(
                    "repeat: count {n} exceeds MAX_LOOP_ITERS ({MAX_LOOP_ITERS})"
                ));
            }
            for _ in 0..n_capped {
                execute(body, ctx)?;
                if ctx.exit_code.is_some() {
                    return Ok(());
                }
            }
            Ok(())
        }

        Statement::Save { specs } => {
            // Append to the driving circuit's recording set so the next
            // analysis run (op/dc/tran/...) honours the additions. Dedupe
            // so repeated `save v(out)` lines don't bloat the list.
            if let Some(circuit) = ctx.circuit.as_mut() {
                for spec in specs {
                    if !circuit.save.iter().any(|existing| existing == spec) {
                        circuit.save.push(spec.clone());
                    }
                }
            }
            Ok(())
        }

        Statement::Strcmp { result, a, b } => {
            let a_val = interpolate_vars(a, ctx);
            let b_val = interpolate_vars(b, ctx);
            let cmp = if a_val == b_val { 0.0 } else { 1.0 };
            // Store as a variable (ngspice uses set, not let, for strcmp result)
            ctx.variables.insert(result.clone(), cmp.to_string());
            // Also store as a vector for expression access
            let vec = SimVector::real(format!("__{result}"), vec![cmp]);
            ctx.store_vector(vec);
            Ok(())
        }

        Statement::Print { exprs, file: _ } => {
            let mut parts = Vec::new();
            for expr_str in exprs {
                let resolved = interpolate_vars(expr_str, ctx);
                match eval_vec_expr(&resolved, ctx) {
                    Ok(val) => {
                        if val.data.len() == 1 {
                            parts.push(format_print_value(val.data[0]));
                        } else {
                            for (i, v) in val.data.iter().enumerate() {
                                parts.push(format!("[{}] = {}", i, format_print_value(*v)));
                            }
                        }
                    }
                    Err(e) => parts.push(format!("(error: {e})")),
                }
            }
            ctx.echo(&parts.join(" "));
            Ok(())
        }

        Statement::RunAnalysis(cmd_line) => run_analysis(cmd_line, ctx),

        Statement::Write { file, vectors } => execute_write(file.as_deref(), vectors, ctx),

        Statement::Alter { spec, value } => execute_alter(spec, value, ctx),

        Statement::Eprint(_) => {
            // Not implemented — skip silently
            Ok(())
        }

        Statement::StopWhen(cond) => {
            ctx.stop_when = Some(cond.clone());
            Ok(())
        }

        Statement::Resume => execute_resume(ctx),

        Statement::Source { path } => execute_source(path, ctx),

        Statement::Measure {
            name,
            analysis_type,
            spec,
        } => execute_measure(name, analysis_type, spec, ctx),
    }
}

/// Run a simulation command (op, dc, ac, tran, sens, noise, pz, tf).
///
/// Routes through the Circuit-input simulator surface
/// ([`thevenin::circuit::simulate_*`]) by parsing the command tokens straight
/// into [`cirq_ir::Analysis`] via
/// [`cirq_frontend::control_analysis::parse_analysis_to_ir`]. TEMPER eval and
/// `@model[param]` resolution both operate on the IR Circuit directly — no
/// `thevenin_types::Analysis` or lowered Netlist is constructed.
fn run_analysis(cmd_line: &str, ctx: &mut SimContext) -> Result<(), String> {
    let parts: Vec<&str> = cmd_line.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(());
    }
    let cmd = parts[0].to_lowercase();

    // Special case: `dc temp start stop step` — temperature sweep
    if cmd == "dc" && parts.len() >= 5 && parts[1].eq_ignore_ascii_case("temp") {
        return run_temp_sweep(parts[2], parts[3], parts[4], ctx);
    }

    let circuit_ref = ctx
        .circuit()
        .ok_or_else(|| format!("{cmd}: no circuit attached to context"))?;

    // `run` re-executes the first analysis declared on the circuit; every
    // other command is parsed fresh and lifted to IR shape against the
    // current circuit (so source / net references resolve to Ids).
    let ir_analysis = if cmd == "run" {
        circuit_ref
            .analyses
            .first()
            .cloned()
            .unwrap_or(cirq_ir::Analysis::Op)
    } else {
        cirq_frontend::control_analysis::parse_analysis_to_ir(&cmd, &parts[1..], circuit_ref)
            .map_err(|e| format!("{cmd}: {e}"))?
    };

    // Build a working clone with only the requested analysis selected, then
    // apply TEMPER on the IR shape before dispatch.
    let mut circuit = circuit_ref.clone();
    circuit.analyses = vec![ir_analysis.clone()];
    let temp_c = thevenin::mna_ir::circuit_temp(&circuit);
    evaluate_temper_exprs_circuit(&mut circuit, temp_c);

    let result = match &ir_analysis {
        cirq_ir::Analysis::Op => {
            thevenin::circuit::simulate_op(&circuit).map_err(|e| format!("OP: {e}"))
        }
        cirq_ir::Analysis::Dc(_) => {
            thevenin::circuit::simulate_dc(&circuit).map_err(|e| format!("DC: {e}"))
        }
        cirq_ir::Analysis::Tran(_) => run_tran_with_pause(&circuit, ctx),
        cirq_ir::Analysis::Ac(_) => {
            thevenin::circuit::simulate_ac(&circuit).map_err(|e| format!("AC: {e}"))
        }
        cirq_ir::Analysis::Sens(_) => {
            thevenin::circuit::simulate_sens(&circuit).map_err(|e| format!("Sens: {e}"))
        }
        cirq_ir::Analysis::Noise(_) => {
            thevenin::circuit::simulate_noise(&circuit).map_err(|e| format!("Noise: {e}"))
        }
        cirq_ir::Analysis::Pz(_) => {
            thevenin::circuit::simulate_pz(&circuit).map_err(|e| format!("PZ: {e}"))
        }
        cirq_ir::Analysis::Tf(_) => {
            thevenin::circuit::simulate_tf(&circuit).map_err(|e| format!("TF: {e}"))
        }
        cirq_ir::Analysis::Four(four) => {
            thevenin::circuit::simulate_four(&circuit, four).map_err(|e| format!("Four: {e}"))
        }
        cirq_ir::Analysis::Fft(fft) => {
            thevenin::circuit::simulate_fft(&circuit, fft).map_err(|e| format!("FFT: {e}"))
        }
        // `Analysis` is `#[non_exhaustive]` — unknown analysis kinds
        // are surfaced as an error rather than panic.
        _ => Err("unknown analysis variant".to_string()),
    };

    // Capture post-TEMPER model params for @model[param] queries.
    for model in &circuit.models {
        ctx.resolved_models
            .insert(model.name.to_uppercase(), model.params.clone());
    }

    match result {
        Ok(sim_result) => {
            for plot in sim_result.plots {
                ctx.add_plot(plot);
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Run a `.tran` analysis with optional pause support.
///
/// If `ctx.stop_when` is set, the run honours that condition (currently only
/// `time = <value>`) and may pause partway through. The partial result is
/// always returned so the caller's plot machinery shows whatever was
/// integrated up to the pause point; the snapshot is stashed on
/// `ctx.paused_tran` for a subsequent `resume`.
///
/// The `stop_when` field is consumed (cleared) regardless of whether the run
/// actually paused, matching ngspice's one-shot semantics.
fn run_tran_with_pause(
    circuit: &cirq_ir::Circuit,
    ctx: &mut SimContext,
) -> Result<SimResult, String> {
    let stop = ctx.stop_when.take();
    let t_pause = stop.map(|StopCondition::TimeEq(t)| t);

    let mna = thevenin::mna_ir::assemble_mna_from_circuit(circuit, false, None)
        .map_err(|e| format!("Tran: {e}"))?
        .ok_or_else(|| "Tran: circuit not representable in mna_ir".to_string())?;
    let mut params = thevenin::mna_ir::tran_params_from_circuit(circuit, &mna)
        .map_err(|e| format!("Tran: {e}"))?;
    params.t_pause = t_pause;

    match thevenin::run_tran(mna, params).map_err(|e| format!("Tran: {e}"))? {
        TranOutcome::Complete(r) => {
            // A new complete run invalidates any prior pause snapshot —
            // resume only makes sense for the most recent paused tran.
            ctx.paused_tran = None;
            Ok(r)
        }
        TranOutcome::Paused { snapshot, partial } => {
            ctx.paused_tran = Some(snapshot);
            Ok(partial)
        }
    }
}

/// Execute the `resume` command: continue the most recent paused transient
/// from where it stopped, against the (possibly `alter`-mutated) current
/// netlist.
///
/// Errors if no transient is paused. Re-assembles the MNA from the working
/// netlist (which `alter` keeps in sync with `Circuit`), seeds the resumed
/// run with the snapshot's solution and accumulated output, and runs from
/// the snapshot's `t_paused` to the original `tstop`.
///
/// The resumed result replaces the paused leg's plot rather than adding a
/// new one — this matches ngspice's behaviour where `tran ... ; resume`
/// produces a single contiguous plot, not two.
fn execute_resume(ctx: &mut SimContext) -> Result<(), String> {
    let snapshot = ctx
        .paused_tran
        .take()
        .ok_or_else(|| "resume: no paused transient simulation".to_string())?;

    // Build a working circuit from the current ctx.circuit (already
    // mutated by any `alter` calls between pause and resume). Override
    // its analysis with the paused leg's Tran params so the resumed run
    // honours the original `tstep`/`tstop` even when `tran` was an
    // interpreter command (.control-only) rather than a circuit directive.
    let mut circuit = ctx
        .circuit()
        .ok_or_else(|| "resume: no circuit attached to context".to_string())?
        .clone();
    circuit.analyses = vec![cirq_ir::Analysis::Tran(cirq_ir::TranAnalysis {
        step: snapshot.t_step,
        stop: snapshot.t_stop,
        start: 0.0,
        tmax: snapshot.t_max,
        // The original paused leg may have been uic; the resumed leg
        // does not re-apply uic — it starts from the snapshot's solution
        // via `start_state` and so its IC/uic handling is short-circuited
        // inside run_tran (see the `start_state.is_none()` guard).
        uic: false,
    })];
    let temp_c = thevenin::mna_ir::circuit_temp(&circuit);
    evaluate_temper_exprs_circuit(&mut circuit, temp_c);

    let mna = thevenin::mna_ir::assemble_mna_from_circuit(&circuit, false, None)
        .map_err(|e| format!("resume: {e}"))?
        .ok_or_else(|| "resume: circuit not representable in mna_ir".to_string())?;
    let mut params = thevenin::mna_ir::tran_params_from_circuit(&circuit, &mna)
        .map_err(|e| format!("resume: {e}"))?;
    params.start_state = Some(TranStartState {
        t_initial: snapshot.t_paused,
        solution: snapshot.solution,
        output_vecs: snapshot.output_vecs,
    });

    let outcome = thevenin::run_tran(mna, params).map_err(|e| format!("resume: {e}"))?;
    let resumed = match outcome {
        TranOutcome::Complete(r) => r,
        // A second `stop when` between resume and the new tstop is not
        // supported today — the snapshot would replace the just-restored one
        // and the interpreter would re-enter resume against a moving target.
        // ngspice supports this but resume-1.cir does not exercise it.
        TranOutcome::Paused { partial, snapshot } => {
            ctx.paused_tran = Some(snapshot);
            partial
        }
    };

    // Replace the paused leg's plot in-place so the interpreter sees a
    // single contiguous tran plot. The paused leg was added as "tran<N>";
    // overwrite its vecs while keeping the name + plot index stable.
    let resumed_vecs = resumed
        .plots
        .into_iter()
        .next()
        .map(|p| p.vecs)
        .unwrap_or_default();
    if let Some(idx) = ctx.current_plot
        && let Some(plot) = ctx.plots.get_mut(idx)
    {
        plot.vecs = resumed_vecs;
    } else {
        // No current plot (shouldn't happen in practice — the paused tran
        // always created one) — fall back to adding the resumed leg as a
        // fresh plot.
        ctx.add_plot(SimPlot {
            name: "tran".to_string(),
            vecs: resumed_vecs,
        });
    }
    Ok(())
}

/// Execute the `source <path>` control command.
///
/// Reads the file relative to the current working directory, parses it as
/// a `.control` block, and executes the statements in the *same* context
/// (variables, plots, and circuit are shared with the caller). Errors at
/// file I/O, parse, or sub-execution level all propagate up.
///
/// Recursion guard: the canonicalised path is pushed onto `ctx.sourcing`
/// before parsing the sub-script and popped afterward. Re-entering a
/// script that is already higher up the stack errors cleanly instead of
/// recursing until the OS stack overflows.
fn execute_source(path: &str, ctx: &mut SimContext) -> Result<(), String> {
    use std::path::PathBuf;

    let resolved = std::path::Path::new(path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(path));

    if ctx.sourcing.contains(&resolved) {
        return Err(format!(
            "source: recursive include of {} (already sourcing)",
            resolved.display()
        ));
    }

    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("source: cannot read '{path}': {e}"))?;
    let lines: Vec<String> = contents.lines().map(|l| l.to_string()).collect();
    let stmts = cirq_ir::control::parse_control_block(&lines)
        .map_err(|e| format!("source '{path}': parse error: {e}"))?;

    ctx.sourcing.insert(resolved.clone());
    let result = execute(&stmts, ctx);
    ctx.sourcing.remove(&resolved);
    result
}

/// Execute the `measure <kind> <name> <spec>` control command.
///
/// Parses the spec into a typed [`cirq_ir::MeasureSpec`] and evaluates it
/// against the simulation results so far. The result is appended to the
/// `measurements` plot (created if absent) and bound as a control variable
/// named after the measurement so subsequent expressions can reference it.
///
/// Unlike the `.meas` directive — which runs once at end-of-simulation —
/// this command operates on whatever plots have been produced up to the
/// point it is called, matching ngspice's interactive `measure` command.
fn execute_measure(
    name: &str,
    analysis_type: &str,
    spec: &str,
    ctx: &mut SimContext,
) -> Result<(), String> {
    let parsed = cirq_ir::MeasureSpec::parse(name, analysis_type, spec);
    if parsed.expr.is_none() {
        return Err(format!("measure '{name}': cannot parse spec '{spec}'"));
    }

    // Run the evaluator over a freshly-built SimResult that shares our
    // current plots. The evaluator appends a `measurements` plot on
    // success, leaving the underlying plots untouched.
    let mut tmp = SimResult {
        plots: std::mem::take(&mut ctx.plots),
    };
    let saved_current = ctx.current_plot;
    thevenin::evaluate_circuit_measures(std::slice::from_ref(&parsed), &mut tmp);
    // Restore plots (evaluate may have pushed a fresh `measurements` plot).
    ctx.plots = tmp.plots;
    ctx.current_plot = saved_current;

    // Locate the measurement we just produced and bind it as a control
    // variable + named user vector so `$<name>` / `print <name>` work.
    let value = ctx
        .plots
        .iter()
        .find(|p| p.name == "measurements")
        .and_then(|p| {
            p.vecs
                .iter()
                .find(|v| v.name.eq_ignore_ascii_case(name))
                .and_then(|v| v.data.as_real().first().copied())
        });

    if let Some(v) = value {
        ctx.variables.insert(name.to_string(), v.to_string());
        // Stash as a named vector too so `let x = <name>` resolves.
        ctx.store_vector(SimVector::real(name.to_string(), vec![v]));
        Ok(())
    } else {
        Err(format!(
            "measure '{name}': evaluation produced no result (no matching {analysis_type} plot?)"
        ))
    }
}

/// Execute the `write` control command. Mirrors ngspice's behaviour:
/// no filename ⇒ `thevenin.raw`, no vector list ⇒ everything in the
/// current plot. Format is `.csv` → CSV, anything else → ngspice raw.
/// Within raw, the `filetype` variable (`ascii` / `binary`) chooses ASCII
/// vs binary; default is binary, matching ngspice.
///
/// Vector filtering only applies inside the current plot — other plots
/// in the `SimResult` are emitted in full so multi-analysis sessions
/// produce a faithful multi-plot raw file.
fn execute_write(
    file: Option<&str>,
    vectors: &[String],
    ctx: &mut SimContext,
) -> Result<(), String> {
    let filename = file
        .map(|f| interpolate_vars(f, ctx))
        .unwrap_or_else(|| "thevenin.raw".to_string());

    // Build a SimResult holding the plots we want to write. By default
    // that's `ctx.plots` verbatim. When the user specified a vector list,
    // filter the *current* plot down to those vectors (matching the
    // ngspice command's behaviour); leave other plots intact.
    let title = ctx
        .circuit()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "thevenin".to_string());

    let mut plots: Vec<SimPlot> = ctx.plots.clone();
    if !vectors.is_empty()
        && let Some(idx) = ctx.current_plot
        && let Some(plot) = plots.get_mut(idx)
    {
        let wanted: Vec<String> = vectors.iter().map(|v| v.to_lowercase()).collect();
        plot.vecs.retain(|v| {
            let lower = v.name.to_lowercase();
            // Match either the literal name or an `i(<x>)` against
            // `<x>#branch`.
            wanted.iter().any(|w| {
                if &lower == w {
                    return true;
                }
                if let Some(stripped) = w.strip_prefix("i(").and_then(|s| s.strip_suffix(')'))
                    && lower == format!("{stripped}#branch")
                {
                    return true;
                }
                false
            })
        });
    }

    let result = SimResult { plots };

    // Pick format by filename extension, then by `filetype` set var.
    let lower = filename.to_lowercase();
    let csv = lower.ends_with(".csv");
    let ascii_raw = !csv
        && ctx
            .variables
            .get("filetype")
            .map(|s| s.eq_ignore_ascii_case("ascii"))
            .unwrap_or(false);

    let mut file = std::fs::File::create(&filename)
        .map_err(|e| format!("write: cannot create '{filename}': {e}"))?;
    let write_result = if csv {
        thevenin::raw_output::write_csv(&mut file, &result)
    } else if ascii_raw {
        thevenin::raw_output::write_ascii_raw(&mut file, &result, &title)
    } else {
        thevenin::raw_output::write_binary_raw(&mut file, &result, &title)
    };
    write_result.map_err(|e| format!("write: I/O error on '{filename}': {e}"))?;
    Ok(())
}

/// Parse a SPICE number token, stripping any trailing non-SI unit designator
/// first (so `27C` / `5V` resolve like the analysis-command parser).
fn parse_num(s: &str) -> Result<f64, String> {
    let s =
        s.trim_end_matches(|c: char| c.is_ascii_alphabetic() && !"tTgGkKmMuUnNpPfFaA".contains(c));
    cirq_ir::control::parse_spice_number(s)
}

/// Run a DC temperature sweep: `dc temp start stop step`.
///
/// At each temperature point, sets `circuit.temps`, re-evaluates
/// TEMPER-dependent expressions on a Circuit clone, and solves the operating
/// point through the Circuit-input dispatcher.
fn run_temp_sweep(
    start_s: &str,
    stop_s: &str,
    step_s: &str,
    ctx: &mut SimContext,
) -> Result<(), String> {
    let start = parse_num(start_s)?;
    let stop = parse_num(stop_s)?;
    let step = parse_num(step_s)?;

    // Generate sweep points
    let mut temps = Vec::new();
    let mut t = start;
    if step > 0.0 {
        while t <= stop + step * 1e-9 {
            temps.push(t);
            t += step;
        }
    } else if step < 0.0 {
        while t >= stop + step * 1e-9 {
            temps.push(t);
            t += step;
        }
    } else {
        temps.push(start);
    }

    // Build result vectors: temp-sweep + node voltages + branch currents
    // We need to run the first point to discover the node/branch names
    let mut vecs: Vec<SimVector> = Vec::new();
    let mut first = true;

    for &temp_c in &temps {
        // Update the context circuit's temperature so the next analysis (or
        // a follow-up `.control` step) inherits the sweep's final value,
        // matching the old Netlist-side `set_netlist_temp` behaviour.
        match ctx.circuit.as_mut() {
            Some(c) => c.temps = vec![temp_c],
            None => return Err("dc temp: no circuit attached to context".to_string()),
        }

        // Working clone for this point: apply TEMPER on IR shape and force
        // an OP analysis (temp sweep is OP-only by definition).
        let mut circuit = ctx.circuit().expect("ctx.circuit set above").clone();
        evaluate_temper_exprs_circuit(&mut circuit, temp_c);
        circuit.analyses = vec![cirq_ir::Analysis::Op];

        let result = thevenin::circuit::simulate_op(&circuit)
            .map_err(|e| format!("dc temp: OP at T={temp_c}: {e}"))?;

        if let Some(plot) = result.plots.first() {
            if first {
                // Initialize result vectors
                vecs.push(SimVector::real("temp-sweep", Vec::new()));
                for v in &plot.vecs {
                    vecs.push(SimVector::real(v.name.clone(), Vec::new()));
                }
                first = false;
            }

            // Record temperature
            vecs[0].data.as_real_mut().push(temp_c);
            // Record all node voltages and branch currents
            for (i, v) in plot.vecs.iter().enumerate() {
                if i + 1 < vecs.len() {
                    let val = v.data.as_real().first().copied().unwrap_or(0.0);
                    vecs[i + 1].data.as_real_mut().push(val);
                }
            }
        }
    }

    let plot = SimPlot {
        name: "dc1".to_string(),
        vecs,
    };
    ctx.add_plot(plot);
    Ok(())
}

/// Substitute the `temper` keyword in `Value::String("{...}")` model
/// parameters with the current temperature and evaluate the inner
/// expression to a `Value::Real`, then apply TC1/TC2/TCE temperature
/// scaling to resistor models and instances.
///
/// TCE overrides TC1/TC2 at instance level, instance TC overrides model
/// TC, and model-based resistor instances are left alone (their model's
/// `R` was already scaled here).
fn evaluate_temper_exprs_circuit(circuit: &mut cirq_ir::Circuit, temp_c: f64) {
    use cirq_ir::{DeviceType, ElementKind, Value};

    // Collect TC for resistor models, and apply TEMPER eval + model R scaling
    // in a single pass.
    let mut model_tc: std::collections::HashMap<String, (f64, f64, f64)> =
        std::collections::HashMap::new();

    for model in &mut circuit.models {
        // TEMPER eval: replace 'temper' inside brace-quoted Value::String params.
        for (_name, value) in &mut model.params {
            if let Value::String(s) = value
                && let Some(inner) = s.strip_prefix('{').and_then(|t| t.strip_suffix('}'))
            {
                let replaced = crate::vecexpr::replace_word(inner, "temper", &temp_c.to_string());
                let eval_ctx = thevenin::expr::EvalContext::default();
                if let Ok(val) = eval_ctx.eval_str(&replaced) {
                    *value = Value::Real(val);
                }
            }
        }

        // Resistor-model TC collection + R scaling.
        let is_resistor_model = matches!(
            &model.device_type,
            DeviceType::Other(s) if s.eq_ignore_ascii_case("r")
        );
        if !is_resistor_model {
            continue;
        }
        let mut has_tc = false;
        let mut tc1 = 0.0_f64;
        let mut tc2 = 0.0_f64;
        let mut tce = 0.0_f64;
        for (name, value) in &model.params {
            if let Some(v) = crate::vecexpr::value_as_real(value) {
                match name.to_uppercase().as_str() {
                    "TC1" => {
                        tc1 = v;
                        has_tc = true;
                    }
                    "TC2" => {
                        tc2 = v;
                        has_tc = true;
                    }
                    "TCE" => {
                        tce = v;
                        has_tc = true;
                    }
                    _ => {}
                }
            }
        }
        if has_tc {
            model_tc.insert(model.name.to_uppercase(), (tc1, tc2, tce));
            let tdiff = temp_c - 27.0;
            if tdiff != 0.0 {
                for (name, value) in &mut model.params {
                    if (name.eq_ignore_ascii_case("r") || name.eq_ignore_ascii_case("resistance"))
                        && let Some(r) = crate::vecexpr::value_as_real(value)
                    {
                        let factor = if tce != 0.0 {
                            1.01_f64.powf(tce * tdiff)
                        } else {
                            1.0 + tc1 * tdiff + tc2 * tdiff * tdiff
                        };
                        *value = Value::Real(r * factor);
                    }
                }
            }
        }
    }

    // Snapshot model id -> name (uppercase) so the instance loop below can
    // look up TC fallbacks without re-borrowing `circuit.models`.
    let model_name_by_id: std::collections::HashMap<cirq_ir::Id, String> = circuit
        .models
        .iter()
        .map(|m| (m.id, m.name.to_uppercase()))
        .collect();

    // Apply TC to resistor element instances.
    let tnom = 27.0_f64;
    let tdiff = temp_c - tnom;

    for elem in &mut circuit.elements {
        if !matches!(elem.kind, ElementKind::Resistor) {
            continue;
        }
        let mut tc1 = 0.0_f64;
        let mut tc2 = 0.0_f64;
        let mut tce = 0.0_f64;
        let mut has_instance_tc = false;
        let mut has_instance_tce = false;

        for (name, value) in &elem.params {
            if let Some(v) = crate::vecexpr::value_as_real(value) {
                match name.to_uppercase().as_str() {
                    "TC1" => {
                        tc1 = v;
                        has_instance_tc = true;
                    }
                    "TC2" => {
                        tc2 = v;
                        has_instance_tc = true;
                    }
                    "TCE" => {
                        tce = v;
                        has_instance_tce = true;
                    }
                    _ => {}
                }
            }
        }
        // TCE takes precedence over TC1/TC2 at instance level.
        if has_instance_tce {
            has_instance_tc = false;
        }

        let is_model_based = elem.model.is_some();

        // Model-TC fallback only when no instance TC was set AND the element
        // is freestanding. For model-based resistors, the model's R parameter
        // was already scaled above; touching the instance "value" would
        // double-apply.
        if !has_instance_tc
            && !has_instance_tce
            && let Some(model_id) = elem.model
            && let Some(model_name_upper) = model_name_by_id.get(&model_id)
            && let Some(&(mtc1, mtc2, mtce)) = model_tc.get(model_name_upper)
        {
            tc1 = mtc1;
            tc2 = mtc2;
            tce = mtce;
            if mtce != 0.0 {
                has_instance_tce = true;
            } else {
                has_instance_tc = true;
            }
        }

        // Scale the instance "value" only for freestanding resistors with TC.
        if (has_instance_tc || has_instance_tce) && tdiff != 0.0 && !is_model_based {
            let factor = if has_instance_tce || tce != 0.0 {
                1.01_f64.powf(tce * tdiff)
            } else {
                1.0 + tc1 * tdiff + tc2 * tdiff * tdiff
            };
            for (name, value) in &mut elem.params {
                if (name.eq_ignore_ascii_case("value") || name.eq_ignore_ascii_case("resistance"))
                    && let Some(r) = crate::vecexpr::value_as_real(value)
                {
                    *value = Value::Real(r * factor);
                    break;
                }
            }
        }

        // Strip tc1 / tc2 / tce element params once we've baked them into
        // `value`, so the downstream IR-level stamping (which now also
        // recognises tc1/tc2 on plain resistors — see
        // `mna_ir::resolve_resistor_tc`) does not re-apply the same factor.
        // The `is_model_based` branch is intentionally also affected: model
        // R is already pre-scaled above, so any leftover instance tc on a
        // model-based resistor would be wrong to apply a second time.
        if has_instance_tc || has_instance_tce {
            elem.params.retain(|(name, _)| {
                !name.eq_ignore_ascii_case("tc1")
                    && !name.eq_ignore_ascii_case("tc2")
                    && !name.eq_ignore_ascii_case("tce")
            });
        }
    }
}

/// Execute an `alter` command.
///
/// Mutates the driving [`cirq_ir::Circuit`] when the spec resolves to a
/// device the circuit knows about; otherwise falls back to stashing the
/// value as a named vector so `@device[param]` lookups still resolve.
///
/// Accepted spec shapes:
/// - `@device[param] = value` — explicit param (e.g. `@r1[resistance]`)
/// - `@device = value` — defaults to the kind's primary param (`dc` for
///   sources, `value` for R/C/L)
/// - `device[param] = value` / `device = value` — same, without the `@`
///   prefix (the resume-1 plain form)
///
/// Vector alters (e.g. `@v1[pulse] = [ ... ]`) always take the stored-vector
/// path because waveform parameters are not first-class on the IR yet —
/// they live inside `Element.source_spec.waveform` with a typed shape
/// that doesn't accept a flat coefficient vector.
fn execute_alter(spec: &str, value: &AlterValue, ctx: &mut SimContext) -> Result<(), String> {
    let spec_trimmed = spec.trim();
    let (device, param) = parse_alter_spec(spec_trimmed)?;

    // Vector alters keep the legacy stash-as-named-vector behavior; the
    // IR doesn't represent waveform params as a flat coefficient vector
    // we could push back into Element.source_spec.
    let scalar = match value {
        AlterValue::Scalar(v) => *v,
        AlterValue::Vector(v) => {
            ctx.store_vector(SimVector::real(spec_trimmed.to_lowercase(), v.clone()));
            return Ok(());
        }
    };

    // Try mutating the Circuit first. If the circuit doesn't have the
    // referenced device/model, fall through to the named-vector stash
    // so `@device[param]` lookups still resolve.
    let mutated = ctx
        .circuit
        .as_mut()
        .is_some_and(|circuit| alter_circuit(circuit, &device, param.as_deref(), scalar));
    if mutated {
        // The Circuit is the single source of truth for the next analysis
        // (assemble_mna_from_circuit reads it directly); no cache refresh
        // needed. Also stash the new value as a named vector so
        // expressions that look up `@device[param]` via find_vector keep
        // working.
        ctx.store_vector(SimVector::real(spec_trimmed.to_lowercase(), vec![scalar]));
        return Ok(());
    }

    // Fall-through: stash the value as a named vector.
    ctx.store_vector(SimVector::real(spec_trimmed.to_lowercase(), vec![scalar]));
    Ok(())
}

/// Parse an `alter` spec into `(device_name, optional_param_name)`.
///
/// Accepts `@device[param]`, `@device`, `device[param]`, and `device`. The
/// `@` prefix is optional — both ngspice's bracketed form and the plain
/// form used in resume-1 (`alter v1 = -5`) are valid.
fn parse_alter_spec(spec: &str) -> Result<(String, Option<String>), String> {
    let s = spec.strip_prefix('@').unwrap_or(spec).trim();
    if s.is_empty() {
        return Err(format!("alter: missing device name in spec: {spec}"));
    }
    if let Some(bracket) = s.find('[') {
        let end = s
            .find(']')
            .ok_or_else(|| format!("alter: unmatched '[' in spec: {spec}"))?;
        if end <= bracket {
            return Err(format!("alter: malformed [param] in spec: {spec}"));
        }
        let device = s[..bracket].trim().to_string();
        let param = s[bracket + 1..end].trim().to_string();
        if device.is_empty() || param.is_empty() {
            return Err(format!("alter: empty device or param in spec: {spec}"));
        }
        Ok((device, Some(param)))
    } else {
        Ok((s.to_string(), None))
    }
}

/// Apply an `alter` to a Circuit. Returns `true` if a matching element or
/// model was found and mutated, `false` otherwise (so the caller can fall
/// back to legacy behavior).
fn alter_circuit(
    circuit: &mut cirq_ir::Circuit,
    device: &str,
    param: Option<&str>,
    value: f64,
) -> bool {
    for elem in &mut circuit.elements {
        if elem.name.eq_ignore_ascii_case(device) {
            return apply_element_alter(elem, param, value);
        }
    }
    for model in &mut circuit.models {
        if model.name.eq_ignore_ascii_case(device) {
            return apply_model_alter(model, param, value);
        }
    }
    false
}

fn apply_element_alter(elem: &mut cirq_ir::Element, param: Option<&str>, value: f64) -> bool {
    use cirq_ir::{ElementKind, SourceSpec, Value};

    // Pick a default param name when the alter is plain-form (no
    // explicit `[param]`). Sources default to `dc`; R/C/L default to
    // `value`. Anything else without an explicit param fails so callers
    // know to be explicit (e.g. M1 needs `m1[w]`, not `m1`).
    let default_param = match elem.kind {
        ElementKind::VoltageSource | ElementKind::CurrentSource => Some("dc"),
        ElementKind::Resistor | ElementKind::Capacitor | ElementKind::Inductor => Some("value"),
        _ => None,
    };
    let Some(p) = param.or(default_param) else {
        return false;
    };
    let p_lower = p.to_lowercase();

    // Source DC value lives in source_spec.dc, not in params.
    if matches!(
        elem.kind,
        ElementKind::VoltageSource | ElementKind::CurrentSource
    ) && p_lower == "dc"
    {
        let spec = elem.source_spec.get_or_insert_with(SourceSpec::default);
        spec.dc = Some(value);
        return true;
    }

    // Otherwise it's a typed param. Update in place if present, else
    // append so element ordering of existing params is preserved.
    for (name, v) in &mut elem.params {
        if name.to_lowercase() == p_lower {
            *v = Value::Real(value);
            return true;
        }
    }
    elem.params.push((p.to_string(), Value::Real(value)));
    true
}

fn apply_model_alter(model: &mut cirq_ir::Model, param: Option<&str>, value: f64) -> bool {
    use cirq_ir::Value;

    // Model alters require an explicit param — there is no sensible
    // default since a model is a bag of named coefficients.
    let Some(p) = param else { return false };
    let p_lower = p.to_lowercase();
    for (name, v) in &mut model.params {
        if name.to_lowercase() == p_lower {
            *v = Value::Real(value);
            return true;
        }
    }
    model.params.push((p.to_string(), Value::Real(value)));
    true
}

/// Resolve echo fragments into a string.
fn resolve_echo(fragments: &[EchoFragment], ctx: &SimContext) -> String {
    let mut result = String::new();
    for frag in fragments {
        match frag {
            EchoFragment::Literal(s) => result.push_str(s),
            EchoFragment::VarRef(name) => {
                result.push_str(&ctx.resolve_var(name));
            }
            EchoFragment::VecScalar(name) => {
                result.push_str(&ctx.resolve_vec_scalar(name));
            }
        }
    }
    result
}

/// Interpolate `$var`, `$&var`, and `{$var}` references in a string.
pub fn interpolate_vars(s: &str, ctx: &SimContext) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'$') {
            // {$var} — braced variable substitution (strips braces)
            chars.next(); // consume '$'
            let mut name = String::new();
            while let Some(&c) = chars.peek() {
                if c == '}' {
                    chars.next(); // consume '}'
                    break;
                }
                if c.is_alphanumeric() || c == '_' {
                    name.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            result.push_str(&ctx.resolve_var(&name));
        } else if c == '$' {
            if chars.peek() == Some(&'&') {
                chars.next();
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                result.push_str(&ctx.resolve_vec_scalar(&name));
            } else if chars
                .peek()
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                result.push_str(&ctx.resolve_var(&name));
            } else {
                result.push('$');
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn format_print_value(v: f64) -> String {
    if v == 0.0 {
        "0".to_string()
    } else if v.abs() >= 0.001 && v.abs() < 1e6 {
        format!("{v:.6e}")
    } else {
        format!("{v:e}")
    }
}
