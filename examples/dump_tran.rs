//! Debug utility: run a SPICE deck and dump all real vectors as TSV.
//! Usage: cargo run --release --example dump_tran -- <deck.cir>

use cirq_spice_import::import_spice;
use thevenin::circuit::simulate;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_tran <deck>");
    let src = std::fs::read_to_string(&path).expect("read deck");
    let circuits = import_spice(&src).expect("import");
    for circuit in &circuits {
        let result = simulate(circuit).expect("simulate");
        for plot in &result.plots {
            println!("# plot {}", plot.name);
            let names: Vec<&str> = plot.vecs.iter().map(|v| v.name.as_str()).collect();
            println!("# {}", names.join("\t"));
            let cols: Vec<Vec<f64>> = plot
                .vecs
                .iter()
                .map(|v| v.data.try_real().map(|r| r.to_vec()).unwrap_or_default())
                .collect();
            let n = cols.iter().map(|c| c.len()).max().unwrap_or(0);
            for i in 0..n {
                let row: Vec<String> = cols
                    .iter()
                    .map(|c| c.get(i).map(|v| format!("{v:.9e}")).unwrap_or_default())
                    .collect();
                println!("{}", row.join("\t"));
            }
        }
    }
}
