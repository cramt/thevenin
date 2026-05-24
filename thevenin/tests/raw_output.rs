//! Integration tests for the ngspice raw-file writer (`thevenin::raw_output`).
//!
//! These tests run a real simulation through the public Circuit surface, then
//! either parse the emitted raw file back with a small in-test reader or
//! check the exact bytes of the binary payload.

use std::io::Cursor;

use thevenin::raw_output::{write_ascii_raw, write_binary_raw, write_csv};
use thevenin_types::{Complex, Netlist, SimPlot, SimResult, SimVector, VectorData};

mod common;
use common::{simulate_ac, simulate_dc, simulate_op, simulate_tran};

// ---------------------------------------------------------------------------
// Tiny raw-file parser used only by these tests.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ParsedPlot {
    title: String,
    plotname: String,
    flags: String,
    n_variables: usize,
    n_points: usize,
    variable_names: Vec<String>,
    variable_types: Vec<String>,
    values: Vec<Vec<f64>>, // per-point row; complex flattens to (re, im) pairs
}

/// Parse a single plot from an ASCII raw file. Returns the parsed plot and
/// the index of the byte after the plot.
fn parse_ascii_plot(bytes: &[u8], start: usize) -> Option<(ParsedPlot, usize)> {
    let text = std::str::from_utf8(&bytes[start..]).ok()?;
    let mut lines = text.lines();
    let mut title = String::new();
    let mut plotname = String::new();
    let mut flags = String::new();
    let mut n_variables = 0;
    let mut n_points = 0;
    let mut consumed_bytes = 0usize;

    loop {
        let line = lines.next()?;
        consumed_bytes += line.len() + 1;
        if let Some(rest) = line.strip_prefix("Title:") {
            title = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Plotname:") {
            plotname = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Flags:") {
            flags = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("No. Variables:") {
            n_variables = rest.trim().parse().ok()?;
        } else if let Some(rest) = line.strip_prefix("No. Points:") {
            n_points = rest.trim().parse().ok()?;
        } else if line == "Variables:" {
            break;
        }
        // Skip Date:, etc.
    }

    let mut variable_names = Vec::with_capacity(n_variables);
    let mut variable_types = Vec::with_capacity(n_variables);
    for _ in 0..n_variables {
        let line = lines.next()?;
        consumed_bytes += line.len() + 1;
        let parts: Vec<&str> = line.split('\t').filter(|s| !s.is_empty()).collect();
        if parts.len() < 3 {
            return None;
        }
        variable_names.push(parts[1].to_string());
        variable_types.push(parts[2].to_string());
    }

    let values_marker = lines.next()?;
    consumed_bytes += values_marker.len() + 1;
    if values_marker != "Values:" {
        return None;
    }

    let is_complex = flags.contains("complex");
    let mut values: Vec<Vec<f64>> = Vec::with_capacity(n_points);
    for _ in 0..n_points {
        let mut row = Vec::with_capacity(n_variables * if is_complex { 2 } else { 1 });
        for col in 0..n_variables {
            let line = lines.next()?;
            consumed_bytes += line.len() + 1;
            let trimmed = if col == 0 {
                // First column has the leading point index.
                line.trim_start()
                    .split('\t')
                    .nth(1)
                    .unwrap_or("")
                    .to_string()
            } else {
                line.trim().to_string()
            };
            if is_complex {
                let (re, im) = trimmed.split_once(',')?;
                row.push(re.trim().parse().ok()?);
                row.push(im.trim().parse().ok()?);
            } else {
                row.push(trimmed.trim().parse().ok()?);
            }
        }
        values.push(row);
    }

    Some((
        ParsedPlot {
            title,
            plotname,
            flags,
            n_variables,
            n_points,
            variable_names,
            variable_types,
            values,
        },
        start + consumed_bytes,
    ))
}

/// Parse all plots from an ASCII raw file.
fn parse_ascii_all(bytes: &[u8]) -> Vec<ParsedPlot> {
    let mut plots = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        // Skip leading whitespace between plots.
        while cursor < bytes.len() && (bytes[cursor] == b'\n' || bytes[cursor] == b'\r') {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }
        let (plot, next) = match parse_ascii_plot(bytes, cursor) {
            Some(p) => p,
            None => break,
        };
        plots.push(plot);
        cursor = next;
    }
    plots
}

/// Parse a binary raw file: text header up to `Binary:\n`, then little-endian
/// `f64` values, row-major (point-by-point, each point has N values; complex
/// is two f64 per variable).
fn parse_binary_plot(bytes: &[u8], start: usize) -> Option<(ParsedPlot, usize)> {
    // Find the "Binary:\n" marker scanning forward from `start`.
    let marker = b"Binary:\n";
    let mut bin_pos = None;
    for i in start..=(bytes.len().saturating_sub(marker.len())) {
        if &bytes[i..i + marker.len()] == marker {
            bin_pos = Some(i);
            break;
        }
    }
    let bin_pos = bin_pos?;
    let header = std::str::from_utf8(&bytes[start..bin_pos]).ok()?;
    let mut lines = header.lines();
    let mut title = String::new();
    let mut plotname = String::new();
    let mut flags = String::new();
    let mut n_variables = 0;
    let mut n_points = 0;
    let mut variable_names = Vec::new();
    let mut variable_types = Vec::new();
    let mut saw_variables = false;
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("Title:") {
            title = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Plotname:") {
            plotname = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("Flags:") {
            flags = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("No. Variables:") {
            n_variables = rest.trim().parse().ok()?;
        } else if let Some(rest) = line.strip_prefix("No. Points:") {
            n_points = rest.trim().parse().ok()?;
        } else if line == "Variables:" {
            saw_variables = true;
            for _ in 0..n_variables {
                let vline = lines.next()?;
                let parts: Vec<&str> = vline.split('\t').filter(|s| !s.is_empty()).collect();
                if parts.len() < 3 {
                    return None;
                }
                variable_names.push(parts[1].to_string());
                variable_types.push(parts[2].to_string());
            }
        }
    }
    if !saw_variables {
        return None;
    }

    let is_complex = flags.contains("complex");
    let stride = if is_complex { 2 } else { 1 };
    let mut cursor = bin_pos + marker.len();
    let mut values = Vec::with_capacity(n_points);
    for _ in 0..n_points {
        let mut row = Vec::with_capacity(n_variables * stride);
        for _ in 0..n_variables {
            for _ in 0..stride {
                if cursor + 8 > bytes.len() {
                    return None;
                }
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&bytes[cursor..cursor + 8]);
                row.push(f64::from_le_bytes(buf));
                cursor += 8;
            }
        }
        values.push(row);
    }

    Some((
        ParsedPlot {
            title,
            plotname,
            flags,
            n_variables,
            n_points,
            variable_names,
            variable_types,
            values,
        },
        cursor,
    ))
}

fn parse_binary_all(bytes: &[u8]) -> Vec<ParsedPlot> {
    let mut plots = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        while cursor < bytes.len() && (bytes[cursor] == b'\n' || bytes[cursor] == b'\r') {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }
        let (plot, next) = match parse_binary_plot(bytes, cursor) {
            Some(p) => p,
            None => break,
        };
        plots.push(plot);
        cursor = next;
    }
    plots
}

// ---------------------------------------------------------------------------
// Synthetic helper: build a SimResult without running a simulation. Useful
// when we only want to test the writer + reader.
// ---------------------------------------------------------------------------

fn synthetic_tran_plot() -> SimPlot {
    let times = vec![0.0, 1e-6, 2e-6, 3e-6];
    let vout = vec![0.0, 0.5, 1.0, 0.5];
    SimPlot {
        name: "tran1".to_string(),
        vecs: vec![
            SimVector::real("time", times),
            SimVector::real("v(out)", vout),
        ],
    }
}

fn synthetic_ac_plot() -> SimPlot {
    let freqs = vec![1.0, 10.0, 100.0];
    let vout = vec![
        Complex::new(1.0, 0.0),
        Complex::new(0.5, -0.5),
        Complex::new(0.1, -0.05),
    ];
    SimPlot {
        name: "ac1".to_string(),
        vecs: vec![
            SimVector::real("frequency", freqs),
            SimVector::complex("v(out)", vout),
        ],
    }
}

// ---------------------------------------------------------------------------
// ASCII round-trip
// ---------------------------------------------------------------------------

#[test]
fn ascii_roundtrip_real_plot() {
    let plot = synthetic_tran_plot();
    let result = SimResult {
        plots: vec![plot.clone()],
    };
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_ascii_raw(&mut buf, &result, "rt-real").expect("write");
    let parsed = parse_ascii_all(buf.get_ref());
    assert_eq!(parsed.len(), 1);
    let p = &parsed[0];
    assert_eq!(p.title, "rt-real");
    assert_eq!(p.plotname, "Transient Analysis");
    assert_eq!(p.flags, "real");
    assert_eq!(p.n_variables, 2);
    assert_eq!(p.n_points, 4);
    assert_eq!(p.variable_names, vec!["time", "v(out)"]);
    assert_eq!(p.variable_types, vec!["time", "voltage"]);

    let time_data = plot.vecs[0].data.as_real();
    let vout_data = plot.vecs[1].data.as_real();
    for (i, row) in p.values.iter().enumerate() {
        assert_eq!(row.len(), 2);
        assert!((row[0] - time_data[i]).abs() < 1e-12);
        assert!((row[1] - vout_data[i]).abs() < 1e-12);
    }
}

#[test]
fn ascii_roundtrip_complex_plot() {
    let plot = synthetic_ac_plot();
    let result = SimResult {
        plots: vec![plot.clone()],
    };
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_ascii_raw(&mut buf, &result, "rt-ac").expect("write");
    let parsed = parse_ascii_all(buf.get_ref());
    assert_eq!(parsed.len(), 1);
    let p = &parsed[0];
    assert_eq!(p.plotname, "AC Analysis");
    assert_eq!(p.flags, "complex");
    assert_eq!(p.n_points, 3);
    // Each row has 2 vars × 2 (re,im) = 4 values, even though frequency is real.
    for (i, row) in p.values.iter().enumerate() {
        assert_eq!(row.len(), 4);
        // Frequency padded with 0 imaginary.
        let freq = plot.vecs[0].data.as_real()[i];
        assert!((row[0] - freq).abs() < 1e-12);
        assert!(row[1].abs() < 1e-12, "real-var imag padding is 0");
        let c = plot.vecs[1].data.as_complex()[i];
        assert!((row[2] - c.re).abs() < 1e-12);
        assert!((row[3] - c.im).abs() < 1e-12);
    }
}

// ---------------------------------------------------------------------------
// Binary round-trip
// ---------------------------------------------------------------------------

#[test]
fn binary_roundtrip_real_plot() {
    let plot = synthetic_tran_plot();
    let result = SimResult {
        plots: vec![plot.clone()],
    };
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_binary_raw(&mut buf, &result, "rt-bin").expect("write");
    let parsed = parse_binary_all(buf.get_ref());
    assert_eq!(parsed.len(), 1);
    let p = &parsed[0];
    assert_eq!(p.flags, "real");
    assert_eq!(p.n_points, 4);
    let time_data = plot.vecs[0].data.as_real();
    let vout_data = plot.vecs[1].data.as_real();
    for (i, row) in p.values.iter().enumerate() {
        assert_eq!(row.len(), 2);
        assert_eq!(row[0], time_data[i]);
        assert_eq!(row[1], vout_data[i]);
    }
}

#[test]
fn binary_roundtrip_complex_plot() {
    let plot = synthetic_ac_plot();
    let result = SimResult {
        plots: vec![plot.clone()],
    };
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_binary_raw(&mut buf, &result, "rt-bin-ac").expect("write");
    let parsed = parse_binary_all(buf.get_ref());
    assert_eq!(parsed.len(), 1);
    let p = &parsed[0];
    assert_eq!(p.flags, "complex");
    for (i, row) in p.values.iter().enumerate() {
        assert_eq!(row.len(), 4);
        let freq = plot.vecs[0].data.as_real()[i];
        assert_eq!(row[0], freq);
        assert_eq!(row[1], 0.0);
        let c = plot.vecs[1].data.as_complex()[i];
        assert_eq!(row[2], c.re);
        assert_eq!(row[3], c.im);
    }
}

// ---------------------------------------------------------------------------
// Binary endianness — explicit bytes match f64::to_le_bytes
// ---------------------------------------------------------------------------

#[test]
fn binary_is_little_endian_f64() {
    // Use a value whose IEEE 754 bytes are asymmetric so endianness
    // confusion would change them.
    let val = std::f64::consts::PI;
    let plot = SimPlot {
        name: "op1".to_string(),
        vecs: vec![SimVector::real("v(x)", vec![val])],
    };
    let result = SimResult { plots: vec![plot] };
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_binary_raw(&mut buf, &result, "endian").expect("write");
    let bytes = buf.into_inner();
    let marker = b"Binary:\n";
    let pos = bytes
        .windows(marker.len())
        .position(|w| w == marker)
        .unwrap();
    let payload = &bytes[pos + marker.len()..];
    assert_eq!(payload.len(), 8);
    assert_eq!(payload, &val.to_le_bytes());
}

// ---------------------------------------------------------------------------
// Multi-plot file: two analyses back-to-back
// ---------------------------------------------------------------------------

#[test]
fn multi_plot_ascii_round_trip() {
    let result = SimResult {
        plots: vec![synthetic_tran_plot(), synthetic_ac_plot()],
    };
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_ascii_raw(&mut buf, &result, "multi").expect("write");
    let parsed = parse_ascii_all(buf.get_ref());
    assert_eq!(parsed.len(), 2, "two plots round-trip");
    assert_eq!(parsed[0].plotname, "Transient Analysis");
    assert_eq!(parsed[0].flags, "real");
    assert_eq!(parsed[1].plotname, "AC Analysis");
    assert_eq!(parsed[1].flags, "complex");
}

#[test]
fn multi_plot_binary_round_trip() {
    let result = SimResult {
        plots: vec![synthetic_tran_plot(), synthetic_ac_plot()],
    };
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_binary_raw(&mut buf, &result, "multi-bin").expect("write");
    let parsed = parse_binary_all(buf.get_ref());
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].plotname, "Transient Analysis");
    assert_eq!(parsed[1].plotname, "AC Analysis");
    assert_eq!(parsed[1].flags, "complex");
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

#[test]
fn csv_real_plot_has_header_and_rows() {
    let result = SimResult {
        plots: vec![synthetic_tran_plot()],
    };
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_csv(&mut buf, &result).expect("write");
    let text = String::from_utf8(buf.into_inner()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5, "header + 4 rows");
    assert_eq!(lines[0], "time,v(out)");
    let parts: Vec<&str> = lines[1].split(',').collect();
    assert_eq!(parts.len(), 2);
    assert!(parts[0].parse::<f64>().is_ok());
    assert!(parts[1].parse::<f64>().is_ok());
}

#[test]
fn csv_complex_plot_splits_into_real_imag_columns() {
    let result = SimResult {
        plots: vec![synthetic_ac_plot()],
    };
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_csv(&mut buf, &result).expect("write");
    let text = String::from_utf8(buf.into_inner()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "frequency,v(out)_real,v(out)_imag");
}

// ---------------------------------------------------------------------------
// End-to-end: real simulator output
// ---------------------------------------------------------------------------

#[test]
fn op_simulation_round_trips_through_ascii() {
    // Voltage divider, 6 V in, V(mid) should be 4 V.
    let spice = "Divider\n\
        V1 in 0 6\n\
        R1 in mid 1k\n\
        R2 mid 0 2k\n\
        .op\n\
        .end\n";
    let netlist = Netlist::parse_single(spice).expect("parse netlist");
    let result = simulate_op(&netlist);
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_ascii_raw(&mut buf, &result, "divider").expect("write");
    let parsed = parse_ascii_all(buf.get_ref());
    assert_eq!(parsed.len(), 1);
    let p = &parsed[0];
    assert_eq!(p.title, "divider");
    assert_eq!(p.plotname, "Operating Point");
    // V(mid) ~= 4 V — find it by name.
    let mid_idx = p
        .variable_names
        .iter()
        .position(|n| n.eq_ignore_ascii_case("v(mid)"))
        .expect("v(mid) variable");
    assert!((p.values[0][mid_idx] - 4.0).abs() < 1e-6);
}

#[test]
fn tran_simulation_round_trips_through_binary() {
    let spice = "RC step\n\
        V1 in 0 PULSE(0 1 0 1n 1n 1m 2m)\n\
        R1 in out 1k\n\
        C1 out 0 1u\n\
        .tran 100u 1m\n\
        .end\n";
    let netlist = Netlist::parse_single(spice).expect("parse");
    let netlist = if matches!(netlist.analysis, thevenin_types::Analysis::Tran { .. }) {
        netlist
    } else {
        panic!("expected .tran analysis on netlist");
    };
    let result = simulate_tran(&netlist);
    assert!(!result.plots.is_empty());
    let mut buf = Cursor::new(Vec::<u8>::new());
    write_binary_raw(&mut buf, &result, "rc-tran").expect("write");
    let parsed = parse_binary_all(buf.get_ref());
    assert!(!parsed.is_empty());
    let p = &parsed[0];
    assert_eq!(p.title, "rc-tran");
    assert_eq!(p.plotname, "Transient Analysis");
    // Time column matches the simulator's time vector.
    let sim_time = match &result.plots[0]
        .vecs
        .iter()
        .find(|v| v.name == "time")
        .expect("time vec")
        .data
    {
        VectorData::Real(d) => d.clone(),
        _ => panic!("time vector should be real"),
    };
    assert_eq!(p.n_points, sim_time.len());
    let time_col_idx = p
        .variable_names
        .iter()
        .position(|n| n == "time")
        .expect("time variable column");
    for (i, row) in p.values.iter().enumerate() {
        assert_eq!(row[time_col_idx], sim_time[i]);
    }
}

#[test]
fn dc_and_ac_concatenated_into_one_file() {
    // Two separate simulations into the same SimResult.plots vec.
    let dc_spice = "Sweep\n\
        V1 in 0 1\n\
        R1 in out 1k\n\
        R2 out 0 1k\n\
        .dc V1 0 5 1\n\
        .end\n";
    let ac_spice = "Filter\n\
        V1 in 0 AC 1\n\
        R1 in out 1k\n\
        C1 out 0 1n\n\
        .ac DEC 10 1k 1Meg\n\
        .end\n";
    let dc_net = Netlist::parse_single(dc_spice).expect("parse dc");
    let ac_net = Netlist::parse_single(ac_spice).expect("parse ac");

    let dc_result = simulate_dc(&dc_net);
    let ac_result = simulate_ac(&ac_net);
    let combined = SimResult {
        plots: dc_result.plots.into_iter().chain(ac_result.plots).collect(),
    };

    let mut buf = Cursor::new(Vec::<u8>::new());
    write_ascii_raw(&mut buf, &combined, "dc+ac").expect("write");
    let parsed = parse_ascii_all(buf.get_ref());
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].plotname, "DC transfer characteristic");
    assert_eq!(parsed[1].plotname, "AC Analysis");
    assert_eq!(parsed[1].flags, "complex");
}
