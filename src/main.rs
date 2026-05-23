#![cfg_attr(target_arch = "wasm32", allow(unused))]

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
use facet::Facet;
#[cfg(not(target_arch = "wasm32"))]
use figue::{self as args, FigueBuiltins};

#[cfg(not(target_arch = "wasm32"))]
#[derive(Facet)]
struct Cli {
    #[facet(args::subcommand)]
    command: Command,

    #[facet(flatten)]
    builtins: FigueBuiltins,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Facet)]
#[repr(u8)]
#[allow(dead_code)]
enum Command {
    /// Run a circuit simulation.
    Run {
        /// Input file (.cirq or SPICE netlist).
        #[facet(args::positional)]
        input: String,
    },
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let cli: Cli = figue::from_std_args().unwrap();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Run { input } => {
            let src = std::fs::read_to_string(&input)
                .map_err(|e| format!("failed to read {input}: {e}"))?;
            let circuits: Vec<cirq_ir::Circuit> = if is_cirq_file(&input) {
                let base_dir = Path::new(&input)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_owned();
                let circuit = cirq_frontend::compile_file(&src, &base_dir).map_err(|diags| {
                    let msgs: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
                    msgs.join("\n")
                })?;
                vec![circuit]
            } else {
                cirq_spice_import::import_spice(&src)
                    .map_err(|e| format!("SPICE import to Cirq IR failed: {e}"))?
            };
            run_circuits(&circuits)
        }
    }
}

/// Dispatch each Cirq IR circuit through the IR-shaped entry points.
///
/// Non-`.control` circuits go through [`thevenin::circuit::simulate`], the
/// Stage 4 Circuit-input dispatcher that bypasses the lowered Netlist on
/// the happy path. `.control` blocks still need the Netlist-shaped
/// interpreter context (TEMPER + `@device[param]` are not yet on IR — see
/// `docs/archive/migration/old-path-retirement-checklist.md`).
#[cfg(not(target_arch = "wasm32"))]
fn run_circuits(circuits: &[cirq_ir::Circuit]) -> Result<(), Box<dyn std::error::Error>> {
    for circuit in circuits {
        if thevenin_control::has_control_block_ir(circuit) {
            let ctrl_result = thevenin_control::execute_control_block_ir(circuit)
                .map_err(|e| format!("control block error: {e}"))?;
            print_control_result(&ctrl_result);
        } else {
            let result = thevenin::circuit::simulate(circuit)
                .map_err(|e| format!("simulation error: {e}"))?;
            print_plots(&result.plots);
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn print_control_result(ctrl_result: &thevenin_control::exec::ControlResult) {
    if !ctrl_result.output.is_empty() {
        print!("{}", ctrl_result.output);
    }
    print_plots(&ctrl_result.sim_result.plots);
}

#[cfg(not(target_arch = "wasm32"))]
fn print_plots(plots: &[thevenin_types::SimPlot]) {
    for plot in plots {
        println!("{}:", plot.name);
        for vec in &plot.vecs {
            let preview: Vec<String> = vec
                .data
                .as_real()
                .iter()
                .take(5)
                .map(|v| format!("{v:.6}"))
                .collect();
            println!("  {} = [{}]", vec.name, preview.join(", "));
        }
    }
}

/// Detect Cirq source files by extension.
#[cfg(not(target_arch = "wasm32"))]
fn is_cirq_file(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| ext == "cirq")
}

#[cfg(test)]
mod tests {
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    use thevenin_types::VectorData;

    /// Check that two f64 values are approximately equal (absolute or relative).
    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9 || (a - b).abs() / a.abs().max(1e-15) < 1e-6
    }

    /// Simulate SPICE source through the IR pipeline twice (once via
    /// `Netlist::parse_single` + `import_netlist`, once via
    /// `import_spice`) and assert both produce identical results.
    ///
    /// Historically this compared the IR path against a Netlist-shaped
    /// direct path, but the latter is `pub(crate)` post Stage 4; both legs
    /// now exit through `thevenin::circuit::simulate`, so the assertion
    /// exercises SPICE-parse fidelity rather than two distinct simulators.
    fn assert_roundtrip(spice: &str) {
        let legacy_netlist = thevenin_types::Netlist::parse_single(spice).unwrap();
        let mut resolved = legacy_netlist.clone();
        thevenin::expr::resolve_netlist_exprs(&mut resolved).unwrap();
        let legacy_circuit = cirq_spice_import::import_netlist(&resolved).unwrap();
        let legacy_result = thevenin::circuit::simulate(&legacy_circuit).unwrap();

        let circuits = cirq_spice_import::import_spice(spice).unwrap();
        assert_eq!(
            circuits.len(),
            1,
            "expected one circuit from IR path, got {}",
            circuits.len()
        );
        let ir_result = thevenin::circuit::simulate(&circuits[0]).unwrap();

        assert_eq!(
            legacy_result.plots.len(),
            ir_result.plots.len(),
            "plot count mismatch"
        );
        for (lp, ip) in legacy_result.plots.iter().zip(ir_result.plots.iter()) {
            assert_eq!(
                lp.vecs.len(),
                ip.vecs.len(),
                "vector count mismatch in plot '{}' vs '{}'",
                lp.name,
                ip.name
            );
            for (lv, iv) in lp.vecs.iter().zip(ip.vecs.iter()) {
                match (&lv.data, &iv.data) {
                    (VectorData::Real(ld), VectorData::Real(id)) => {
                        assert_eq!(
                            ld.len(),
                            id.len(),
                            "length mismatch: '{}' vs '{}'",
                            lv.name,
                            iv.name
                        );
                        for (i, (a, b)) in ld.iter().zip(id.iter()).enumerate() {
                            assert!(
                                approx_eq(*a, *b),
                                "real mismatch at {i} for '{}' vs '{}': {a} != {b}",
                                lv.name,
                                iv.name
                            );
                        }
                    }
                    (VectorData::Complex(ld), VectorData::Complex(id)) => {
                        assert_eq!(
                            ld.len(),
                            id.len(),
                            "length mismatch: '{}' vs '{}'",
                            lv.name,
                            iv.name
                        );
                        for (i, (a, b)) in ld.iter().zip(id.iter()).enumerate() {
                            assert!(
                                approx_eq(a.re, b.re) && approx_eq(a.im, b.im),
                                "complex mismatch at {i} for '{}' vs '{}': ({}, {}) != ({}, {})",
                                lv.name,
                                iv.name,
                                a.re,
                                a.im,
                                b.re,
                                b.im
                            );
                        }
                    }
                    _ => panic!(
                        "data type mismatch for '{}' vs '{}': one real, one complex",
                        lv.name, iv.name
                    ),
                }
            }
        }
    }

    #[test]
    fn roundtrip_voltage_divider_op() {
        assert_roundtrip(
            "Voltage Divider\n\
             V1 in 0 1.0\n\
             R1 in mid 1k\n\
             R2 mid 0 2k\n\
             .op\n\
             .end\n",
        );
    }

    #[test]
    fn roundtrip_rc_transient() {
        assert_roundtrip(
            "RC Transient\n\
             V1 in 0 PULSE(0 1 0 1n 1n 50n 100n)\n\
             R1 in out 1k\n\
             C1 out 0 1p\n\
             .tran 0.1n 200n\n\
             .end\n",
        );
    }

    #[test]
    fn roundtrip_ac_rlc() {
        assert_roundtrip(
            "RLC AC\n\
             V1 in 0 AC 1\n\
             R1 in mid 100\n\
             L1 mid out 1m\n\
             C1 out 0 1u\n\
             .ac dec 10 1 1meg\n\
             .end\n",
        );
    }

    #[test]
    fn roundtrip_diode_op() {
        assert_roundtrip(
            "Diode Test\n\
             V1 in 0 0.7\n\
             D1 in 0 DMOD\n\
             .model DMOD D IS=1e-14\n\
             .op\n\
             .end\n",
        );
    }

    #[test]
    fn roundtrip_dc_sweep() {
        assert_roundtrip(
            "DC Sweep\n\
             V1 in 0 1.0\n\
             R1 in out 1k\n\
             R2 out 0 1k\n\
             .dc V1 0 5 0.1\n\
             .end\n",
        );
    }

    #[test]
    fn roundtrip_bjt_amplifier() {
        assert_roundtrip(
            "BJT Common Emitter\n\
             VCC vcc 0 5\n\
             VIN in 0 0.7\n\
             RC vcc out 1k\n\
             RB in base 10k\n\
             Q1 out base 0 QMOD\n\
             .model QMOD NPN BF=100 IS=1e-15\n\
             .op\n\
             .end\n",
        );
    }

    #[test]
    fn roundtrip_mosfet_inverter() {
        assert_roundtrip(
            "MOSFET Inverter\n\
             VDD vdd 0 3.3\n\
             VIN in 0 1.65\n\
             M1 out in vdd vdd PMOD W=10u L=1u\n\
             M2 out in 0 0 NMOD W=5u L=1u\n\
             .model NMOD NMOS VTO=0.7 KP=110u\n\
             .model PMOD PMOS VTO=-0.7 KP=55u\n\
             .op\n\
             .end\n",
        );
    }

    #[test]
    fn roundtrip_subcircuit() {
        assert_roundtrip(
            "Subcircuit Test\n\
             .subckt DIVIDER in out\n\
             R1 in out 1k\n\
             R2 out 0 1k\n\
             .ends DIVIDER\n\
             V1 vin 0 10\n\
             X1 vin vout DIVIDER\n\
             .op\n\
             .end\n",
        );
    }

    #[test]
    fn roundtrip_mutual_inductor() {
        assert_roundtrip(
            "Mutual Inductor\n\
             V1 in 0 PULSE(0 1 0 1n 1n 50n 100n)\n\
             R1 in n1 50\n\
             L1 n1 0 10u\n\
             L2 n2 0 10u\n\
             K1 L1 L2 0.99\n\
             R2 n2 0 50\n\
             .tran 1n 200n\n\
             .end\n",
        );
    }

    #[test]
    fn roundtrip_options_and_temp() {
        assert_roundtrip(
            "Options Test\n\
             V1 in 0 1\n\
             R1 in 0 1k\n\
             .options GMIN=1e-13 RELTOL=1e-4\n\
             .temp 50\n\
             .op\n\
             .end\n",
        );
    }

    #[test]
    fn roundtrip_vcvs_dependent_source() {
        assert_roundtrip(
            "VCVS Test\n\
             V1 in 0 1\n\
             R1 in 0 1k\n\
             E1 out 0 in 0 10\n\
             R2 out 0 10k\n\
             .op\n\
             .end\n",
        );
    }
}
