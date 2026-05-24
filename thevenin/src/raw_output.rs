//! ngspice "raw file" output: the canonical interchange format for SPICE
//! simulation results.
//!
//! Supports three formats:
//!
//! - **ASCII raw** ([`write_ascii_raw`]) — human-readable text body after
//!   the header, one row per data point.
//! - **Binary raw** ([`write_binary_raw`]) — IEEE 754 little-endian `f64`
//!   values after the header. ngspice itself writes in native byte order;
//!   we standardise on little-endian for portability.
//! - **CSV** ([`write_csv`]) — convenience format for plotting tools that
//!   speak CSV but not raw.
//!
//! The raw file format is documented in the ngspice manual, chapter 13.7.
//! See [`docs/architecture/raw-file-format.md`] for the thevenin-specific
//! choices (little-endian binary, plotname mapping, type inference).
//!
//! [`docs/architecture/raw-file-format.md`]: ../../../docs/architecture/raw-file-format.md

use std::io::{self, Write};

use thevenin_types::{SimPlot, SimResult, SimVector, VectorData};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Write a [`SimResult`] in ASCII raw file format. Each plot in
/// `result.plots` becomes one header+values block, concatenated in order —
/// matching ngspice's multi-plot raw file behaviour.
pub fn write_ascii_raw<W: Write>(
    writer: &mut W,
    result: &SimResult,
    title: &str,
) -> io::Result<()> {
    for plot in &result.plots {
        write_plot_ascii(writer, plot, title)?;
    }
    Ok(())
}

/// Write a [`SimResult`] in binary raw file format. The header is identical
/// ASCII text; the data section is IEEE 754 little-endian `f64` values,
/// row-major (point-by-point). Complex values are pairs of `f64`
/// `(real, imag)`.
pub fn write_binary_raw<W: Write>(
    writer: &mut W,
    result: &SimResult,
    title: &str,
) -> io::Result<()> {
    for plot in &result.plots {
        write_plot_binary(writer, plot, title)?;
    }
    Ok(())
}

/// Write the first plot of a [`SimResult`] as CSV. Header row is the
/// variable names; one row per data point. Complex values become two
/// columns `name_real`, `name_imag`.
///
/// Only the first plot is written — CSV has no concept of concatenated
/// plots, so a multi-plot `SimResult` collapses to its first entry.
pub fn write_csv<W: Write>(writer: &mut W, result: &SimResult) -> io::Result<()> {
    let Some(plot) = result.plots.first() else {
        return Ok(());
    };
    write_plot_csv(writer, plot)
}

// ---------------------------------------------------------------------------
// Per-plot writers
// ---------------------------------------------------------------------------

fn write_plot_ascii<W: Write>(writer: &mut W, plot: &SimPlot, title: &str) -> io::Result<()> {
    let is_complex = plot.vecs.iter().any(|v| v.data.is_complex());
    let n_points = plot.vecs.iter().map(|v| v.len()).max().unwrap_or(0);
    write_header(writer, plot, title, is_complex, n_points)?;
    writeln!(writer, "Values:")?;
    for i in 0..n_points {
        write!(writer, " {i}")?;
        for vec in &plot.vecs {
            // ngspice writes a tab before every value; the point index sits
            // alone on the first column.
            write!(writer, "\t")?;
            write_value_ascii(writer, vec, i, is_complex)?;
            writeln!(writer)?;
        }
    }
    Ok(())
}

fn write_plot_binary<W: Write>(writer: &mut W, plot: &SimPlot, title: &str) -> io::Result<()> {
    let is_complex = plot.vecs.iter().any(|v| v.data.is_complex());
    let n_points = plot.vecs.iter().map(|v| v.len()).max().unwrap_or(0);
    write_header(writer, plot, title, is_complex, n_points)?;
    writeln!(writer, "Binary:")?;
    for i in 0..n_points {
        for vec in &plot.vecs {
            write_value_binary(writer, vec, i, is_complex)?;
        }
    }
    Ok(())
}

fn write_plot_csv<W: Write>(writer: &mut W, plot: &SimPlot) -> io::Result<()> {
    // Header row.
    let mut first = true;
    for vec in &plot.vecs {
        if !first {
            write!(writer, ",")?;
        }
        first = false;
        if vec.data.is_complex() {
            write!(writer, "{}_real,{}_imag", vec.name, vec.name)?;
        } else {
            write!(writer, "{}", vec.name)?;
        }
    }
    writeln!(writer)?;

    let n_points = plot.vecs.iter().map(|v| v.len()).max().unwrap_or(0);
    for i in 0..n_points {
        let mut first = true;
        for vec in &plot.vecs {
            if !first {
                write!(writer, ",")?;
            }
            first = false;
            match &vec.data {
                VectorData::Real(d) => {
                    let v = d.get(i).copied().unwrap_or(0.0);
                    write!(writer, "{v:.15e}")?;
                }
                VectorData::Complex(d) => {
                    if let Some(c) = d.get(i) {
                        write!(writer, "{:.15e},{:.15e}", c.re, c.im)?;
                    } else {
                        write!(writer, "0.000000000000000e0,0.000000000000000e0")?;
                    }
                }
            }
        }
        writeln!(writer)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Header (shared between ASCII and binary)
// ---------------------------------------------------------------------------

fn write_header<W: Write>(
    writer: &mut W,
    plot: &SimPlot,
    title: &str,
    is_complex: bool,
    n_points: usize,
) -> io::Result<()> {
    writeln!(writer, "Title: {title}")?;
    writeln!(writer, "Date: {}", date_now())?;
    writeln!(writer, "Plotname: {}", plotname_for(&plot.name))?;
    writeln!(
        writer,
        "Flags: {}",
        if is_complex { "complex" } else { "real" }
    )?;
    writeln!(writer, "No. Variables: {}", plot.vecs.len())?;
    writeln!(writer, "No. Points: {n_points}")?;
    writeln!(writer, "Variables:")?;
    for (i, vec) in plot.vecs.iter().enumerate() {
        writeln!(
            writer,
            "\t{i}\t{}\t{}",
            display_name(&vec.name),
            type_for(&vec.name)
        )?;
    }
    Ok(())
}

/// Map a thevenin plot name like `"op1"`, `"tran2"` to the ngspice
/// `Plotname:` header value. Plot names in [`SimPlot::name`] are
/// `<analysis_tag><counter>`; we strip the trailing digits and map the
/// analysis tag to the human-readable label ngspice writes.
fn plotname_for(plot_name: &str) -> &'static str {
    let lower = plot_name.to_lowercase();
    let tag: String = lower
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .to_string();
    match tag.as_str() {
        "op" => "Operating Point",
        "dc" => "DC transfer characteristic",
        "tran" => "Transient Analysis",
        "ac" => "AC Analysis",
        "noise" => "Noise Spectral Density",
        "pz" => "Pole-Zero Analysis",
        "tf" => "Transfer Function",
        "sens" => "Sensitivity Analysis",
        _ => "Operating Point",
    }
}

/// Map a vector name to the ngspice `Variables:` type column.
///
/// `time` → time, `frequency` → frequency, `v(...)` → voltage,
/// `i(...)` or `*#branch` → current, else → notype.
fn type_for(vec_name: &str) -> &'static str {
    let lower = vec_name.to_lowercase();
    if lower == "time" {
        "time"
    } else if lower == "frequency" {
        "frequency"
    } else if lower.starts_with("v(") {
        "voltage"
    } else if lower.starts_with("i(") || lower.contains("#branch") {
        "current"
    } else {
        "notype"
    }
}

/// Render a vector name for the `Variables:` block. ngspice writes
/// `i(<name>)` rather than `<name>#branch`; we mirror that.
fn display_name(vec_name: &str) -> String {
    if let Some(stripped) = vec_name.strip_suffix("#branch") {
        format!("i({stripped})")
    } else {
        vec_name.to_string()
    }
}

// ---------------------------------------------------------------------------
// Per-point writers
// ---------------------------------------------------------------------------

fn write_value_ascii<W: Write>(
    writer: &mut W,
    vec: &SimVector,
    i: usize,
    file_is_complex: bool,
) -> io::Result<()> {
    // ngspice uses ~16 significant digits (DOUBLE_PRECISION = 16) for raw
    // file ASCII values. `{:.15e}` matches that — 15 digits after the
    // decimal in scientific form.
    match &vec.data {
        VectorData::Real(d) => {
            let v = d.get(i).copied().unwrap_or(0.0);
            if file_is_complex {
                // Pad a real vector inside a complex plot with a zero
                // imaginary part, matching ngspice's `%.*e,0.0` layout.
                write!(writer, "{v:.15e},0.0")
            } else {
                write!(writer, "{v:.15e}")
            }
        }
        VectorData::Complex(d) => {
            let c = d
                .get(i)
                .copied()
                .unwrap_or(thevenin_types::Complex { re: 0.0, im: 0.0 });
            write!(writer, "{:.15e},{:.15e}", c.re, c.im)
        }
    }
}

fn write_value_binary<W: Write>(
    writer: &mut W,
    vec: &SimVector,
    i: usize,
    file_is_complex: bool,
) -> io::Result<()> {
    match &vec.data {
        VectorData::Real(d) => {
            let v = d.get(i).copied().unwrap_or(0.0);
            writer.write_all(&v.to_le_bytes())?;
            if file_is_complex {
                // Real vector inside a complex plot: pad the imaginary
                // part with zero.
                writer.write_all(&0.0f64.to_le_bytes())?;
            }
        }
        VectorData::Complex(d) => {
            let c = d
                .get(i)
                .copied()
                .unwrap_or(thevenin_types::Complex { re: 0.0, im: 0.0 });
            writer.write_all(&c.re.to_le_bytes())?;
            writer.write_all(&c.im.to_le_bytes())?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Date string
// ---------------------------------------------------------------------------

/// Render the current date for the `Date:` header.
///
/// ngspice writes the output of `asctime(localtime(now))`, but the field is
/// metadata only — no consumer of the raw file parses it. We emit a
/// deterministic fixed string when system time is unavailable and a UTC
/// ISO-8601-ish string otherwise.
fn date_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc_seconds(secs)
}

/// Format `secs` since the UNIX epoch as a `YYYY-MM-DD HH:MM:SS UTC`
/// string. Computes the calendar date with the proleptic Gregorian
/// algorithm so we don't pull in a date crate just for this metadata.
fn format_utc_seconds(secs: u64) -> String {
    let day = secs / 86_400;
    let tod = secs % 86_400;
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;
    let (year, month, day_of_month) = days_to_ymd(day as i64 + 719_468);
    format!("{year:04}-{month:02}-{day_of_month:02} {hour:02}:{minute:02}:{second:02} UTC")
}

/// Howard Hinnant's `days_from_civil` inverse — converts a "days from
/// 0000-03-01" count into `(year, month, day)`.
fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use thevenin_types::{Complex, SimPlot, SimVector};

    fn op_plot() -> SimPlot {
        SimPlot {
            name: "op1".to_string(),
            vecs: vec![SimVector::real("v(out)", vec![1.25])],
        }
    }

    #[test]
    fn type_for_known_vectors() {
        assert_eq!(type_for("time"), "time");
        assert_eq!(type_for("frequency"), "frequency");
        assert_eq!(type_for("v(out)"), "voltage");
        assert_eq!(type_for("i(vsrc)"), "current");
        assert_eq!(type_for("vsrc#branch"), "current");
        assert_eq!(type_for("temp-sweep"), "notype");
    }

    #[test]
    fn type_for_is_case_insensitive() {
        assert_eq!(type_for("V(Out)"), "voltage");
        assert_eq!(type_for("TIME"), "time");
    }

    #[test]
    fn plotname_for_known_analyses() {
        assert_eq!(plotname_for("op1"), "Operating Point");
        assert_eq!(plotname_for("dc1"), "DC transfer characteristic");
        assert_eq!(plotname_for("tran1"), "Transient Analysis");
        assert_eq!(plotname_for("ac1"), "AC Analysis");
        assert_eq!(plotname_for("noise2"), "Noise Spectral Density");
        assert_eq!(plotname_for("pz1"), "Pole-Zero Analysis");
        assert_eq!(plotname_for("tf1"), "Transfer Function");
        assert_eq!(plotname_for("sens1"), "Sensitivity Analysis");
    }

    #[test]
    fn display_name_branch_to_i() {
        assert_eq!(display_name("v1#branch"), "i(v1)");
        assert_eq!(display_name("v(out)"), "v(out)");
        assert_eq!(display_name("time"), "time");
    }

    #[test]
    fn ascii_header_has_required_fields() {
        let result = SimResult {
            plots: vec![op_plot()],
        };
        let mut buf = Vec::new();
        write_ascii_raw(&mut buf, &result, "my-circuit").unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.starts_with("Title: my-circuit\n"));
        assert!(text.contains("Plotname: Operating Point\n"));
        assert!(text.contains("Flags: real\n"));
        assert!(text.contains("No. Variables: 1\n"));
        assert!(text.contains("No. Points: 1\n"));
        assert!(text.contains("Variables:\n"));
        assert!(text.contains("\t0\tv(out)\tvoltage\n"));
        assert!(text.contains("Values:\n"));
    }

    #[test]
    fn binary_emits_little_endian_f64() {
        let result = SimResult {
            plots: vec![op_plot()],
        };
        let mut buf = Vec::new();
        write_binary_raw(&mut buf, &result, "lit").unwrap();
        // Find "Binary:\n" and check the trailing 8 bytes match
        // f64::to_le_bytes(1.25).
        let marker = b"Binary:\n";
        let pos = buf
            .windows(marker.len())
            .position(|w| w == marker)
            .expect("Binary: marker present");
        let data = &buf[pos + marker.len()..];
        assert_eq!(data.len(), 8, "one f64 point");
        assert_eq!(data, &1.25f64.to_le_bytes());
    }

    #[test]
    fn ac_plot_marks_flags_complex() {
        let plot = SimPlot {
            name: "ac1".to_string(),
            vecs: vec![
                SimVector::real("frequency", vec![1.0, 10.0]),
                SimVector::complex(
                    "v(out)",
                    vec![Complex::new(0.5, 0.25), Complex::new(0.4, 0.1)],
                ),
            ],
        };
        let result = SimResult { plots: vec![plot] };
        let mut buf = Vec::new();
        write_ascii_raw(&mut buf, &result, "ac").unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("Flags: complex\n"));
        // Real-valued frequency column should still emit `re,0.0` inside a
        // complex plot.
        assert!(
            text.contains("1.000000000000000e0,0.0\n"),
            "real vector padded with 0.0 imaginary: {text}"
        );
    }

    #[test]
    fn csv_header_and_rows() {
        let plot = SimPlot {
            name: "tran1".to_string(),
            vecs: vec![
                SimVector::real("time", vec![0.0, 1e-3]),
                SimVector::real("v(out)", vec![1.0, 0.5]),
            ],
        };
        let result = SimResult { plots: vec![plot] };
        let mut buf = Vec::new();
        write_csv(&mut buf, &result).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "time,v(out)");
        assert!(lines[1].starts_with("0.000000000000000e0"));
    }

    #[test]
    fn multi_plot_concatenates_headers() {
        let result = SimResult {
            plots: vec![
                op_plot(),
                SimPlot {
                    name: "ac1".to_string(),
                    vecs: vec![
                        SimVector::real("frequency", vec![1.0]),
                        SimVector::complex("v(out)", vec![Complex::new(1.0, 0.0)]),
                    ],
                },
            ],
        };
        let mut buf = Vec::new();
        write_ascii_raw(&mut buf, &result, "multi").unwrap();
        let text = String::from_utf8(buf).unwrap();
        let title_count = text.matches("Title: multi\n").count();
        assert_eq!(title_count, 2, "one Title per plot");
        assert!(text.contains("Plotname: Operating Point\n"));
        assert!(text.contains("Plotname: AC Analysis\n"));
    }

    #[test]
    fn date_string_is_well_formed() {
        // Spot-check the calendar algorithm against a known value.
        // 2021-01-01 00:00:00 UTC = 1609459200 seconds since the epoch.
        let s = format_utc_seconds(1_609_459_200);
        assert_eq!(s, "2021-01-01 00:00:00 UTC");
        // 2024-02-29 — leap-year boundary.
        let s = format_utc_seconds(1_709_164_800);
        assert_eq!(s, "2024-02-29 00:00:00 UTC");
    }
}
