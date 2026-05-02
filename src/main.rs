use std::path::Path;

use facet::Facet;
use figue::{self as args, FigueBuiltins};

#[derive(Facet)]
struct Cli {
    #[facet(args::subcommand)]
    command: Command,

    #[facet(flatten)]
    builtins: FigueBuiltins,
}

#[derive(Facet)]
#[repr(u8)]
#[allow(dead_code)]
enum Command {
    /// Run a circuit simulation.
    Run {
        /// Input file (.cirq or SPICE netlist).
        #[facet(args::positional)]
        input: String,

        /// Bypass the Cirq IR pipeline and use the legacy SPICE parser directly.
        #[facet(args::named)]
        legacy: bool,
    },
}

fn main() {
    let cli: Cli = figue::from_std_args().unwrap();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Run { input, legacy } => {
            let src = std::fs::read_to_string(&input)
                .map_err(|e| format!("failed to read {input}: {e}"))?;

            let netlists = if is_cirq_file(&input) {
                if legacy {
                    return Err("--legacy is not supported for .cirq files".into());
                }
                let base_dir = Path::new(&input)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_owned();
                cirq_frontend::compile_file_to_netlist(&src, &base_dir).map_err(|diags| {
                    let msgs: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
                    msgs.join("\n")
                })?
            } else if legacy {
                // Legacy path: parse SPICE directly into Netlist, bypass IR.
                vec![thevenin_types::Netlist::parse_single(&src)?]
            } else {
                // Default path: route SPICE through Cirq IR.
                spice_through_ir(&src)?
            };

            for netlist in &netlists {
                if thevenin_control::has_control_block(netlist) {
                    let ctrl_result = thevenin_control::execute_control_block(netlist)
                        .map_err(|e| format!("control block error: {e}"))?;

                    if !ctrl_result.output.is_empty() {
                        print!("{}", ctrl_result.output);
                    }

                    for plot in &ctrl_result.sim_result.plots {
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
                } else {
                    let result = thevenin::simulate(netlist)?;
                    for plot in &result.plots {
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
            }
            Ok(())
        }
    }
}

/// Route SPICE source text through the Cirq IR pipeline.
///
/// Parses SPICE → imports into Cirq IR → converts back to Netlist for simulation.
/// This validates the IR round-trip and is the default path for SPICE files.
fn spice_through_ir(
    source: &str,
) -> Result<Vec<thevenin_types::Netlist>, Box<dyn std::error::Error>> {
    let circuits = cirq_spice_import::import_spice(source)?;
    let mut all_netlists = Vec::new();
    for circuit in &circuits {
        let netlists = cirq_frontend::to_netlist::circuit_to_netlists(circuit)
            .map_err(|e| format!("IR-to-netlist conversion failed: {e}"))?;
        all_netlists.extend(netlists);
    }
    Ok(all_netlists)
}

/// Detect Cirq source files by extension.
fn is_cirq_file(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| ext == "cirq")
}

#[cfg(test)]
mod tests {
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    use super::spice_through_ir;
    use thevenin_types::VectorData;

    /// Check that two f64 values are approximately equal (absolute or relative).
    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9 || (a - b).abs() / a.abs().max(1e-15) < 1e-6
    }

    /// Simulate SPICE source via legacy (direct) and IR paths, assert results match.
    fn assert_roundtrip(spice: &str) {
        let legacy_netlist = thevenin_types::Netlist::parse_single(spice).unwrap();
        let legacy_result = thevenin::simulate(&legacy_netlist).unwrap();

        let ir_netlists = spice_through_ir(spice).unwrap();
        assert_eq!(
            ir_netlists.len(),
            1,
            "expected one netlist from IR path, got {}",
            ir_netlists.len()
        );
        let ir_result = thevenin::simulate(&ir_netlists[0]).unwrap();

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
