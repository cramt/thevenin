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
        Command::Run { input } => {
            let src = std::fs::read_to_string(&input)
                .map_err(|e| format!("failed to read {input}: {e}"))?;

            let netlists = if is_cirq_file(&input) {
                let base_dir = Path::new(&input)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_owned();
                cirq_frontend::compile_file_to_netlist(&src, &base_dir).map_err(|diags| {
                    let msgs: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
                    msgs.join("\n")
                })?
            } else {
                vec![thevenin_types::Netlist::parse_single(&src)?]
            };

            for netlist in &netlists {
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
            Ok(())
        }
    }
}

/// Detect Cirq source files by extension.
fn is_cirq_file(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext == "cirq")
}

#[cfg(test)]
mod tests {
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn test_hello() {
        assert_eq!(1 + 1, 2);
    }
}
