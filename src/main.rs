use std::io::{self, Read, Write};

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
    /// Convert a SPICE netlist to CirQ format.
    Convert {
        /// Input SPICE file (reads stdin if omitted).
        #[facet(args::positional, default)]
        input: Option<String>,

        /// Output file (writes to stdout if omitted).
        #[facet(args::named, args::short = 'o')]
        output: Option<String>,

        /// Output format: yaml (default) or json.
        #[facet(args::named, args::short = 'f', default = "yaml")]
        format: String,
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
        Command::Convert {
            input,
            output,
            format,
        } => convert(input, output, &format),
    }
}

fn convert(
    input: Option<String>,
    output: Option<String>,
    format: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Read input
    let spice = match &input {
        Some(path) if path != "-" => {
            std::fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))?
        }
        _ => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("failed to read stdin: {e}"))?;
            buf
        }
    };

    // Parse SPICE → IR
    let circuit = thevenin_cirq::from_spice::from_spice(&spice)?;

    // Serialize IR → CirQ
    let result = match format {
        "json" => thevenin_cirq::to_cirq::to_json(&circuit)?,
        _ => thevenin_cirq::to_cirq::to_yaml(&circuit)?,
    };

    // Write output
    match &output {
        Some(path) => {
            std::fs::write(path, &result).map_err(|e| format!("failed to write {path}: {e}"))?;
        }
        None => {
            io::stdout()
                .write_all(result.as_bytes())
                .map_err(|e| format!("failed to write stdout: {e}"))?;
        }
    }

    Ok(())
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
