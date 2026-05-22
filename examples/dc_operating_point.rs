//! DC operating point analysis of a simple resistive voltage divider.
//!
//! Circuit:
//!   V1 (1V) -> R1 (1kΩ) -> node "mid" -> R2 (2kΩ) -> GND
//!
//! Expected: V(mid) = 1V * 2k/(1k+2k) ≈ 0.667V

use cirq_spice_import::import_spice;
use thevenin::circuit::simulate;

fn main() {
    let circuit = import_spice(
        "\
Voltage Divider
V1 in 0 1.0
R1 in mid 1k
R2 mid 0 2k
.op
.end
",
    )
    .expect("failed to parse SPICE source")
    .pop()
    .expect("expected at least one circuit");

    let result = simulate(&circuit).expect("simulation failed");

    println!("=== DC Operating Point ===");
    let plot = result.plot().expect("no plot");
    for vec in plot.voltages() {
        println!("  {:>20} = {:>12.6} ", vec.name, vec.data.as_real()[0]);
    }
    for vec in plot.currents() {
        println!("  {:>20} = {:>12.6e}", vec.name, vec.data.as_real()[0]);
    }
}
