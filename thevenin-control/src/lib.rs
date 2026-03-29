//! `.control` block interpreter for the thevenin circuit simulator.
//!
//! Parses and executes the ngspice `.control` / `.endc` scripting language,
//! supporting simulation commands, vector expressions, control flow, and
//! variable management.

pub mod ast;
pub mod context;
pub mod exec;
pub mod parse;
pub mod vecexpr;

use context::SimContext;
use exec::ControlResult;
use thevenin_types::{Netlist, SimResult};

/// Execute a `.control` block from a netlist.
///
/// Finds the first `Item::Control` in the netlist, parses it, and executes
/// all commands. Returns the merged simulation results and exit code.
pub fn execute_control_block(netlist: &Netlist) -> Result<ControlResult, String> {
    // Find .control block(s)
    let control_lines: Vec<&Vec<String>> = netlist
        .items
        .iter()
        .filter_map(|item| {
            if let thevenin_types::Item::Control(lines) = item {
                Some(lines)
            } else {
                None
            }
        })
        .collect();

    if control_lines.is_empty() {
        return Err("no .control block found".to_string());
    }

    // Create execution context with the netlist (minus .control blocks)
    let mut ctx = SimContext::new(netlist.clone());

    // Execute each .control block
    for lines in control_lines {
        let stmts = parse::parse_control_block(lines)?;
        exec::execute(&stmts, &mut ctx)?;
        if ctx.exit_code.is_some() {
            break;
        }
    }

    let exit_code = ctx.exit_code.unwrap_or(0);

    // Merge all plots into a SimResult
    let sim_result = SimResult { plots: ctx.plots };

    Ok(ControlResult {
        sim_result,
        exit_code,
        output: ctx.output,
    })
}

/// Check if a netlist contains a `.control` block.
pub fn has_control_block(netlist: &Netlist) -> bool {
    netlist
        .items
        .iter()
        .any(|item| matches!(item, thevenin_types::Item::Control(_)))
}
