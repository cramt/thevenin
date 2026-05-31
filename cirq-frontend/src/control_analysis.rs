//! Parse a `.control` analysis command directly into the Cirq IR shape.
//!
//! The `.control` interpreter receives analysis directives as whitespace-split
//! token strings (`dc v1 0 5 0.1`, `ac dec 10 1 1meg`, `pz in 0 out 0 vol pz`,
//! ...). This module parses those tokens straight into [`cirq_ir::Analysis`],
//! resolving source / node names to `Id`s against the surrounding [`Circuit`]
//! and SPICE numbers to `f64`.
//!
//! No SPICE `Netlist` or `thevenin_types::Analysis` intermediate is built: the
//! `.control` run-path is IR-native from the command string onward. This is the
//! converter that the old `parse_analysis_command` + `netlist_analysis_to_ir`
//! two-step collapsed into — see
//! `docs/archive/migration/old-path-retirement-checklist.md` for the broader
//! plan to retire the cached Netlist inside `thevenin-control`.

use cirq_ir::{Circuit, Id};
use thiserror::Error;

/// Failures that can arise when parsing a `.control` analysis command and
/// lifting it to its IR equivalent.
#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("unknown analysis command `{0}`")]
    UnknownCommand(String),
    #[error("dc: need src start stop step, got {0:?}")]
    DcArity(Vec<String>),
    #[error("ac: need variation n fstart fstop")]
    AcArity,
    #[error("ac: unknown variation `{0}`")]
    AcVariation(String),
    #[error("tran: need tstep tstop")]
    TranArity,
    #[error("sens: need output variable")]
    SensArity,
    #[error("noise: need output ref src variation n fstart fstop")]
    NoiseArity,
    #[error("noise: unknown variation `{0}`")]
    NoiseVariation(String),
    #[error("pz: need node_i node_g node_j node_k input_type analysis_type")]
    PzArity,
    #[error("pz: unknown input type `{0}`")]
    PzInputType(String),
    #[error("pz: unknown analysis type `{0}`")]
    PzAnalysisType(String),
    #[error("tf: need output input")]
    TfArity,
    #[error("`{field}`: cannot parse number `{token}`")]
    BadNumber { field: &'static str, token: String },
    #[error("unknown net `{0}` (analysis references a node not in the circuit)")]
    UnknownNet(String),
    #[error("unknown source `{0}` (analysis references an element not in the circuit)")]
    UnknownSource(String),
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

/// Parse a `.control` analysis command (`cmd` + whitespace-split `args`) into
/// its [`cirq_ir::Analysis`] equivalent, resolving source / node names to
/// `Id`s through `circuit`.
///
/// Handles the eight analysis kinds the `.control` interpreter dispatches:
/// `op`, `dc`, `ac`, `tran`, `sens`, `noise`, `pz`, `tf`. (`four` / `fft` are
/// declared as native `analysis` blocks and reach the run-path through
/// `Circuit.analyses`, not as `.control` commands.)
pub fn parse_analysis_to_ir(
    cmd: &str,
    args: &[&str],
    circuit: &Circuit,
) -> Result<cirq_ir::Analysis, AnalysisError> {
    match cmd {
        "op" => Ok(cirq_ir::Analysis::Op),

        "dc" => {
            if args.len() < 4 {
                return Err(AnalysisError::DcArity(
                    args.iter().map(|s| s.to_string()).collect(),
                ));
            }
            let mut sweeps = vec![cirq_ir::DcSweep {
                source: lookup_source(circuit, args[0])?,
                start: parse_num(args[1], "dc.start")?,
                stop: parse_num(args[2], "dc.stop")?,
                step: parse_num(args[3], "dc.step")?,
            }];
            if args.len() >= 8 {
                sweeps.push(cirq_ir::DcSweep {
                    source: lookup_source(circuit, args[4])?,
                    start: parse_num(args[5], "dc.src2.start")?,
                    stop: parse_num(args[6], "dc.src2.stop")?,
                    step: parse_num(args[7], "dc.src2.step")?,
                });
            }
            Ok(cirq_ir::Analysis::Dc(cirq_ir::DcAnalysis { sweeps }))
        }

        "ac" => {
            if args.len() < 4 {
                return Err(AnalysisError::AcArity);
            }
            let scale = match args[0].to_lowercase().as_str() {
                "dec" => cirq_ir::FrequencyScale::Decade,
                "oct" => cirq_ir::FrequencyScale::Octave,
                "lin" => cirq_ir::FrequencyScale::Linear,
                other => return Err(AnalysisError::AcVariation(other.to_string())),
            };
            Ok(cirq_ir::Analysis::Ac(cirq_ir::AcAnalysis {
                scale,
                points: parse_num(args[1], "ac.n")? as u32,
                start: parse_num(args[2], "ac.fstart")?,
                stop: parse_num(args[3], "ac.fstop")?,
            }))
        }

        "tran" => {
            // ngspice grammar: `tran tstep tstop [tstart [tmax]] [uic]`. The
            // trailing `uic` keyword is optional and order-independent relative
            // to the numeric positionals, so strip it first.
            let mut numeric: Vec<&str> = Vec::with_capacity(args.len());
            let mut uic = false;
            for a in args {
                if a.eq_ignore_ascii_case("uic") {
                    uic = true;
                } else {
                    numeric.push(a);
                }
            }
            if numeric.len() < 2 {
                return Err(AnalysisError::TranArity);
            }
            Ok(cirq_ir::Analysis::Tran(cirq_ir::TranAnalysis {
                step: parse_num(numeric[0], "tran.tstep")?,
                stop: parse_num(numeric[1], "tran.tstop")?,
                start: if numeric.len() > 2 {
                    parse_num(numeric[2], "tran.tstart")?
                } else {
                    0.0
                },
                tmax: if numeric.len() > 3 {
                    Some(parse_num(numeric[3], "tran.tmax")?)
                } else {
                    None
                },
                uic,
            }))
        }

        "sens" => {
            // `.sens output [AC|DC variation n fstart fstop]`. The first token
            // is the output expression; the optional tail is an AC sweep.
            if args.is_empty() {
                return Err(AnalysisError::SensArity);
            }
            let output_tokens: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let first = output_tokens
                .first()
                .ok_or(AnalysisError::EmptySensOutputs)?
                .clone();
            let ac = parse_sens_ac_tail(&output_tokens[1..])?;
            Ok(cirq_ir::Analysis::Sens(cirq_ir::SensAnalysis {
                output: first,
                ac,
            }))
        }

        "noise" => {
            if args.len() < 6 {
                return Err(AnalysisError::NoiseArity);
            }
            let scale = match args[2].to_lowercase().as_str() {
                "dec" => cirq_ir::FrequencyScale::Decade,
                "oct" => cirq_ir::FrequencyScale::Octave,
                "lin" => cirq_ir::FrequencyScale::Linear,
                other => return Err(AnalysisError::NoiseVariation(other.to_string())),
            };
            // SPICE permits `v(node)` or `v(node,ref)` as the output spec; the
            // IR carries separate `output_net` / `reference_net` Ids, so unpack
            // the parenthesised form here.
            let (out_name, inline_ref) = parse_voltage_spec(args[0]);
            Ok(cirq_ir::Analysis::Noise(cirq_ir::NoiseAnalysis {
                output_net: lookup_net(circuit, &out_name)?,
                reference_net: match inline_ref.as_deref() {
                    Some(name) => lookup_net(circuit, name)?,
                    None => ground_net_id(circuit),
                },
                source: lookup_source(circuit, args[1])?,
                scale,
                points: parse_num(args[3], "noise.n")? as u32,
                start: parse_num(args[4], "noise.fstart")?,
                stop: parse_num(args[5], "noise.fstop")?,
            }))
        }

        "pz" => {
            if args.len() < 6 {
                return Err(AnalysisError::PzArity);
            }
            let transfer = match args[4].to_lowercase().as_str() {
                "vol" => cirq_ir::TransferType::Voltage,
                "cur" => cirq_ir::TransferType::Current,
                other => return Err(AnalysisError::PzInputType(other.to_string())),
            };
            let analysis_type = match args[5].to_lowercase().as_str() {
                "pol" => cirq_ir::PzType::Poles,
                "zer" => cirq_ir::PzType::Zeros,
                "pz" => cirq_ir::PzType::Both,
                other => return Err(AnalysisError::PzAnalysisType(other.to_string())),
            };
            Ok(cirq_ir::Analysis::Pz(cirq_ir::PzAnalysis {
                input_pos: lookup_net(circuit, args[0])?,
                input_neg: lookup_net(circuit, args[1])?,
                output_pos: lookup_net(circuit, args[2])?,
                output_neg: lookup_net(circuit, args[3])?,
                transfer,
                analysis_type,
            }))
        }

        "tf" => {
            if args.len() < 2 {
                return Err(AnalysisError::TfArity);
            }
            Ok(cirq_ir::Analysis::Tf(cirq_ir::TfAnalysis {
                output: args[0].to_string(),
                source: lookup_source(circuit, args[1])?,
            }))
        }

        other => Err(AnalysisError::UnknownCommand(other.to_string())),
    }
}

/// Parse a SPICE number token, stripping any trailing non-SI unit designator
/// first (so `5V` / `1kHz` resolve like the `.control` parser's `parse_num`).
fn parse_num(token: &str, field: &'static str) -> Result<f64, AnalysisError> {
    let stripped = token
        .trim_end_matches(|c: char| c.is_ascii_alphabetic() && !"tTgGkKmMuUnNpPfFaA".contains(c));
    cirq_ir::control::parse_spice_number(stripped).map_err(|_| AnalysisError::BadNumber {
        field,
        token: token.to_string(),
    })
}

fn lookup_net(circuit: &Circuit, name: &str) -> Result<Id, AnalysisError> {
    // Net name matching mirrors the SPICE convention: case-insensitive, with
    // `gnd` and `0` both standing for the ground net (Id(0)).
    let lower = name.to_lowercase();
    if lower == "0" || lower == "gnd" {
        return Ok(ground_net_id(circuit));
    }
    for net in &circuit.nets {
        if net.name.eq_ignore_ascii_case(name) {
            return Ok(net.id);
        }
    }
    Err(AnalysisError::UnknownNet(name.to_string()))
}

fn lookup_source(circuit: &Circuit, name: &str) -> Result<Id, AnalysisError> {
    for element in &circuit.elements {
        if element.name.eq_ignore_ascii_case(name) {
            return Ok(element.id);
        }
    }
    Err(AnalysisError::UnknownSource(name.to_string()))
}

/// Canonical ground net Id. The SPICE importer and `ir_lower` both always emit
/// Id(0) as the ground net (see `cirq-frontend/src/ir_lower.rs` and
/// `cirq-spice-import/src/lib.rs`).
fn ground_net_id(_circuit: &Circuit) -> Id {
    Id(0)
}

/// Strip SPICE voltage syntax `v(node[,ref])` from a `.noise` output spec.
///
/// Returns `(bare_node_name, optional_inline_ref)`. A bare token is returned
/// as-is with no inline ref. Mirrors `thevenin::noise::parse_output_spec` so
/// the IR sees the same node resolution the Netlist path applied.
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
fn parse_sens_ac_tail(tail: &[String]) -> Result<Option<cirq_ir::SensAcSpec>, AnalysisError> {
    if tail.is_empty() {
        return Ok(None);
    }
    let first = tail[0].to_ascii_lowercase();
    if first == "dc" {
        return Ok(None);
    }
    if first != "ac" {
        return Err(AnalysisError::SensAcMarker(tail[0].clone()));
    }
    if tail.len() < 5 {
        return Err(AnalysisError::SensAcArity {
            tokens: tail.to_vec(),
        });
    }
    let scale = match tail[1].to_ascii_lowercase().as_str() {
        "dec" | "decade" => cirq_ir::FrequencyScale::Decade,
        "oct" | "octave" => cirq_ir::FrequencyScale::Octave,
        "lin" | "linear" => cirq_ir::FrequencyScale::Linear,
        other => return Err(AnalysisError::SensAcVariation(other.to_string())),
    };
    let parse_ac = |token: &str, field: &'static str| -> Result<f64, AnalysisError> {
        cirq_ir::control::parse_spice_number(token).map_err(|_| AnalysisError::SensAcNumber {
            field,
            token: token.to_string(),
        })
    };
    let points = parse_ac(&tail[2], "n")? as u32;
    let fstart = parse_ac(&tail[3], "fstart")?;
    let fstop = parse_ac(&tail[4], "fstop")?;
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

    /// Build a minimal Circuit with one voltage source and three nets so the
    /// parser has something to resolve against.
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
            csparams: vec![],
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

    fn parse(cmd: &str, args: &[&str]) -> Result<cirq_ir::Analysis, AnalysisError> {
        parse_analysis_to_ir(cmd, args, &fixture_circuit())
    }

    #[test]
    fn op_parses_directly() {
        assert!(matches!(parse("op", &[]).unwrap(), cirq_ir::Analysis::Op));
    }

    #[test]
    fn dc_resolves_source_name() {
        match parse("dc", &["V1", "0", "5", "0.1"]).unwrap() {
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
        match parse("dc", &["v1", "0", "5", "1", "v1", "10", "20", "2"]).unwrap() {
            cirq_ir::Analysis::Dc(dc) => {
                assert_eq!(dc.sweeps.len(), 2);
                assert_eq!(dc.sweeps[1].start, 10.0);
            }
            _ => panic!("expected Dc"),
        }
    }

    #[test]
    fn dc_too_few_args_errors() {
        assert!(matches!(
            parse("dc", &["v1", "0"]).unwrap_err(),
            AnalysisError::DcArity(_)
        ));
    }

    #[test]
    fn tran_parses_with_optional_fields_and_uic() {
        match parse("tran", &["1n", "1u", "0", "2n", "uic"]).unwrap() {
            cirq_ir::Analysis::Tran(t) => {
                assert!((t.step - 1e-9).abs() < 1e-18);
                assert!((t.stop - 1e-6).abs() < 1e-15);
                assert_eq!(t.start, 0.0);
                assert!((t.tmax.unwrap() - 2e-9).abs() < 1e-18);
                assert!(t.uic);
            }
            _ => panic!("expected Tran"),
        }
    }

    #[test]
    fn tran_minimal() {
        match parse("tran", &["5u", "5m"]).unwrap() {
            cirq_ir::Analysis::Tran(t) => {
                assert!((t.step - 5e-6).abs() < 1e-15);
                assert!((t.stop - 5e-3).abs() < 1e-12);
                assert_eq!(t.start, 0.0);
                assert_eq!(t.tmax, None);
                assert!(!t.uic);
            }
            _ => panic!("expected Tran"),
        }
    }

    #[test]
    fn ac_variation_maps_to_scale() {
        match parse("ac", &["dec", "10", "1", "1meg"]).unwrap() {
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
    fn ac_unknown_variation_errors() {
        assert!(matches!(
            parse("ac", &["xyz", "10", "1", "1k"]).unwrap_err(),
            AnalysisError::AcVariation(_)
        ));
    }

    #[test]
    fn noise_resolves_nets_and_source() {
        match parse("noise", &["out", "v1", "dec", "10", "1", "1meg"]).unwrap() {
            cirq_ir::Analysis::Noise(n) => {
                assert_eq!(n.output_net, Id(2));
                assert_eq!(n.reference_net, Id(0)); // gnd default
                assert_eq!(n.source, Id(10));
                assert_eq!(n.points, 10);
            }
            _ => panic!("expected Noise"),
        }
    }

    #[test]
    fn noise_unpacks_v_node_form() {
        match parse("noise", &["v(out)", "v1", "dec", "10", "1", "1meg"]).unwrap() {
            cirq_ir::Analysis::Noise(n) => {
                assert_eq!(n.output_net, Id(2));
                assert_eq!(n.reference_net, Id(0));
            }
            _ => panic!("expected Noise"),
        }
    }

    #[test]
    fn noise_inline_ref_overrides_default() {
        match parse("noise", &["v(out,in)", "v1", "dec", "10", "1", "1meg"]).unwrap() {
            cirq_ir::Analysis::Noise(n) => {
                assert_eq!(n.output_net, Id(2));
                assert_eq!(n.reference_net, Id(1)); // "in"
            }
            _ => panic!("expected Noise"),
        }
    }

    #[test]
    fn pz_resolves_four_nodes() {
        match parse("pz", &["in", "0", "out", "0", "vol", "pz"]).unwrap() {
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
        match parse("tf", &["v(out)", "v1"]).unwrap() {
            cirq_ir::Analysis::Tf(tf) => {
                assert_eq!(tf.output, "v(out)");
                assert_eq!(tf.source, Id(10));
            }
            _ => panic!("expected Tf"),
        }
    }

    #[test]
    fn sens_single_output() {
        match parse("sens", &["v(out)"]).unwrap() {
            cirq_ir::Analysis::Sens(s) => {
                assert_eq!(s.output, "v(out)");
                assert!(s.ac.is_none());
            }
            _ => panic!("expected Sens"),
        }
    }

    #[test]
    fn sens_with_ac_tail_split() {
        match parse("sens", &["v(out)", "ac", "lin", "1", "1e6", "1.1e6"]).unwrap() {
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
        match parse("sens", &["v(out)", "dc"]).unwrap() {
            cirq_ir::Analysis::Sens(s) => {
                assert_eq!(s.output, "v(out)");
                assert!(s.ac.is_none());
            }
            _ => panic!("expected Sens"),
        }
    }

    #[test]
    fn sens_empty_outputs_rejected() {
        assert!(matches!(
            parse("sens", &[]).unwrap_err(),
            AnalysisError::SensArity
        ));
    }

    #[test]
    fn unknown_source_errors() {
        assert!(matches!(
            parse("dc", &["v_missing", "0", "1", "0.1"]).unwrap_err(),
            AnalysisError::UnknownSource(_)
        ));
    }

    #[test]
    fn unknown_net_errors() {
        assert!(matches!(
            parse("pz", &["no_such_net", "0", "out", "0", "vol", "pz"]).unwrap_err(),
            AnalysisError::UnknownNet(_)
        ));
    }

    #[test]
    fn bad_number_errors() {
        assert!(matches!(
            parse("dc", &["v1", "not_a_number", "1", "0.1"]).unwrap_err(),
            AnalysisError::BadNumber { .. }
        ));
    }

    #[test]
    fn unknown_command_errors() {
        assert!(matches!(
            parse("bogus", &[]).unwrap_err(),
            AnalysisError::UnknownCommand(_)
        ));
    }
}
