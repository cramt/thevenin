//! AC frequency response of a low-pass RC filter.
//!
//! Circuit: V1 (AC=1) -> R1 (1kΩ) -> node "out" -> C1 (1µF) -> GND
//! Cutoff frequency: f_c = 1/(2π·R·C) ≈ 159 Hz

use cirq_spice_import::import_spice;
use thevenin::circuit::simulate;

fn main() {
    let circuit = import_spice(
        "\
RC Low-Pass Filter
V1 in 0 DC 0 AC 1
R1 in out 1k
C1 out 0 1u
.ac dec 10 1 100k
.end
",
    )
    .expect("failed to parse SPICE source")
    .pop()
    .expect("expected at least one circuit");

    let result = simulate(&circuit).expect("simulation failed");

    let freq = result["frequency"].data.as_real();
    let vout = result["v(out)"].data.as_complex();

    println!("=== RC Low-Pass Filter Frequency Response ===");
    println!(
        "{:>12}  {:>12}  {:>10}",
        "Freq (Hz)", "|V(out)|", "Phase (deg)"
    );
    println!(
        "{:>12}  {:>12}  {:>10}",
        "---------", "--------", "---------"
    );
    for (f, c) in freq.iter().zip(vout) {
        println!(
            "{f:>12.1}  {:>12.6}  {:>10.2}",
            c.magnitude(),
            c.phase_deg()
        );
    }
}
