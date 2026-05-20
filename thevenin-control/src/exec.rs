//! Executor for `.control` block statements.

use thevenin::{TranOutcome, TranStartState};
use thevenin_types::{
    AcVariation, Analysis, DcSweep, Expr, PzAnalysisType, PzInputType, SimPlot, SimResult,
    SimVector,
};

use crate::ast::{AlterValue, EchoFragment, Statement, StopCondition};
use crate::context::SimContext;
use crate::vecexpr::{eval_condition, eval_vec_expr};

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
    }
}

/// Run a simulation command (op, dc, ac, tran, sens, noise, pz, tf).
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

    // Parse the command line into an Analysis and build a single-analysis netlist.
    // `run` re-executes whatever analysis is declared in the netlist.
    let analysis = if cmd == "run" {
        ctx.netlist.analysis.clone()
    } else {
        parse_analysis_command(&cmd, &parts[1..])?
    };

    // Build a working copy of the netlist with the analysis set directly
    let mut netlist = ctx.netlist.clone();
    netlist.analysis = analysis.clone();
    let temp_c = thevenin::netlist_temp(&netlist);
    evaluate_temper_exprs(&mut netlist, temp_c);

    let result = match &analysis {
        Analysis::Op => thevenin::simulate_op_dc(&netlist).map_err(|e| format!("OP: {e}")),
        Analysis::Dc { .. } => thevenin::simulate_dc(&netlist).map_err(|e| format!("DC: {e}")),
        Analysis::Tran { .. } => run_tran_with_pause(&netlist, ctx),
        Analysis::Ac { .. } => thevenin::simulate_ac(&netlist).map_err(|e| format!("AC: {e}")),
        Analysis::Sens { .. } => {
            thevenin::simulate_sens(&netlist).map_err(|e| format!("Sens: {e}"))
        }
        Analysis::Noise { .. } => {
            thevenin::simulate_noise(&netlist).map_err(|e| format!("Noise: {e}"))
        }
        Analysis::Pz { .. } => thevenin::simulate_pz(&netlist).map_err(|e| format!("PZ: {e}")),
        Analysis::Tf { .. } => thevenin::simulate_tf(&netlist).map_err(|e| format!("TF: {e}")),
    };

    // Store resolved model parameters for @model[param] queries
    for item in &netlist.items {
        if let thevenin_types::Item::Model(model) = item {
            ctx.resolved_models
                .insert(model.name.to_uppercase(), model.params.clone());
        }
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
    netlist: &thevenin_types::Netlist,
    ctx: &mut SimContext,
) -> Result<SimResult, String> {
    let stop = ctx.stop_when.take();
    let t_pause = stop.map(|StopCondition::TimeEq(t)| t);

    let mna = thevenin::mna::assemble_mna(netlist).map_err(|e| format!("Tran: {e}"))?;
    let mut params =
        thevenin::tran_run_params_from_netlist(netlist, &mna).map_err(|e| format!("Tran: {e}"))?;
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

    // Build a working netlist from the current ctx.netlist (already
    // refreshed by any `alter` calls between pause and resume). Override
    // its analysis with the paused leg's Tran params so the resumed run
    // honours the original `tstep`/`tstop` even when `tran` was an
    // interpreter command (.control-only) rather than a netlist directive.
    let mut netlist = ctx.netlist.clone();
    netlist.analysis = Analysis::Tran {
        tstep: Expr::Num(snapshot.t_step),
        tstop: Expr::Num(snapshot.t_stop),
        tstart: None,
        tmax: snapshot.t_max.map(Expr::Num),
        // The original paused leg may have been uic; the resumed leg
        // does not re-apply uic — it starts from the snapshot's solution
        // via `start_state` and so its IC/uic handling is short-circuited
        // inside run_tran (see the `start_state.is_none()` guard).
        uic: false,
    };
    let temp_c = thevenin::netlist_temp(&netlist);
    evaluate_temper_exprs(&mut netlist, temp_c);

    let mna = thevenin::mna::assemble_mna(&netlist).map_err(|e| format!("resume: {e}"))?;
    let mut params = thevenin::tran_run_params_from_netlist(&netlist, &mna)
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

/// Run a DC temperature sweep: `dc temp start stop step`.
///
/// At each temperature point, modifies the netlist temperature, re-evaluates
/// TEMPER-dependent expressions, re-assembles MNA, and solves the operating point.
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
        // Set the temperature in the netlist
        set_netlist_temp(&mut ctx.netlist, temp_c);

        // Re-evaluate TEMPER-dependent expressions
        let mut netlist_copy = ctx.netlist.clone();
        evaluate_temper_exprs(&mut netlist_copy, temp_c);

        // Re-resolve expressions (parameters may depend on temperature)
        if let Err(e) = thevenin::expr::resolve_netlist_exprs(&mut netlist_copy) {
            return Err(format!("dc temp: expression resolution at T={temp_c}: {e}"));
        }

        // Flatten subcircuits
        let flat = thevenin::flatten_netlist(&netlist_copy)
            .map_err(|e| format!("dc temp: flatten at T={temp_c}: {e}"))?;

        // Solve OP
        let result = thevenin::simulate_op_dc(&flat)
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

/// Set the simulation temperature in the netlist.
fn set_netlist_temp(netlist: &mut thevenin_types::Netlist, temp_c: f64) {
    // Remove existing .temp directives and add new one
    netlist
        .items
        .retain(|item| !matches!(item, thevenin_types::Item::Temp(_)));
    netlist.items.push(thevenin_types::Item::Temp(temp_c));
}

/// Evaluate model parameter expressions, substituting TEMPER keyword, and
/// apply temperature coefficients (TC1/TC2/TCE) to resistor elements.
///
/// Scans model definitions for parameters with Brace expressions, substitutes
/// 'temper' with the current temperature, and evaluates to numeric values.
/// This also resolves non-TEMPER Brace expressions in models (e.g., `is='1e-12'`)
/// that `resolve_netlist_exprs` doesn't touch.
fn evaluate_temper_exprs(netlist: &mut thevenin_types::Netlist, temp_c: f64) {
    use thevenin_types::{Expr, Item};

    // Collect model TC parameters for resistor models
    let mut model_tc: std::collections::HashMap<String, (f64, f64, f64)> =
        std::collections::HashMap::new();

    for item in &mut netlist.items {
        if let Item::Model(model) = item {
            let mut has_tc = false;
            let mut tc1 = 0.0_f64;
            let mut tc2 = 0.0_f64;
            let mut tce = 0.0_f64;

            for param in &mut model.params {
                if let Expr::Brace(expr) = &param.value {
                    let replaced =
                        crate::vecexpr::replace_word(expr, "temper", &temp_c.to_string());
                    let eval_ctx = thevenin::expr::EvalContext::default();
                    if let Ok(val) = eval_ctx.eval_str(&replaced) {
                        param.value = Expr::Num(val);
                    }
                }
                // Collect TC parameters from resistor models
                if let Expr::Num(v) = &param.value {
                    match param.name.to_uppercase().as_str() {
                        "TC1" => {
                            tc1 = *v;
                            has_tc = true;
                        }
                        "TC2" => {
                            tc2 = *v;
                            has_tc = true;
                        }
                        "TCE" => {
                            tce = *v;
                            has_tc = true;
                        }
                        _ => {}
                    }
                }
            }
            if has_tc && model.kind.eq_ignore_ascii_case("r") {
                model_tc.insert(model.name.to_uppercase(), (tc1, tc2, tce));

                // Apply TC to the model's R parameter
                let tdiff = temp_c - 27.0;
                if tdiff != 0.0 {
                    for param in &mut model.params {
                        if (param.name.eq_ignore_ascii_case("r")
                            || param.name.eq_ignore_ascii_case("resistance"))
                            && let Expr::Num(r) = &param.value
                        {
                            let factor = if tce != 0.0 {
                                1.01_f64.powf(tce * tdiff)
                            } else {
                                1.0 + tc1 * tdiff + tc2 * tdiff * tdiff
                            };
                            param.value = Expr::Num(*r * factor);
                        }
                    }
                }
            }
        }
    }

    // Apply TC1/TC2/TCE to resistor elements (instance-level params)
    let tnom = 27.0_f64; // default TNOM
    let tdiff = temp_c - tnom;

    for item in &mut netlist.items {
        if let Item::Element(el) = item
            && let thevenin_types::ElementKind::Resistor { value, params, .. } = &mut el.kind
        {
            // Get TC from instance params or model
            let mut tc1 = 0.0_f64;
            let mut tc2 = 0.0_f64;
            let mut tce = 0.0_f64;
            let mut has_instance_tc = false;
            let mut has_instance_tce = false;

            for p in params.iter() {
                if let Expr::Num(v) = &p.value {
                    match p.name.to_uppercase().as_str() {
                        "TC1" => {
                            tc1 = *v;
                            has_instance_tc = true;
                        }
                        "TC2" => {
                            tc2 = *v;
                            has_instance_tc = true;
                        }
                        "TCE" => {
                            tce = *v;
                            has_instance_tce = true;
                        }
                        _ => {}
                    }
                }
            }

            // If instance has TCE, it overrides TC1/TC2
            if has_instance_tce {
                has_instance_tc = false; // TCE takes precedence
            }

            // Fall back to model TC if no instance TC
            if !has_instance_tc && !has_instance_tce {
                // Check if element uses a resistor model
                if let Expr::Param(model_name) = value
                    && let Some(&(mtc1, mtc2, mtce)) = model_tc.get(&model_name.to_uppercase())
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
            }

            // Apply temperature scaling to resistance value
            if (has_instance_tc || has_instance_tce) && tdiff != 0.0 {
                // Get the base resistance value
                let base_r = match value {
                    Expr::Num(r) => Some(*r),
                    Expr::Param(_model_name) => {
                        // Model-based resistor: look up the model's R parameter
                        // The model params were already resolved above
                        None // handled below via model_r lookup
                    }
                    _ => None,
                };

                let factor = if has_instance_tce || tce != 0.0 {
                    1.01_f64.powf(tce * tdiff)
                } else {
                    1.0 + tc1 * tdiff + tc2 * tdiff * tdiff
                };

                if let Some(r) = base_r {
                    *value = Expr::Num(r * factor);
                }
                // For model-based resistors, we can't modify the element value
                // directly (it's a model reference). Instead, we need to modify
                // the model's R parameter.
            }
        }
    }
}

/// Parse an analysis command string into an Analysis enum.
fn parse_analysis_command(cmd: &str, args: &[&str]) -> Result<Analysis, String> {
    match cmd {
        "op" => Ok(Analysis::Op),
        "dc" => {
            if args.len() < 4 {
                return Err(format!("dc: need src start stop step, got {args:?}"));
            }
            let src = args[0].to_string();
            let start = Expr::Num(parse_num(args[1])?);
            let stop = Expr::Num(parse_num(args[2])?);
            let step = Expr::Num(parse_num(args[3])?);
            let src2 = if args.len() >= 8 {
                Some(DcSweep {
                    src: args[4].to_string(),
                    start: Expr::Num(parse_num(args[5])?),
                    stop: Expr::Num(parse_num(args[6])?),
                    step: Expr::Num(parse_num(args[7])?),
                })
            } else {
                None
            };
            Ok(Analysis::Dc {
                src,
                start,
                stop,
                step,
                src2,
            })
        }
        "ac" => {
            if args.len() < 4 {
                return Err("ac: need variation n fstart fstop".to_string());
            }
            let variation = match args[0].to_lowercase().as_str() {
                "dec" => AcVariation::Dec,
                "oct" => AcVariation::Oct,
                "lin" => AcVariation::Lin,
                other => return Err(format!("ac: unknown variation: {other}")),
            };
            Ok(Analysis::Ac {
                variation,
                n: parse_num(args[1])? as u32,
                fstart: Expr::Num(parse_num(args[2])?),
                fstop: Expr::Num(parse_num(args[3])?),
            })
        }
        "tran" => {
            // ngspice grammar: `tran tstep tstop [tstart [tmax]] [uic]`.
            // The trailing `uic` keyword is optional and order-independent
            // relative to the numeric positionals — strip it first so the
            // remaining args are pure positionals.
            let mut numeric: Vec<&str> = Vec::with_capacity(args.len());
            let mut uic = false;
            for a in args {
                if a.eq_ignore_ascii_case("uic") {
                    uic = true;
                } else {
                    numeric.push(a);
                }
            }
            if numeric.len() < 2 {
                return Err("tran: need tstep tstop".to_string());
            }
            Ok(Analysis::Tran {
                tstep: Expr::Num(parse_num(numeric[0])?),
                tstop: Expr::Num(parse_num(numeric[1])?),
                tstart: if numeric.len() > 2 {
                    Some(Expr::Num(parse_num(numeric[2])?))
                } else {
                    None
                },
                tmax: if numeric.len() > 3 {
                    Some(Expr::Num(parse_num(numeric[3])?))
                } else {
                    None
                },
                uic,
            })
        }
        "sens" => {
            if args.is_empty() {
                return Err("sens: need output variable".to_string());
            }
            // `sens v(1) dc` or `sens v(1) ac lin 1 1e6 1.1e6`
            // Keep all tokens — simulate_sens parses dc/ac from output[1].
            let output: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            Ok(Analysis::Sens { output })
        }
        "noise" => {
            if args.len() < 6 {
                return Err("noise: need output ref src variation n fstart fstop".to_string());
            }
            let variation = match args[2].to_lowercase().as_str() {
                "dec" => AcVariation::Dec,
                "oct" => AcVariation::Oct,
                "lin" => AcVariation::Lin,
                other => return Err(format!("noise: unknown variation: {other}")),
            };
            Ok(Analysis::Noise {
                output: args[0].to_string(),
                ref_node: None,
                src: args[1].to_string(),
                variation,
                n: parse_num(args[3])? as u32,
                fstart: Expr::Num(parse_num(args[4])?),
                fstop: Expr::Num(parse_num(args[5])?),
            })
        }
        "pz" => {
            if args.len() < 6 {
                return Err("pz: need 6 args".to_string());
            }
            let input_type = match args[4].to_lowercase().as_str() {
                "vol" => PzInputType::Vol,
                "cur" => PzInputType::Cur,
                other => return Err(format!("pz: unknown input type: {other}")),
            };
            let analysis_type = match args[5].to_lowercase().as_str() {
                "pol" => PzAnalysisType::Pol,
                "zer" => PzAnalysisType::Zer,
                "pz" => PzAnalysisType::Pz,
                other => return Err(format!("pz: unknown analysis type: {other}")),
            };
            Ok(Analysis::Pz {
                node_i: args[0].to_string(),
                node_g: args[1].to_string(),
                node_j: args[2].to_string(),
                node_k: args[3].to_string(),
                input_type,
                analysis_type,
            })
        }
        "tf" => {
            if args.len() < 2 {
                return Err("tf: need output input".to_string());
            }
            Ok(Analysis::Tf {
                output: args[0].to_string(),
                input: args[1].to_string(),
            })
        }
        _ => Err(format!("unknown analysis command: {cmd}")),
    }
}

fn parse_num(s: &str) -> Result<f64, String> {
    // Strip trailing unit suffixes that aren't SI prefixes
    let s =
        s.trim_end_matches(|c: char| c.is_ascii_alphabetic() && !"tTgGkKmMuUnNpPfFaA".contains(c));
    crate::parse::parse_spice_number_pub(s)
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
        // Re-derive the cached netlist so the next analysis (which
        // still reads through the SPICE-Expr-shape adapter) sees the
        // mutation. Both `ctx.circuit` and `ctx.netlist` track the
        // same logical state.
        ctx.refresh_netlist_cache()?;
        // Also stash the new value as a named vector so expressions
        // that look up `@device[param]` via find_vector keep working.
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
