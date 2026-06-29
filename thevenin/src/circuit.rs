//! The canonical simulation surface: run analyses on a [`cirq_ir::Circuit`].
//!
//! Every entry point here takes a name-resolved, parameter-evaluated
//! [`cirq_ir::Circuit`] (produced by [`cirq_frontend`] from Cirq source, or by
//! [`cirq_spice_import`](https://docs.rs/cirq-spice-import) from a SPICE
//! netlist) and returns a [`SimResult`] of named result plots.
//!
//! [`SimResult`]: thevenin_types::SimResult
//!
//! - [`simulate`] — top-level driver. Runs **every** analysis the circuit
//!   declares, in order, applies multi-temperature sweeps, and evaluates any
//!   `measure` declarations. This is the usual entry point.
//! - [`simulate_op`], [`simulate_dc`], [`simulate_tran`], [`simulate_ac`],
//!   [`simulate_noise`], [`simulate_sens`], [`simulate_pz`], [`simulate_tf`] —
//!   run a single analysis of that kind, selecting the first matching analysis
//!   the circuit declares.
//! - [`simulate_four`] / [`simulate_fft`] — Fourier / FFT post-processing of a
//!   preceding transient.
//!
//! # How it runs
//!
//! The MNA system is assembled **directly from the IR** via
//! [`crate::mna_ir::assemble_mna_from_circuit`] — there is no SPICE netlist in
//! the loop. Every `IrElementKind` is supported, so assembly never falls back
//! to a netlist-shaped path (the legacy `assemble_mna(&Netlist)` stamper has
//! been removed).
//!
//! # Example
//!
//! ```
//! use thevenin::circuit::simulate;
//!
//! // A resistive divider written in Cirq source, compiled to IR.
//! let circuit = cirq_frontend::compile(
//!     "circuit divider {
//!          V1: vsource(in -> gnd, dc: 1.0)
//!          R1: resistor(in -> mid, 1k)
//!          R2: resistor(mid -> gnd, 2k)
//!          analysis op {}
//!      }",
//! )
//! .expect("compiles");
//!
//! let result = simulate(&circuit).expect("simulates");
//! assert!(!result.plots.is_empty());
//! ```

use std::sync::Arc;

use cirq_ir::Circuit;
use thevenin_types::{SimPlot, SimResult};
use thevenin_xspice::CodeModelRegistry;

use crate::MnaError;
use crate::mna_ir;

/// Errors that can occur when simulating a [`Circuit`] directly.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CircuitSimError {
    #[error("simulation failed: {0}")]
    Mna(#[from] MnaError),
}

/// Assemble an [`MnaSystem`] directly from the IR via
/// [`mna_ir::assemble_mna_from_circuit`]. Every `IrElementKind` is supported,
/// so `Ok(None)` is unreachable in practice and is surfaced as an error rather
/// than silently falling back.
fn assemble(circuit: &Circuit) -> Result<crate::mna::MnaSystem, CircuitSimError> {
    mna_ir::assemble_mna_from_circuit(circuit, false, None)?.ok_or_else(|| {
        CircuitSimError::Mna(MnaError::UnsupportedElement(
            "circuit contains an element the IR assembler cannot stamp".to_string(),
        ))
    })
}

/// Compute the DC operating point with an XSPICE code model registry.
///
/// Assembles MNA directly from the IR through
/// [`mna_ir::assemble_mna_from_circuit`] with the registry threaded
/// through, then solves with default NR options and no nodeset —
/// preserving the historical XSPICE-OP behaviour. The result is
/// formatted via the shared `simulate_op_with_mna` so output shape stays
/// canonical.
pub fn simulate_op_with_xspice(
    circuit: &Circuit,
    registry: Arc<CodeModelRegistry>,
) -> Result<SimResult, CircuitSimError> {
    let mna =
        mna_ir::assemble_mna_from_circuit(circuit, false, Some(registry))?.ok_or_else(|| {
            CircuitSimError::Mna(MnaError::UnsupportedElement(
                "circuit not representable in mna_ir for XSPICE OP".to_string(),
            ))
        })?;
    Ok(crate::simulate::simulate_op_with_mna(
        &mna,
        &crate::newton::NrOptions::default(),
        &[],
    )?)
}

/// Run a DC operating-point analysis on the circuit.
///
/// The MNA system is assembled directly from the IR; the solve and result
/// formatting route through [`crate::simulate::simulate_op_with_mna`] so the
/// output shape is canonical.
pub fn simulate_op(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let mna = assemble(circuit)?;
    let opts = mna_ir::nr_options_from_circuit(circuit);
    let nodeset = mna_ir::resolve_nodeset_from_circuit(circuit, &mna);
    Ok(crate::simulate::simulate_op_with_mna(
        &mna, &opts, &nodeset,
    )?)
}

/// Run a DC sweep on the circuit's first declared `.dc` analysis.
///
/// Fully Circuit-driven: NR options come from `circuit.options`, sweep params
/// from `circuit.analyses`, and the MnaSystem is built directly from IR.
pub fn simulate_dc(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let mna = assemble(circuit)?;
    let mut nr_opts = mna_ir::nr_options_from_circuit(circuit);
    nr_opts.diag_gmin = 0.0;
    let params = mna_ir::dc_sweep_params_from_circuit(circuit)?;
    Ok(crate::simulate::run_dc_sweep(mna, nr_opts, params)?)
}

/// Run a transient analysis on the circuit's first declared `.tran` analysis.
///
/// As with the harness, an operating-point solve is prepended so the
/// transient starts from a valid steady state. Fully Circuit-driven: NR
/// options, nodeset, `.ic` overrides, `.print` queries, and `.tran` analysis
/// params all come from the IR Circuit.
pub fn simulate_tran(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let mna_op = assemble(circuit)?;
    let opts = mna_ir::nr_options_from_circuit(circuit);
    let nodeset = mna_ir::resolve_nodeset_from_circuit(circuit, &mna_op);
    let mut plots: Vec<SimPlot> = Vec::new();
    if let Ok(op) = crate::simulate::simulate_op_with_mna(&mna_op, &opts, &nodeset) {
        plots.extend(op.plots);
    }
    let mna_tran = assemble(circuit)?;
    let params = mna_ir::tran_params_from_circuit(circuit, &mna_tran)?;
    plots.extend(
        crate::transient::run_tran(mna_tran, params)?
            .into_result()
            .plots,
    );
    Ok(SimResult { plots })
}

/// Run an AC small-signal analysis on the circuit's first declared `.ac`.
///
/// Fully Circuit-driven: NR options, nodeset, AC source excitations, and
/// `.ac` sweep params all come from the IR Circuit.
pub fn simulate_ac(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let mna = assemble(circuit)?;
    let params = mna_ir::ac_sweep_params_from_circuit(circuit, &mna)?;
    Ok(crate::ac::run_ac_sweep(mna, params)?)
}

/// Top-level dispatcher: run every analysis declared on `circuit.analyses`
/// and concatenate the result plots.
///
/// Covers all eight core analysis kinds (op / dc / tran / ac / noise / sens /
/// pz / tf) plus Fourier/FFT post-processing. This is the recommended entry
/// point; the historical netlist-shaped simulator is now a crate-internal
/// implementation detail.
///
/// Multi-temperature sweeps (`circuit.temps.len() > 1`) re-run every
/// analysis at each temperature and label the resulting plots with
/// `{plot_name}_temp{index}_{temp}`.
pub fn simulate(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let mut result = if circuit.temps.len() > 1 {
        simulate_multi_temp(circuit, &circuit.temps)?
    } else {
        simulate_single(circuit)?
    };
    evaluate_circuit_measurements(circuit, &mut result);
    Ok(result)
}

/// Run any `.meas` directives declared on the circuit against the simulation
/// result, appending a `"measurements"` plot. Dispatches directly to the
/// typed evaluator — the Circuit already carries parsed `MeasureExpr` values
/// alongside the verbatim spec strings.
fn evaluate_circuit_measurements(circuit: &Circuit, result: &mut SimResult) {
    crate::measure::evaluate_circuit_measures(&circuit.measures, result);
}

/// Run every analysis declared on `circuit.analyses` once and concatenate
/// the resulting plots. The multi-temperature wrapper [`simulate`] dispatches
/// here per temperature.
fn simulate_single(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let mut plots = Vec::new();
    let analyses = if circuit.analyses.is_empty() {
        std::borrow::Cow::Owned(vec![cirq_ir::Analysis::Op])
    } else {
        std::borrow::Cow::Borrowed(&circuit.analyses)
    };
    for analysis in analyses.iter() {
        let result = match analysis {
            cirq_ir::Analysis::Op => simulate_op(circuit)?,
            cirq_ir::Analysis::Dc(_) => simulate_dc(circuit)?,
            cirq_ir::Analysis::Tran(_) => simulate_tran(circuit)?,
            cirq_ir::Analysis::Ac(_) => simulate_ac(circuit)?,
            cirq_ir::Analysis::Noise(_) => simulate_noise(circuit)?,
            cirq_ir::Analysis::Sens(_) => simulate_sens(circuit)?,
            cirq_ir::Analysis::Pz(_) => simulate_pz(circuit)?,
            cirq_ir::Analysis::Tf(_) => simulate_tf(circuit)?,
            cirq_ir::Analysis::Four(four) => simulate_four(circuit, four)?,
            cirq_ir::Analysis::Fft(fft) => simulate_fft(circuit, fft)?,
            // `Analysis` is `#[non_exhaustive]` — new analyses (`.disto`,
            // etc.) must grow an explicit arm before they can be
            // simulated. Until then return an error rather than panic.
            _ => {
                return Err(CircuitSimError::Mna(MnaError::UnsupportedElement(
                    "unknown analysis variant — extend thevenin::circuit::simulate".to_string(),
                )));
            }
        };
        plots.extend(result.plots);
    }
    Ok(SimResult { plots })
}

/// Run `.four` Fourier post-processing.
///
/// `.four` is post-processing on a transient run. We require the circuit
/// to declare a `.tran` analysis (the SPICE convention); we run that
/// transient, then compute the harmonic table on its plot.
pub fn simulate_four(
    circuit: &Circuit,
    four: &cirq_ir::FourAnalysis,
) -> Result<SimResult, CircuitSimError> {
    let tran_result = simulate_tran(circuit)?;
    let tran_plot = tran_result
        .plots
        .iter()
        .find(|p| p.name.starts_with("tran"))
        .ok_or_else(|| {
            CircuitSimError::Mna(MnaError::UnsupportedElement(
                ".four needs a .tran analysis to post-process".to_string(),
            ))
        })?;
    let vec_refs: Vec<&str> = four.vectors.iter().map(String::as_str).collect();
    let results =
        crate::fourier::four_analysis(tran_plot, four.fundamental, &vec_refs, four.num_harmonics)
            .map_err(|e| CircuitSimError::Mna(MnaError::UnsupportedElement(e.to_string())))?;
    Ok(SimResult {
        plots: vec![four_to_plot(&results)],
    })
}

/// Run `.fft` Fourier post-processing.
pub fn simulate_fft(
    circuit: &Circuit,
    fft: &cirq_ir::FftAnalysis,
) -> Result<SimResult, CircuitSimError> {
    let tran_result = simulate_tran(circuit)?;
    let tran_plot = tran_result
        .plots
        .iter()
        .find(|p| p.name.starts_with("tran"))
        .ok_or_else(|| {
            CircuitSimError::Mna(MnaError::UnsupportedElement(
                ".fft needs a .tran analysis to post-process".to_string(),
            ))
        })?;
    let opts = crate::fourier::FftOptions {
        vectors: fft.vectors.clone(),
        start: fft.start,
        stop: fft.stop,
        npoints: fft.npoints,
        window: fft.window,
        format: fft.format,
    };
    let results = crate::fourier::fft_analysis(tran_plot, &opts)
        .map_err(|e| CircuitSimError::Mna(MnaError::UnsupportedElement(e.to_string())))?;
    Ok(SimResult {
        plots: vec![fft_to_plot(&results)],
    })
}

fn four_to_plot(results: &[crate::fourier::FourResult]) -> SimPlot {
    use thevenin_types::SimVector;
    // For each vector we emit four real vectors: <name>_freq, <name>_mag,
    // <name>_phase, <name>_norm. The DC component is index 0 of each.
    let mut vecs = Vec::new();
    for r in results {
        let mut freqs = vec![0.0];
        let mut mags = vec![r.dc.abs()];
        let mut phases = vec![0.0];
        let mut norms = vec![0.0];
        for h in &r.harmonics {
            freqs.push(h.frequency);
            mags.push(h.magnitude);
            phases.push(h.phase_deg);
            norms.push(h.normalised);
        }
        let prefix = sanitize_vec_name(&r.vector);
        vecs.push(SimVector::real(format!("{prefix}_freq"), freqs));
        vecs.push(SimVector::real(format!("{prefix}_mag"), mags));
        vecs.push(SimVector::real(format!("{prefix}_phase"), phases));
        vecs.push(SimVector::real(format!("{prefix}_norm"), norms));
    }
    SimPlot {
        name: "fourier1".to_string(),
        vecs,
    }
}

fn fft_to_plot(results: &[crate::fourier::FftResult]) -> SimPlot {
    use thevenin_types::SimVector;
    let mut vecs = Vec::new();
    for r in results {
        let prefix = sanitize_vec_name(&r.vector);
        vecs.push(SimVector::real(
            format!("{prefix}_freq"),
            r.frequencies.clone(),
        ));
        vecs.push(SimVector::complex(
            format!("{prefix}_fft"),
            r.values.clone(),
        ));
    }
    SimPlot {
        name: "fft1".to_string(),
        vecs,
    }
}

fn sanitize_vec_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Run every analysis at each requested temperature, labelling plots so the
/// caller can distinguish sweep points. Mirrors the Netlist-side
/// `simulate_multi_temp` so a Circuit lowered from a SPICE `.temp 25 50 100`
/// netlist produces the same `{name}_temp{i}_{temp}` plot naming.
fn simulate_multi_temp(circuit: &Circuit, temps: &[f64]) -> Result<SimResult, CircuitSimError> {
    let mut plots = Vec::with_capacity(temps.len() * circuit.analyses.len().max(1));
    for (i, &temp) in temps.iter().enumerate() {
        let mut single_temp = circuit.clone();
        single_temp.temps = vec![temp];
        let result = simulate_single(&single_temp)?;
        for mut plot in result.plots {
            plot.name = format!("{}_temp{}_{}", plot.name, i + 1, temp);
            plots.push(plot);
        }
    }
    Ok(SimResult { plots })
}

/// Run a transfer function (`.tf`) analysis on a Circuit, assembled directly
/// from the IR.
pub fn simulate_tf(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let mna = assemble(circuit)?;
    let (output, input) = mna_ir::tf_spec_from_circuit(circuit)?;
    Ok(crate::tf::run_tf(mna, &output, &input)?)
}

/// Run a pole-zero (`.pz`) analysis on a Circuit. Fully Netlist-free on
/// the happy path.
pub fn simulate_pz(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let mna = assemble(circuit)?;
    let params = mna_ir::pz_params_from_circuit(circuit)?;
    Ok(crate::pz::run_pz(mna, params)?)
}

/// Run a noise analysis (`.noise`) on a Circuit. Fully Netlist-free on
/// the happy path.
pub fn simulate_noise(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let mna = assemble(circuit)?;
    let params = mna_ir::noise_params_from_circuit(circuit, &mna)?;
    Ok(crate::noise::run_noise(mna, params)?)
}

/// Run a sensitivity (`.sens`) analysis on a Circuit. Fully Netlist-free on
/// the happy path.
///
/// The IR's [`cirq_ir::SensAnalysis`] carries the output spec as a single
/// token plus an optional typed [`cirq_ir::SensAcSpec`] for the AC variant —
/// the Netlist's tokenized `Vec<String>` is reconstructed by the emitter
/// only when the Netlist path is taken.
pub fn simulate_sens(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let mna = assemble(circuit)?;
    let params = mna_ir::sens_params_from_circuit(circuit, &mna)?;
    Ok(crate::sens::run_sens(mna, params)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cirq_ir::{
        Analysis as IrAnalysis, Connection, Element, ElementKind, Id, Net, ResolvedParam,
        SourceSpec, TranAnalysis, Value,
    };

    fn voltage_divider() -> Circuit {
        Circuit {
            name: "voltage_divider".into(),
            nets: vec![
                Net {
                    id: Id(0),
                    name: "gnd".into(),
                    is_global: true,
                },
                Net {
                    id: Id(1),
                    name: "in".into(),
                    is_global: false,
                },
                Net {
                    id: Id(2),
                    name: "mid".into(),
                    is_global: false,
                },
            ],
            elements: vec![
                Element {
                    id: Id(0),
                    name: "V1".into(),
                    kind: ElementKind::VoltageSource,
                    connections: vec![
                        Connection {
                            terminal: "pos".into(),
                            net: Id(1),
                        },
                        Connection {
                            terminal: "neg".into(),
                            net: Id(0),
                        },
                    ],
                    params: vec![],
                    model: None,
                    source_spec: Some(SourceSpec {
                        dc: Some(1.0),
                        ac: None,
                        waveform: None,
                    }),
                },
                Element {
                    id: Id(1),
                    name: "R1".into(),
                    kind: ElementKind::Resistor,
                    connections: vec![
                        Connection {
                            terminal: "pos".into(),
                            net: Id(1),
                        },
                        Connection {
                            terminal: "neg".into(),
                            net: Id(2),
                        },
                    ],
                    params: vec![("value".into(), Value::Real(1_000.0))],
                    model: None,
                    source_spec: None,
                },
                Element {
                    id: Id(2),
                    name: "R2".into(),
                    kind: ElementKind::Resistor,
                    connections: vec![
                        Connection {
                            terminal: "pos".into(),
                            net: Id(2),
                        },
                        Connection {
                            terminal: "neg".into(),
                            net: Id(0),
                        },
                    ],
                    params: vec![("value".into(), Value::Real(2_000.0))],
                    model: None,
                    source_spec: None,
                },
            ],
            models: vec![],
            analyses: vec![IrAnalysis::Op],
            params: Vec::<ResolvedParam>::new(),
            csparams: Vec::<ResolvedParam>::new(),
            options: vec![],
            temps: vec![],
            save: vec![],
            funcs: vec![],
            initial_conditions: vec![],
            nodeset: vec![],
            measures: vec![],
            code_blocks: vec![],
            raw_directives: vec![],
        }
    }

    #[test]
    fn op_voltage_divider() {
        let result = simulate_op(&voltage_divider()).expect("op");
        let v_mid = result.plots[0]
            .vecs
            .iter()
            .find(|v| v.name == "v(mid)")
            .expect("v(mid)");
        let v = match &v_mid.data {
            thevenin_types::VectorData::Real(r) => r[0],
            _ => panic!(),
        };
        assert!((v - 2.0 / 3.0).abs() < 1e-6, "v(mid) = {v}");
    }
}
