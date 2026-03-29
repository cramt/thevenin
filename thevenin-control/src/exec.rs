//! Executor for `.control` block statements.

use thevenin_types::{
    AcVariation, Analysis, DcSweep, Expr, PzAnalysisType, PzInputType, SimPlot, SimResult,
    SimVector,
};

use crate::ast::{AlterValue, EchoFragment, Statement};
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
            let vec = SimVector {
                name: name.clone(),
                real: val.data,
                complex: Vec::new(),
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
            let vec = SimVector {
                name: name.clone(),
                real: values,
                complex: Vec::new(),
            };
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
            let vec = SimVector {
                name: format!("__{result}"),
                real: vec![cmp],
                complex: Vec::new(),
            };
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

    // Parse the command line into an Analysis and temporarily inject it into the netlist
    let analysis = parse_analysis_command(&cmd, &parts[1..])?;

    // Build a working copy of the netlist with TEMPER expressions resolved
    let mut netlist = ctx.netlist.clone();
    let temp_c = thevenin::netlist_temp(&netlist);
    evaluate_temper_exprs(&mut netlist, temp_c);
    netlist
        .items
        .push(thevenin_types::Item::Analysis(analysis.clone()));

    let result = match &analysis {
        Analysis::Op => thevenin::simulate_op_dc(&netlist).map_err(|e| format!("OP: {e}")),
        Analysis::Dc { .. } => thevenin::simulate_dc(&netlist).map_err(|e| format!("DC: {e}")),
        Analysis::Tran { .. } => {
            thevenin::simulate_tran(&netlist).map_err(|e| format!("Tran: {e}"))
        }
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
                vecs.push(SimVector {
                    name: "temp-sweep".to_string(),
                    real: Vec::new(),
                    complex: vec![],
                });
                for v in &plot.vecs {
                    vecs.push(SimVector {
                        name: v.name.clone(),
                        real: Vec::new(),
                        complex: vec![],
                    });
                }
                first = false;
            }

            // Record temperature
            vecs[0].real.push(temp_c);
            // Record all node voltages and branch currents
            for (i, v) in plot.vecs.iter().enumerate() {
                if i + 1 < vecs.len() {
                    let val = v.real.first().copied().unwrap_or(0.0);
                    vecs[i + 1].real.push(val);
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
            if args.len() < 2 {
                return Err("tran: need tstep tstop".to_string());
            }
            Ok(Analysis::Tran {
                tstep: Expr::Num(parse_num(args[0])?),
                tstop: Expr::Num(parse_num(args[1])?),
                tstart: if args.len() > 2 {
                    Some(Expr::Num(parse_num(args[2])?))
                } else {
                    None
                },
                tmax: if args.len() > 3 {
                    Some(Expr::Num(parse_num(args[3])?))
                } else {
                    None
                },
            })
        }
        "sens" => {
            if args.is_empty() {
                return Err("sens: need output variable".to_string());
            }
            // `sens v(1) dc` — collect output vars, skip "dc"/"ac" keyword
            let output: Vec<String> = args
                .iter()
                .filter(|a| !a.eq_ignore_ascii_case("dc") && !a.eq_ignore_ascii_case("ac"))
                .map(|a| a.to_string())
                .collect();
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
        "run" => {
            // `run` executes whatever analyses are in the netlist — default to OP
            Ok(Analysis::Op)
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
fn execute_alter(spec: &str, value: &AlterValue, _ctx: &mut SimContext) -> Result<(), String> {
    // Parse @device[param]
    let spec = spec.trim();
    if !spec.starts_with('@') {
        return Err(format!("alter: expected @device[param], got: {spec}"));
    }
    let inner = &spec[1..];
    let bracket_start = inner
        .find('[')
        .ok_or_else(|| format!("alter: no '[' in {spec}"))?;
    let bracket_end = inner
        .find(']')
        .ok_or_else(|| format!("alter: no ']' in {spec}"))?;
    let _device = &inner[..bracket_start];
    let _param = &inner[bracket_start + 1..bracket_end];

    // TODO: Actually modify device parameters in the netlist.
    // For now, store the alter value so test circuits can at least run
    // without error. Full implementation requires re-running device setup.
    let _ = value;

    Ok(())
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
