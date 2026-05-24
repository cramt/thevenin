//! Fourier post-processing for `.tran` simulation output.
//!
//! Implements the two SPICE Fourier directives:
//!
//! - **`.four`** — classic DFT at a specified fundamental frequency.
//!   Reports the DC component plus magnitude / phase for the first
//!   `num_harmonics` harmonics, the normalised magnitudes, and total
//!   harmonic distortion (THD).
//! - **`.fft`** — windowed radix-2 FFT over a user-selected interval of
//!   the transient. Supports rectangular, Hann, Hamming, Blackman, and
//!   Bartlett windows; `npoints` is rounded up to the next power of two.
//!
//! Both directives are *post-processing*: they consume a [`SimPlot`]
//! already produced by [`crate::transient::run_tran`]. The transient
//! solver picks its own timesteps so the input signal is non-uniformly
//! sampled; both analyses resample to a uniform grid using linear
//! interpolation before transforming.

use thevenin_types::{Complex, SimPlot};

use cirq_ir::{FftFormat, FftWindow};

/// Errors that can arise during Fourier post-processing.
#[derive(Debug, thiserror::Error)]
pub enum FourierError {
    #[error("plot has no `time` vector — `.four`/`.fft` need a transient result")]
    NoTimeVector,

    #[error("plot vector `{0}` not found")]
    VectorNotFound(String),

    #[error("vector `{0}` is complex; `.four`/`.fft` require real transient data")]
    ComplexVector(String),

    #[error("transient is too short for fundamental {fundamental:e} Hz (need at least one period)")]
    TooShortForFundamental { fundamental: f64 },

    #[error("requested window [{start:e}, {stop:e}] s is empty or invalid")]
    EmptyWindow { start: f64, stop: f64 },

    #[error("fundamental frequency must be positive (got {0})")]
    InvalidFundamental(f64),

    #[error("requested at least 2 points for FFT")]
    NotEnoughPoints,
}

// ---------------------------------------------------------------------------
// .four — discrete Fourier transform of harmonics
// ---------------------------------------------------------------------------

/// One row in a `.four` harmonic table.
///
/// `freq` is `k * fundamental`, `magnitude` is the harmonic amplitude as
/// reported by SPICE (peak amplitude, i.e. `2 * |X_k| / N` for `k > 0`),
/// and `phase_deg` is the phase in degrees. The DC component is carried
/// separately on [`FourResult`].
#[derive(Debug, Clone)]
pub struct FourHarmonic {
    pub index: usize,
    pub frequency: f64,
    pub magnitude: f64,
    pub normalised: f64,
    pub phase_deg: f64,
    pub normalised_phase_deg: f64,
}

/// Result of `.four` for a single vector.
#[derive(Debug, Clone)]
pub struct FourResult {
    pub vector: String,
    pub fundamental: f64,
    /// DC component (mean over the last fundamental period).
    pub dc: f64,
    /// Harmonic table including the fundamental (`index == 1`).
    pub harmonics: Vec<FourHarmonic>,
    /// Total harmonic distortion as a percentage:
    /// `100 * sqrt(sum_{k>=2} mag_k^2) / mag_1`.
    pub thd_percent: f64,
}

/// Default number of harmonics reported by ngspice when `.four` is invoked
/// without a `set nfreqs=...` override.
pub const DEFAULT_NUM_HARMONICS: usize = 9;

/// Run `.four` over one or more vectors of a transient plot.
pub fn four_analysis(
    plot: &SimPlot,
    fundamental: f64,
    vector_names: &[&str],
    num_harmonics: usize,
) -> Result<Vec<FourResult>, FourierError> {
    if !(fundamental > 0.0) {
        return Err(FourierError::InvalidFundamental(fundamental));
    }
    let times = real_vector(plot, "time")?;
    if times.len() < 2 {
        return Err(FourierError::TooShortForFundamental { fundamental });
    }
    let t_last = times[times.len() - 1];
    let period = 1.0 / fundamental;
    let t_start = t_last - period;
    if t_start < times[0] - 1e-12 * period {
        return Err(FourierError::TooShortForFundamental { fundamental });
    }

    // ngspice samples 10 points per harmonic. Match that heuristic so the
    // DFT has comfortable resolution for high-order harmonics.
    let samples_per_harmonic = 10usize;
    let min_samples = 100usize;
    let n = (num_harmonics * samples_per_harmonic).max(min_samples);

    let mut results = Vec::with_capacity(vector_names.len());
    for name in vector_names {
        let signal = real_vector(plot, name)?;
        let resampled = resample_uniform(times, signal, t_start, t_last, n);
        results.push(four_dft(name, fundamental, &resampled, num_harmonics));
    }
    Ok(results)
}

fn four_dft(name: &str, fundamental: f64, samples: &[f64], num_harmonics: usize) -> FourResult {
    let n = samples.len();
    let n_f = n as f64;
    let dc = samples.iter().sum::<f64>() / n_f;

    let mut harmonics = Vec::with_capacity(num_harmonics);
    let mut fundamental_mag = 0.0_f64;
    let mut fundamental_phase = 0.0_f64;
    for k in 1..=num_harmonics {
        let mut re = 0.0_f64;
        let mut im = 0.0_f64;
        for (i, &x) in samples.iter().enumerate() {
            let theta = 2.0 * std::f64::consts::PI * (k as f64) * (i as f64) / n_f;
            re += x * theta.cos();
            im -= x * theta.sin();
        }
        // SPICE convention: amplitude = 2|X_k| / N, phase = atan2(im, re).
        let mag = 2.0 * (re * re + im * im).sqrt() / n_f;
        let phase_deg = im.atan2(re).to_degrees();
        if k == 1 {
            fundamental_mag = mag;
            fundamental_phase = phase_deg;
        }
        harmonics.push(FourHarmonic {
            index: k,
            frequency: fundamental * k as f64,
            magnitude: mag,
            normalised: 0.0, // filled below
            phase_deg,
            normalised_phase_deg: 0.0,
        });
    }

    let safe_fund = if fundamental_mag.abs() < 1e-300 {
        1.0
    } else {
        fundamental_mag
    };
    for h in harmonics.iter_mut() {
        h.normalised = h.magnitude / safe_fund;
        h.normalised_phase_deg = h.phase_deg - fundamental_phase;
    }

    let thd_num = harmonics
        .iter()
        .skip(1)
        .map(|h| h.magnitude * h.magnitude)
        .sum::<f64>()
        .sqrt();
    let thd_percent = if fundamental_mag.abs() < 1e-300 {
        0.0
    } else {
        100.0 * thd_num / fundamental_mag
    };

    FourResult {
        vector: name.to_string(),
        fundamental,
        dc,
        harmonics,
        thd_percent,
    }
}

// ---------------------------------------------------------------------------
// .fft — windowed radix-2 FFT
// ---------------------------------------------------------------------------

/// One vector's `.fft` result.
#[derive(Debug, Clone)]
pub struct FftResult {
    pub vector: String,
    /// Frequency bins in Hz, length `n/2 + 1`.
    pub frequencies: Vec<f64>,
    /// Complex spectrum, length `n/2 + 1` (one-sided).
    pub values: Vec<Complex>,
    pub window: FftWindow,
    pub format: FftFormat,
    /// The actual sample count used (rounded up to the next power of two).
    pub n: usize,
}

/// Options for `.fft`.
#[derive(Debug, Clone)]
pub struct FftOptions {
    pub vectors: Vec<String>,
    /// Window start time in seconds. `None` → first time sample.
    pub start: Option<f64>,
    /// Window stop time in seconds. `None` → last time sample.
    pub stop: Option<f64>,
    /// Requested point count. Rounded up to the next power of two for the
    /// radix-2 FFT.
    pub npoints: usize,
    pub window: FftWindow,
    pub format: FftFormat,
}

impl Default for FftOptions {
    fn default() -> Self {
        Self {
            vectors: Vec::new(),
            start: None,
            stop: None,
            npoints: 1024,
            window: FftWindow::Hann,
            format: FftFormat::Magnitude,
        }
    }
}

/// Run `.fft` over one or more vectors of a transient plot.
pub fn fft_analysis(plot: &SimPlot, opts: &FftOptions) -> Result<Vec<FftResult>, FourierError> {
    if opts.npoints < 2 {
        return Err(FourierError::NotEnoughPoints);
    }
    let times = real_vector(plot, "time")?;
    if times.len() < 2 {
        return Err(FourierError::EmptyWindow {
            start: opts.start.unwrap_or(0.0),
            stop: opts.stop.unwrap_or(0.0),
        });
    }
    let t0 = opts.start.unwrap_or(times[0]).max(times[0]);
    let t1 = opts
        .stop
        .unwrap_or(times[times.len() - 1])
        .min(times[times.len() - 1]);
    if !(t1 > t0) {
        return Err(FourierError::EmptyWindow {
            start: t0,
            stop: t1,
        });
    }
    let n = opts.npoints.next_power_of_two();

    let mut results = Vec::with_capacity(opts.vectors.len());
    for name in &opts.vectors {
        let signal = real_vector(plot, name)?;
        let resampled = resample_uniform(times, signal, t0, t1, n);
        let windowed = apply_window(&resampled, opts.window);
        let spectrum = fft_radix2(&windowed);
        let half = n / 2 + 1;
        let dt = (t1 - t0) / (n as f64 - 1.0);
        let df = 1.0 / (dt * n as f64);
        let frequencies = (0..half).map(|k| k as f64 * df).collect();
        let values = spectrum[..half].to_vec();
        results.push(FftResult {
            vector: name.clone(),
            frequencies,
            values,
            window: opts.window,
            format: opts.format,
            n,
        });
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Window functions
// ---------------------------------------------------------------------------

/// Apply the chosen window to a sample buffer. Returns a new vector — the
/// input is left untouched (immutable-by-default style).
pub fn apply_window(samples: &[f64], window: FftWindow) -> Vec<f64> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    let n_f = (n - 1) as f64;
    let two_pi = 2.0 * std::f64::consts::PI;
    samples
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let w = match window {
                FftWindow::Rectangular => 1.0,
                FftWindow::Hann => 0.5 * (1.0 - (two_pi * i as f64 / n_f).cos()),
                FftWindow::Hamming => 0.54 - 0.46 * (two_pi * i as f64 / n_f).cos(),
                FftWindow::Blackman => {
                    0.42 - 0.5 * (two_pi * i as f64 / n_f).cos()
                        + 0.08 * (2.0 * two_pi * i as f64 / n_f).cos()
                }
                FftWindow::Bartlett => {
                    let half = n_f / 2.0;
                    1.0 - ((i as f64 - half) / half).abs()
                }
            };
            x * w
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Look up a real-valued vector by case-insensitive name. Accepts both
/// `"v(out)"` and `"V(out)"` styles. Aborts on complex data — `.four` and
/// `.fft` are defined only for transient (real) vectors.
fn real_vector<'a>(plot: &'a SimPlot, name: &str) -> Result<&'a [f64], FourierError> {
    let vec = plot
        .vector(name)
        .ok_or_else(|| FourierError::VectorNotFound(name.to_string()))?;
    vec.data
        .try_real()
        .ok_or_else(|| FourierError::ComplexVector(name.to_string()))
}

/// Linear interpolation onto a uniform `n`-sample grid over `[t0, t1]`.
///
/// The transient solver picks its own timesteps so the raw signal is
/// non-uniformly sampled. `.four` and `.fft` both require uniform sampling
/// before the transform — this is the resampler.
pub fn resample_uniform(times: &[f64], values: &[f64], t0: f64, t1: f64, n: usize) -> Vec<f64> {
    assert_eq!(times.len(), values.len(), "time/value length mismatch");
    assert!(times.len() >= 2, "need at least 2 sample points");
    assert!(n >= 1, "need at least 1 output point");

    let mut out = Vec::with_capacity(n);
    let step = if n == 1 {
        0.0
    } else {
        (t1 - t0) / (n as f64 - 1.0)
    };
    let mut cursor = 0usize;
    for i in 0..n {
        let t = t0 + step * i as f64;
        // Advance cursor so times[cursor] <= t <= times[cursor + 1].
        while cursor + 1 < times.len() - 1 && times[cursor + 1] < t {
            cursor += 1;
        }
        // Clamp to range.
        let lo = cursor.min(times.len() - 2);
        let hi = lo + 1;
        let (tl, th) = (times[lo], times[hi]);
        let (vl, vh) = (values[lo], values[hi]);
        let frac = if (th - tl).abs() < 1e-30 {
            0.0
        } else {
            ((t - tl) / (th - tl)).clamp(0.0, 1.0)
        };
        out.push(vl + frac * (vh - vl));
    }
    out
}

// ---------------------------------------------------------------------------
// Radix-2 iterative Cooley-Tukey FFT
// ---------------------------------------------------------------------------

/// Compute the FFT of a real-valued buffer whose length is a power of two.
///
/// Returns a complex vector of the same length (full spectrum). For
/// real-valued inputs the output is conjugate-symmetric; callers can
/// safely keep only the first `n/2 + 1` bins.
pub fn fft_radix2(samples: &[f64]) -> Vec<Complex> {
    let n = samples.len();
    assert!(n.is_power_of_two(), "FFT requires power-of-two length");
    let mut buf: Vec<Complex> = samples.iter().map(|&x| Complex::new(x, 0.0)).collect();
    fft_in_place(&mut buf, /* inverse */ false);
    buf
}

fn bit_reverse(mut x: usize, log2n: u32) -> usize {
    let mut r = 0usize;
    for _ in 0..log2n {
        r = (r << 1) | (x & 1);
        x >>= 1;
    }
    r
}

fn fft_in_place(buf: &mut [Complex], inverse: bool) {
    let n = buf.len();
    if n <= 1 {
        return;
    }
    let log2n = n.trailing_zeros();
    // Bit-reversal permutation.
    for i in 0..n {
        let j = bit_reverse(i, log2n);
        if j > i {
            buf.swap(i, j);
        }
    }
    // Iterative butterflies.
    let mut size = 2usize;
    while size <= n {
        let half = size / 2;
        let sign = if inverse { 1.0 } else { -1.0 };
        let theta = sign * 2.0 * std::f64::consts::PI / size as f64;
        let w_step = Complex::new(theta.cos(), theta.sin());
        let mut k = 0usize;
        while k < n {
            let mut w = Complex::new(1.0, 0.0);
            for j in 0..half {
                let u = buf[k + j];
                let t = c_mul(w, buf[k + j + half]);
                buf[k + j] = Complex::new(u.re + t.re, u.im + t.im);
                buf[k + j + half] = Complex::new(u.re - t.re, u.im - t.im);
                w = c_mul(w, w_step);
            }
            k += size;
        }
        size <<= 1;
    }
    if inverse {
        let scale = 1.0 / n as f64;
        for c in buf.iter_mut() {
            *c = Complex::new(c.re * scale, c.im * scale);
        }
    }
}

fn c_mul(a: Complex, b: Complex) -> Complex {
    Complex::new(a.re * b.re - a.im * b.im, a.re * b.im + a.im * b.re)
}

// ---------------------------------------------------------------------------
// Text output (matches ngspice's `.four` table layout within reason)
// ---------------------------------------------------------------------------

/// Format a `.four` result table similar to ngspice's console output.
pub fn format_four_table(result: &FourResult) -> String {
    let mut s = String::new();
    s.push_str(&format!("Fourier analysis for {}:\n", result.vector));
    s.push_str(&format!(
        "  No. Harmonics: {}, THD: {:.6} %, Gridsize: 200, Interpolation Degree: 1\n\n",
        result.harmonics.len(),
        result.thd_percent
    ));
    s.push_str(&format!(
        "Harmonic   Frequency       Magnitude       Phase           Norm. Mag       Norm. Phase\n"
    ));
    s.push_str(&format!(
        "--------   ---------       ---------       -----           ---------       -----------\n"
    ));
    s.push_str(&format!(
        " 0         {:.6}      {:.6}       {:>10.6}     {:>10}     {:>10}\n",
        0.0_f64, result.dc, 0.0_f64, "0", "0"
    ));
    for h in &result.harmonics {
        s.push_str(&format!(
            " {:<8}  {:.6e}    {:.6e}    {:>10.4}     {:.6e}    {:>10.4}\n",
            h.index, h.frequency, h.magnitude, h.phase_deg, h.normalised, h.normalised_phase_deg
        ));
    }
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use thevenin_types::{SimPlot, SimVector};

    fn synth_plot(times: Vec<f64>, vectors: Vec<(&str, Vec<f64>)>) -> SimPlot {
        let mut vecs = vec![SimVector::real("time", times)];
        for (name, data) in vectors {
            vecs.push(SimVector::real(name, data));
        }
        SimPlot {
            name: "tran1".into(),
            vecs,
        }
    }

    #[test]
    fn pure_sine_has_unit_fundamental_no_harmonics() {
        // 1 kHz pure sine, 0–10 ms (10 periods). Linear-interpolation
        // resampling onto the uniform DFT grid leaks a small amount of
        // power into higher harmonics, so the test tolerates ~5 % leakage.
        let n = 2001;
        let dt = 10e-3 / (n as f64 - 1.0);
        let times: Vec<f64> = (0..n).map(|i| i as f64 * dt).collect();
        let freq = 1_000.0;
        let signal: Vec<f64> = times
            .iter()
            .map(|t| (2.0 * std::f64::consts::PI * freq * t).sin())
            .collect();
        let plot = synth_plot(times, vec![("v(out)", signal)]);

        let results = four_analysis(&plot, freq, &["v(out)"], 9).unwrap();
        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert!((r.dc).abs() < 1e-2, "dc = {}", r.dc);
        assert!(
            (r.harmonics[0].magnitude - 1.0).abs() < 5e-2,
            "fundamental mag = {}",
            r.harmonics[0].magnitude
        );
        for h in &r.harmonics[1..] {
            assert!(
                h.magnitude < 5e-2,
                "harmonic {} mag = {}",
                h.index,
                h.magnitude
            );
        }
        assert!(r.thd_percent < 5.0, "thd = {}", r.thd_percent);
    }

    #[test]
    fn distorted_signal_reports_second_harmonic() {
        // sin(2π·1k·t) + 0.5·sin(2π·2k·t). Expect harmonic[2]/harmonic[1] ≈ 0.5.
        let n = 4001;
        let dt = 10e-3 / (n as f64 - 1.0);
        let times: Vec<f64> = (0..n).map(|i| i as f64 * dt).collect();
        let f0 = 1_000.0;
        let signal: Vec<f64> = times
            .iter()
            .map(|t| {
                (2.0 * std::f64::consts::PI * f0 * t).sin()
                    + 0.5 * (2.0 * std::f64::consts::PI * 2.0 * f0 * t).sin()
            })
            .collect();
        let plot = synth_plot(times, vec![("v(out)", signal)]);

        let r = four_analysis(&plot, f0, &["v(out)"], 5).unwrap().remove(0);
        let h1 = r.harmonics[0].magnitude;
        let h2 = r.harmonics[1].magnitude;
        assert!((h1 - 1.0).abs() < 5e-2, "h1 = {h1}");
        assert!((h2 - 0.5).abs() < 5e-2, "h2 = {h2}");
        let ratio = h2 / h1;
        assert!((ratio - 0.5).abs() < 5e-2, "h2/h1 ratio = {ratio}");
    }

    #[test]
    fn fft_npoints_rounds_to_power_of_two() {
        let n = 4096;
        let dt = 1.0 / n as f64;
        let times: Vec<f64> = (0..n).map(|i| i as f64 * dt).collect();
        let signal: Vec<f64> = times
            .iter()
            .map(|t| (2.0 * std::f64::consts::PI * 100.0 * t).cos())
            .collect();
        let plot = synth_plot(times, vec![("v(out)", signal)]);

        let opts = FftOptions {
            vectors: vec!["v(out)".into()],
            npoints: 1000, // not a power of two; should round up to 1024
            ..FftOptions::default()
        };
        let results = fft_analysis(&plot, &opts).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].n, 1024);
        assert_eq!(results[0].frequencies.len(), 1024 / 2 + 1);
    }

    #[test]
    fn fft_hann_window_peak_at_expected_bin() {
        // Cosine at exactly bin 8 of a 256-sample 1-second window.
        let n = 256;
        let dt = 1.0 / n as f64;
        let times: Vec<f64> = (0..n).map(|i| i as f64 * dt).collect();
        let bin = 8.0;
        let signal: Vec<f64> = times
            .iter()
            .map(|t| (2.0 * std::f64::consts::PI * bin * t).cos())
            .collect();
        let plot = synth_plot(times, vec![("v(out)", signal)]);
        let opts = FftOptions {
            vectors: vec!["v(out)".into()],
            npoints: n,
            window: FftWindow::Hann,
            ..FftOptions::default()
        };
        let r = fft_analysis(&plot, &opts).unwrap().remove(0);

        // Find the peak bin.
        let (peak_idx, _) = r
            .values
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.magnitude().partial_cmp(&b.1.magnitude()).unwrap())
            .unwrap();
        assert_eq!(peak_idx as i64, bin as i64);

        // Hann reduces amplitude by 0.5 for a coherent cosine.
        // |X[k]| at the bin ≈ 0.5 * N * 0.5 (half-amplitude) = N/4.
        let peak_mag = r.values[peak_idx].magnitude();
        let expected = (n as f64) * 0.25;
        assert!(
            (peak_mag / expected - 1.0).abs() < 0.1,
            "peak {peak_mag} vs expected {expected}"
        );
    }

    #[test]
    fn rectangular_window_is_identity() {
        let xs = vec![1.0, 2.0, 3.0, 4.0];
        let ys = apply_window(&xs, FftWindow::Rectangular);
        assert_eq!(ys, xs);
    }

    #[test]
    fn hann_window_endpoints_zero() {
        let xs = vec![1.0; 8];
        let ys = apply_window(&xs, FftWindow::Hann);
        assert!(ys[0].abs() < 1e-12);
        assert!(ys[ys.len() - 1].abs() < 1e-12);
    }

    #[test]
    fn fft_radix2_dc_signal() {
        let xs = vec![1.0; 8];
        let spec = fft_radix2(&xs);
        assert!((spec[0].re - 8.0).abs() < 1e-12);
        assert!(spec[0].im.abs() < 1e-12);
        for c in &spec[1..] {
            assert!(c.magnitude() < 1e-10);
        }
    }

    #[test]
    fn fft_radix2_inverse_roundtrip() {
        let xs: Vec<Complex> = (0..16)
            .map(|i| Complex::new((i as f64).sin(), 0.0))
            .collect();
        let mut buf = xs.clone();
        fft_in_place(&mut buf, false);
        fft_in_place(&mut buf, true);
        for (a, b) in xs.iter().zip(buf.iter()) {
            assert!((a.re - b.re).abs() < 1e-10);
            assert!((a.im - b.im).abs() < 1e-10);
        }
    }

    #[test]
    fn resample_uniform_linear() {
        let times = vec![0.0, 1.0, 2.0];
        let values = vec![0.0, 10.0, 20.0];
        let xs = resample_uniform(&times, &values, 0.0, 2.0, 5);
        assert!((xs[0] - 0.0).abs() < 1e-12);
        assert!((xs[2] - 10.0).abs() < 1e-12);
        assert!((xs[4] - 20.0).abs() < 1e-12);
    }

    #[test]
    fn missing_vector_errors() {
        let plot = synth_plot(vec![0.0, 1.0], vec![("v(a)", vec![0.0, 1.0])]);
        let err = four_analysis(&plot, 1.0, &["v(missing)"], 4).unwrap_err();
        assert!(matches!(err, FourierError::VectorNotFound(_)));
    }

    #[test]
    fn invalid_fundamental_rejected() {
        let plot = synth_plot(vec![0.0, 1.0], vec![("v(a)", vec![0.0, 1.0])]);
        let err = four_analysis(&plot, 0.0, &["v(a)"], 4).unwrap_err();
        assert!(matches!(err, FourierError::InvalidFundamental(_)));
    }
}
