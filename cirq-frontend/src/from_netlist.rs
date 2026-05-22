//! Convert a parsed-Netlist analysis directive to the Cirq IR shape.
//!
//! `thevenin_types::Analysis` is the shape produced by the SPICE parser and
//! the `.control` interpreter's `parse_analysis_command`. It carries source
//! and node references as `String` names. `cirq_ir::Analysis` carries them
//! as `Id`s into the surrounding [`Circuit`]. This module is the bridge:
//! given a parsed Analysis and the Circuit it should run against, produce
//! the IR-shape Analysis (with names resolved to Ids and `Expr`s resolved
//! to `f64`).
//!
//! The converter is the keystone for retiring the cached Netlist inside
//! `thevenin-control`: once `.control` can set a parsed Analysis directly
//! on a `Circuit` clone, the analysis-dispatch sites can move to
//! `circuit::simulate_*(&Circuit)` and the Netlist cache disappears. See
//! `docs/migration/old-path-retirement-checklist.md` for the broader plan.
//!
//! All inputs are expected to come from `parse_analysis_command` at
//! runtime, where the `Expr` fields are bare numeric literals
//! (`Expr::Num`). Unresolved param/brace expressions are not supported
//! here — they would belong to a separate, IR-driven analysis builder.

use cirq_ir::{Circuit, Id};
use thiserror::Error;

/// Failures that can arise when lifting a parsed-Netlist analysis to its
/// IR equivalent.
#[derive(Debug, Error)]
pub enum AnalysisConversionError {
    #[error("unknown net `{0}` (analysis references a node not in the circuit)")]
    UnknownNet(String),
    #[error("unknown source `{0}` (analysis references an element not in the circuit)")]
    UnknownSource(String),
    #[error("analysis field `{field}` is not a numeric literal: {expr}")]
    NonNumericExpr { field: String, expr: String },
    #[error("`.sens` requires at least one output expression")]
    EmptySensOutputs,
    #[error("`.sens` AC tail: expected AC|DC marker after output, got `{0}`")]
    SensAcMarker(String),
    #[error("`.sens` AC tail: needs variation n fstart fstop, got `{tokens:?}`")]
    SensAcArity { tokens: Vec<String> },
    #[error("`.sens` AC tail: unknown variation `{0}`")]
    SensAcVariation(String),
    #[error("`.sens` AC tail: bad {field}: `{token}`")]
    SensAcNumber { field: &'static str, token: String },
}

/// Lift a parsed-Netlist [`Analysis`](thevenin_types::Analysis) into its
/// [`cirq_ir::Analysis`] equivalent, resolving source / node names to
/// `Id`s through `circuit`.
///
/// `Expr` fields must already be `Expr::Num` (the `.control` command
/// parser produces only literals; deferred parametric expressions are
/// not supported on this path).
pub fn netlist_analysis_to_ir(
    analysis: &thevenin_types::Analysis,
    circuit: &Circuit,
) -> Result<cirq_ir::Analysis, AnalysisConversionError> {
    use thevenin_types::Analysis as NA;

    Ok(match analysis {
        NA::Op => cirq_ir::Analysis::Op,

        NA::Dc {
            src,
            start,
            stop,
            step,
            src2,
        } => {
            let mut sweeps = vec![cirq_ir::DcSweep {
                source: lookup_source(circuit, src)?,
                start: expr_to_f64(start, "dc.start")?,
                stop: expr_to_f64(stop, "dc.stop")?,
                step: expr_to_f64(step, "dc.step")?,
            }];
            if let Some(s2) = src2 {
                sweeps.push(cirq_ir::DcSweep {
                    source: lookup_source(circuit, &s2.src)?,
                    start: expr_to_f64(&s2.start, "dc.src2.start")?,
                    stop: expr_to_f64(&s2.stop, "dc.src2.stop")?,
                    step: expr_to_f64(&s2.step, "dc.src2.step")?,
                });
            }
            cirq_ir::Analysis::Dc(cirq_ir::DcAnalysis { sweeps })
        }

        NA::Tran {
            tstep,
            tstop,
            tstart,
            tmax,
            uic,
        } => cirq_ir::Analysis::Tran(cirq_ir::TranAnalysis {
            step: expr_to_f64(tstep, "tran.tstep")?,
            stop: expr_to_f64(tstop, "tran.tstop")?,
            start: match tstart {
                Some(e) => expr_to_f64(e, "tran.tstart")?,
                None => 0.0,
            },
            tmax: match tmax {
                Some(e) => Some(expr_to_f64(e, "tran.tmax")?),
                None => None,
            },
            uic: *uic,
        }),

        NA::Ac {
            variation,
            n,
            fstart,
            fstop,
        } => cirq_ir::Analysis::Ac(cirq_ir::AcAnalysis {
            scale: ac_variation_to_scale(*variation),
            points: *n,
            start: expr_to_f64(fstart, "ac.fstart")?,
            stop: expr_to_f64(fstop, "ac.fstop")?,
        }),

        NA::Noise {
            output,
            ref_node,
            src,
            variation,
            n,
            fstart,
            fstop,
        } => {
            // SPICE permits `v(node)` or `v(node,ref)` as the output spec
            // (the `.control` parser preserves it verbatim). The IR has
            // separate `output_net` and `reference_net` Ids so we unpack
            // the parenthesised form here and override `ref_node` when an
            // inline reference is given.
            let (out_name, inline_ref) = parse_voltage_spec(output);
            let ref_name = inline_ref.or_else(|| ref_node.clone());
            cirq_ir::Analysis::Noise(cirq_ir::NoiseAnalysis {
                output_net: lookup_net(circuit, &out_name)?,
                reference_net: match ref_name.as_deref() {
                    Some(name) => lookup_net(circuit, name)?,
                    None => ground_net_id(circuit),
                },
                source: lookup_source(circuit, src)?,
                scale: ac_variation_to_scale(*variation),
                points: *n,
                start: expr_to_f64(fstart, "noise.fstart")?,
                stop: expr_to_f64(fstop, "noise.fstop")?,
            })
        }

        NA::Tf { output, input } => cirq_ir::Analysis::Tf(cirq_ir::TfAnalysis {
            output: output.clone(),
            source: lookup_source(circuit, input)?,
        }),

        NA::Sens { output } => {
            // `.sens` takes the form `output [AC|DC variation n fstart fstop]`.
            // The Netlist parser preserves it as a token vector; the IR
            // splits the first token (the output expression) from the
            // optional AC sweep tail.
            let first = output
                .first()
                .ok_or(AnalysisConversionError::EmptySensOutputs)?
                .clone();
            let ac = parse_sens_ac_tail(&output[1..])?;
            cirq_ir::Analysis::Sens(cirq_ir::SensAnalysis { output: first, ac })
        }

        NA::Pz {
            node_i,
            node_g,
            node_j,
            node_k,
            input_type,
            analysis_type,
        } => cirq_ir::Analysis::Pz(cirq_ir::PzAnalysis {
            input_pos: lookup_net(circuit, node_i)?,
            input_neg: lookup_net(circuit, node_g)?,
            output_pos: lookup_net(circuit, node_j)?,
            output_neg: lookup_net(circuit, node_k)?,
            transfer: pz_input_to_transfer(*input_type),
            analysis_type: pz_type_to_ir(*analysis_type),
        }),
    })
}

fn ac_variation_to_scale(v: thevenin_types::AcVariation) -> cirq_ir::FrequencyScale {
    match v {
        thevenin_types::AcVariation::Dec => cirq_ir::FrequencyScale::Decade,
        thevenin_types::AcVariation::Oct => cirq_ir::FrequencyScale::Octave,
        thevenin_types::AcVariation::Lin => cirq_ir::FrequencyScale::Linear,
    }
}

fn pz_input_to_transfer(t: thevenin_types::PzInputType) -> cirq_ir::TransferType {
    match t {
        thevenin_types::PzInputType::Vol => cirq_ir::TransferType::Voltage,
        thevenin_types::PzInputType::Cur => cirq_ir::TransferType::Current,
    }
}

fn pz_type_to_ir(t: thevenin_types::PzAnalysisType) -> cirq_ir::PzType {
    match t {
        thevenin_types::PzAnalysisType::Pol => cirq_ir::PzType::Poles,
        thevenin_types::PzAnalysisType::Zer => cirq_ir::PzType::Zeros,
        thevenin_types::PzAnalysisType::Pz => cirq_ir::PzType::Both,
    }
}

fn expr_to_f64(
    expr: &thevenin_types::Expr,
    field: &'static str,
) -> Result<f64, AnalysisConversionError> {
    match expr {
        thevenin_types::Expr::Num(v) => Ok(*v),
        other => Err(AnalysisConversionError::NonNumericExpr {
            field: field.to_string(),
            expr: format!("{other}"),
        }),
    }
}

fn lookup_net(circuit: &Circuit, name: &str) -> Result<Id, AnalysisConversionError> {
    // Net name matching mirrors the SPICE convention: case-insensitive,
    // with `gnd` and `0` both standing for the ground net (Id(0)).
    let lower = name.to_lowercase();
    if lower == "0" || lower == "gnd" {
        return Ok(ground_net_id(circuit));
    }
    for net in &circuit.nets {
        if net.name.eq_ignore_ascii_case(name) {
            return Ok(net.id);
        }
    }
    Err(AnalysisConversionError::UnknownNet(name.to_string()))
}

fn lookup_source(circuit: &Circuit, name: &str) -> Result<Id, AnalysisConversionError> {
    for element in &circuit.elements {
        if element.name.eq_ignore_ascii_case(name) {
            return Ok(element.id);
        }
    }
    Err(AnalysisConversionError::UnknownSource(name.to_string()))
}

/// Canonical ground net Id. The SPICE importer and `ir_lower` both
/// always emit Id(0) as the ground net (see `cirq-frontend/src/ir_lower.rs`
/// and `cirq-spice-import/src/lib.rs`).
fn ground_net_id(_circuit: &Circuit) -> Id {
    Id(0)
}

/// Strip SPICE voltage syntax `v(node[,ref])` from a `.noise` output spec.
///
/// Returns `(bare_node_name, optional_inline_ref)`. A bare token is
/// returned as-is with no inline ref. Mirrors
/// `thevenin::noise::parse_output_spec` so the IR sees the same node
/// resolution the Netlist path applies.
fn parse_voltage_spec(spec: &str) -> (String, Option<String>) {
    let s = spec.trim();
    let stripped = s.strip_prefix("v(").or_else(|| s.strip_prefix("V("));
    let Some(rest) = stripped else {
        return (s.to_string(), None);
    };
    let inner = rest.strip_suffix(')').unwrap_or(rest);
    if let Some((pos, neg)) = inner.split_once(',') {
        (pos.trim().to_string(), Some(neg.trim().to_string()))
    } else {
        (inner.trim().to_string(), None)
    }
}

/// Parse the optional AC tail of `.sens output [AC DEC|OCT|LIN n fstart fstop]`.
///
/// Mirrors `cirq_spice_import::parse_sens_ac_tail` so the `.control`
/// interpreter and the SPICE importer agree on shape. A bare `dc` marker
/// (legacy ngspice tolerance) yields `None`.
fn parse_sens_ac_tail(
    tail: &[String],
) -> Result<Option<cirq_ir::SensAcSpec>, AnalysisConversionError> {
    if tail.is_empty() {
        return Ok(None);
    }
    let first = tail[0].to_ascii_lowercase();
    if first == "dc" {
        return Ok(None);
    }
    if first != "ac" {
        return Err(AnalysisConversionError::SensAcMarker(tail[0].clone()));
    }
    if tail.len() < 5 {
        return Err(AnalysisConversionError::SensAcArity {
            tokens: tail.to_vec(),
        });
    }
    let scale = match tail[1].to_ascii_lowercase().as_str() {
        "dec" | "decade" => cirq_ir::FrequencyScale::Decade,
        "oct" | "octave" => cirq_ir::FrequencyScale::Octave,
        "lin" | "linear" => cirq_ir::FrequencyScale::Linear,
        other => return Err(AnalysisConversionError::SensAcVariation(other.to_string())),
    };
    let parse_num = |token: &str, field: &'static str| -> Result<f64, AnalysisConversionError> {
        thevenin_types::parse::parse_spice_number(token).ok_or_else(|| {
            AnalysisConversionError::SensAcNumber {
                field,
                token: token.to_string(),
            }
        })
    };
    let points = parse_num(&tail[2], "n")? as u32;
    let fstart = parse_num(&tail[3], "fstart")?;
    let fstop = parse_num(&tail[4], "fstop")?;
    Ok(Some(cirq_ir::SensAcSpec {
        scale,
        points,
        fstart,
        fstop,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal Circuit with one voltage source and three nets so
    /// the converter has something to resolve against.
    fn fixture_circuit() -> Circuit {
        Circuit {
            name: "test".into(),
            nets: vec![
                cirq_ir::Net {
                    id: Id(0),
                    name: "0".into(),
                    is_global: true,
                },
                cirq_ir::Net {
                    id: Id(1),
                    name: "in".into(),
                    is_global: false,
                },
                cirq_ir::Net {
                    id: Id(2),
                    name: "out".into(),
                    is_global: false,
                },
            ],
            elements: vec![cirq_ir::Element {
                id: Id(10),
                name: "v1".into(),
                kind: cirq_ir::ElementKind::VoltageSource,
                connections: vec![],
                params: vec![],
                model: None,
                source_spec: Some(cirq_ir::SourceSpec {
                    dc: Some(1.0),
                    ac: None,
                    waveform: None,
                }),
            }],
            models: vec![],
            analyses: vec![],
            params: vec![],
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
    fn op_converts_directly() {
        let result =
            netlist_analysis_to_ir(&thevenin_types::Analysis::Op, &fixture_circuit()).unwrap();
        assert!(matches!(result, cirq_ir::Analysis::Op));
    }

    #[test]
    fn dc_resolves_source_name() {
        let netlist_analysis = thevenin_types::Analysis::Dc {
            src: "V1".into(),
            start: thevenin_types::Expr::Num(0.0),
            stop: thevenin_types::Expr::Num(5.0),
            step: thevenin_types::Expr::Num(0.1),
            src2: None,
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Dc(dc) => {
                assert_eq!(dc.sweeps.len(), 1);
                assert_eq!(dc.sweeps[0].source, Id(10));
                assert_eq!(dc.sweeps[0].start, 0.0);
                assert_eq!(dc.sweeps[0].stop, 5.0);
                assert_eq!(dc.sweeps[0].step, 0.1);
            }
            _ => panic!("expected Dc"),
        }
    }

    #[test]
    fn dc_with_second_sweep() {
        let netlist_analysis = thevenin_types::Analysis::Dc {
            src: "v1".into(),
            start: thevenin_types::Expr::Num(0.0),
            stop: thevenin_types::Expr::Num(5.0),
            step: thevenin_types::Expr::Num(1.0),
            src2: Some(thevenin_types::DcSweep {
                src: "v1".into(),
                start: thevenin_types::Expr::Num(10.0),
                stop: thevenin_types::Expr::Num(20.0),
                step: thevenin_types::Expr::Num(2.0),
            }),
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Dc(dc) => {
                assert_eq!(dc.sweeps.len(), 2);
                assert_eq!(dc.sweeps[1].start, 10.0);
            }
            _ => panic!("expected Dc"),
        }
    }

    #[test]
    fn tran_converts_with_optional_fields() {
        let netlist_analysis = thevenin_types::Analysis::Tran {
            tstep: thevenin_types::Expr::Num(1e-9),
            tstop: thevenin_types::Expr::Num(1e-6),
            tstart: Some(thevenin_types::Expr::Num(0.0)),
            tmax: Some(thevenin_types::Expr::Num(2e-9)),
            uic: true,
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Tran(t) => {
                assert_eq!(t.step, 1e-9);
                assert_eq!(t.stop, 1e-6);
                assert_eq!(t.start, 0.0);
                assert_eq!(t.tmax, Some(2e-9));
                assert!(t.uic);
            }
            _ => panic!("expected Tran"),
        }
    }

    #[test]
    fn ac_variation_maps_to_scale() {
        let netlist_analysis = thevenin_types::Analysis::Ac {
            variation: thevenin_types::AcVariation::Dec,
            n: 10,
            fstart: thevenin_types::Expr::Num(1.0),
            fstop: thevenin_types::Expr::Num(1e6),
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Ac(ac) => {
                assert_eq!(ac.scale, cirq_ir::FrequencyScale::Decade);
                assert_eq!(ac.points, 10);
                assert_eq!(ac.start, 1.0);
                assert_eq!(ac.stop, 1e6);
            }
            _ => panic!("expected Ac"),
        }
    }

    #[test]
    fn noise_resolves_nets_and_source() {
        let netlist_analysis = thevenin_types::Analysis::Noise {
            output: "out".into(),
            ref_node: Some("in".into()),
            src: "v1".into(),
            variation: thevenin_types::AcVariation::Dec,
            n: 10,
            fstart: thevenin_types::Expr::Num(1.0),
            fstop: thevenin_types::Expr::Num(1e6),
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Noise(n) => {
                assert_eq!(n.output_net, Id(2));
                assert_eq!(n.reference_net, Id(1));
                assert_eq!(n.source, Id(10));
                assert_eq!(n.points, 10);
            }
            _ => panic!("expected Noise"),
        }
    }

    #[test]
    fn noise_defaults_ref_node_to_ground() {
        let netlist_analysis = thevenin_types::Analysis::Noise {
            output: "out".into(),
            ref_node: None,
            src: "v1".into(),
            variation: thevenin_types::AcVariation::Lin,
            n: 5,
            fstart: thevenin_types::Expr::Num(1.0),
            fstop: thevenin_types::Expr::Num(2.0),
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Noise(n) => {
                assert_eq!(n.reference_net, Id(0));
            }
            _ => panic!("expected Noise"),
        }
    }

    #[test]
    fn pz_resolves_four_nodes() {
        let netlist_analysis = thevenin_types::Analysis::Pz {
            node_i: "in".into(),
            node_g: "0".into(),
            node_j: "out".into(),
            node_k: "0".into(),
            input_type: thevenin_types::PzInputType::Vol,
            analysis_type: thevenin_types::PzAnalysisType::Pz,
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Pz(pz) => {
                assert_eq!(pz.input_pos, Id(1));
                assert_eq!(pz.input_neg, Id(0));
                assert_eq!(pz.output_pos, Id(2));
                assert_eq!(pz.output_neg, Id(0));
                assert_eq!(pz.transfer, cirq_ir::TransferType::Voltage);
                assert_eq!(pz.analysis_type, cirq_ir::PzType::Both);
            }
            _ => panic!("expected Pz"),
        }
    }

    #[test]
    fn tf_resolves_input_source() {
        let netlist_analysis = thevenin_types::Analysis::Tf {
            output: "v(out)".into(),
            input: "v1".into(),
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Tf(tf) => {
                assert_eq!(tf.output, "v(out)");
                assert_eq!(tf.source, Id(10));
            }
            _ => panic!("expected Tf"),
        }
    }

    #[test]
    fn sens_single_output() {
        let netlist_analysis = thevenin_types::Analysis::Sens {
            output: vec!["v(out)".into()],
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Sens(s) => {
                assert_eq!(s.output, "v(out)");
                assert!(s.ac.is_none());
            }
            _ => panic!("expected Sens"),
        }
    }

    #[test]
    fn sens_with_ac_tail_split() {
        let netlist_analysis = thevenin_types::Analysis::Sens {
            output: vec![
                "v(out)".into(),
                "ac".into(),
                "lin".into(),
                "1".into(),
                "1e6".into(),
                "1.1e6".into(),
            ],
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Sens(s) => {
                assert_eq!(s.output, "v(out)");
                let ac = s.ac.expect("ac spec");
                assert_eq!(ac.scale, cirq_ir::FrequencyScale::Linear);
                assert_eq!(ac.points, 1);
                assert_eq!(ac.fstart, 1e6);
                assert_eq!(ac.fstop, 1.1e6);
            }
            _ => panic!("expected Sens"),
        }
    }

    #[test]
    fn sens_dc_marker_yields_no_ac() {
        let netlist_analysis = thevenin_types::Analysis::Sens {
            output: vec!["v(out)".into(), "dc".into()],
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Sens(s) => {
                assert_eq!(s.output, "v(out)");
                assert!(s.ac.is_none());
            }
            _ => panic!("expected Sens"),
        }
    }

    #[test]
    fn noise_unpacks_v_node_form() {
        let netlist_analysis = thevenin_types::Analysis::Noise {
            output: "v(out)".into(),
            ref_node: None,
            src: "v1".into(),
            variation: thevenin_types::AcVariation::Dec,
            n: 10,
            fstart: thevenin_types::Expr::Num(1.0),
            fstop: thevenin_types::Expr::Num(1e6),
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Noise(n) => {
                assert_eq!(n.output_net, Id(2)); // "out"
                assert_eq!(n.reference_net, Id(0)); // gnd default
            }
            _ => panic!("expected Noise"),
        }
    }

    #[test]
    fn noise_inline_ref_overrides_ref_node() {
        let netlist_analysis = thevenin_types::Analysis::Noise {
            output: "v(out,in)".into(),
            ref_node: None,
            src: "v1".into(),
            variation: thevenin_types::AcVariation::Dec,
            n: 10,
            fstart: thevenin_types::Expr::Num(1.0),
            fstop: thevenin_types::Expr::Num(1e6),
        };
        match netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap() {
            cirq_ir::Analysis::Noise(n) => {
                assert_eq!(n.output_net, Id(2));
                assert_eq!(n.reference_net, Id(1)); // "in"
            }
            _ => panic!("expected Noise"),
        }
    }

    #[test]
    fn sens_empty_outputs_rejected() {
        let netlist_analysis = thevenin_types::Analysis::Sens { output: vec![] };
        let err = netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap_err();
        assert!(matches!(err, AnalysisConversionError::EmptySensOutputs));
    }

    #[test]
    fn unknown_source_errors() {
        let netlist_analysis = thevenin_types::Analysis::Dc {
            src: "v_missing".into(),
            start: thevenin_types::Expr::Num(0.0),
            stop: thevenin_types::Expr::Num(1.0),
            step: thevenin_types::Expr::Num(0.1),
            src2: None,
        };
        let err = netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap_err();
        assert!(matches!(err, AnalysisConversionError::UnknownSource(_)));
    }

    #[test]
    fn unknown_net_errors() {
        let netlist_analysis = thevenin_types::Analysis::Pz {
            node_i: "no_such_net".into(),
            node_g: "0".into(),
            node_j: "out".into(),
            node_k: "0".into(),
            input_type: thevenin_types::PzInputType::Vol,
            analysis_type: thevenin_types::PzAnalysisType::Pz,
        };
        let err = netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap_err();
        assert!(matches!(err, AnalysisConversionError::UnknownNet(_)));
    }

    #[test]
    fn non_numeric_expr_errors() {
        let netlist_analysis = thevenin_types::Analysis::Dc {
            src: "v1".into(),
            start: thevenin_types::Expr::Param("foo".into()),
            stop: thevenin_types::Expr::Num(1.0),
            step: thevenin_types::Expr::Num(0.1),
            src2: None,
        };
        let err = netlist_analysis_to_ir(&netlist_analysis, &fixture_circuit()).unwrap_err();
        assert!(matches!(
            err,
            AnalysisConversionError::NonNumericExpr { .. }
        ));
    }
}
