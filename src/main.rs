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
    /// Run a SPICE simulation.
    Run {
        /// Input SPICE netlist file.
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
            let netlist = thevenin_types::Netlist::parse_single(&src)?;
            let result = thevenin::simulate(&netlist)?;
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
            Ok(())
        }
    }
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
