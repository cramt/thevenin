//! Transient analysis: pulse response of an RC circuit.
//!
//! A 1V pulse drives a 1kΩ + 10µF RC network.
//! Time constant τ = R·C = 10ms.

use cirq_spice_import::import_spice;
use thevenin::circuit::simulate;

fn main() {
    let circuit = import_spice(
        "\
RC Pulse Response
V1 in 0 PULSE(0 1 0 1n 1n 50m 100m)
R1 in out 1k
C1 out 0 10u
.tran 1m 100m
.end
",
    )
    .expect("failed to parse SPICE source")
    .pop()
    .expect("expected at least one circuit");

    let result = simulate(&circuit).expect("simulation failed");

    let time = result["time"].data.as_real();
    let vout = result["v(out)"].data.as_real();

    println!("=== RC Pulse Response ===");
    println!("{:>10}  {:>10}", "Time (s)", "V(out)");
    println!("{:>10}  {:>10}", "--------", "------");

    // Print every 10th point to keep output manageable
    for (i, (t, v)) in time.iter().zip(vout).enumerate() {
        if i % 10 == 0 {
            println!("{t:>10.4e}  {v:>10.6}");
        }
    }
}
