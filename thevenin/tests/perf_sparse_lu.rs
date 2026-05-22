//! Wall-time measurement for the sparse-LU code path before/after the
//! symbolic-reuse optimisation lands.
//!
//! Not a real Criterion benchmark — just a `#[test]` that runs known-large
//! ngspice fixtures N times and reports total wall time, plus the count of
//! `LinearSystem::solve()` invocations and average per-solve cost.
//!
//! Gate with `THEVENIN_PERF_BENCH=1` so normal `cargo test` doesn't pay
//! 10× the harness cost — these are slow, opt-in.
//!
//! Run with:
//!
//! ```sh
//! THEVENIN_PERF_BENCH=1 cargo nextest run -p thevenin --test perf_sparse_lu \
//!     --no-capture
//! ```
//!
//! Or via the harness invocation pattern this file mimics:
//!
//! ```sh
//! cargo test -p thevenin --test perf_sparse_lu -- --nocapture --ignored
//! ```

use std::path::Path;
use std::time::Instant;

use cirq_spice_import::import_spice;
use thevenin_control::{execute_control_block_ir, has_control_block_ir};

/// Number of times to repeat each fixture so the wall time is large enough
/// to be measurable above noise. 5 gives ~10-15s per fixture which is plenty
/// of signal for ±5% comparisons.
const REPS: usize = 5;

fn enabled() -> bool {
    std::env::var("THEVENIN_PERF_BENCH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

struct BenchResult {
    elapsed: std::time::Duration,
    /// `(tiny<16, small<48, sparse>=48)` solve counts during this run.
    solves: (usize, usize, usize),
    /// `(dense_solve_ns, sparse_solve_ns, stamp_ns)` cumulative time
    /// spent in each phase, summed across all threads.
    phase_ns: (u64, u64, u64),
    /// `(cache_hits, cache_misses)` for the sparse symbolic LU cache.
    cache: (usize, usize),
    /// `(complex_dense_count, complex_sparse_count)` for AC/noise/pz solves.
    complex_solves: (usize, usize),
    /// `(complex_dense_ns, complex_sparse_ns)` cumulative complex-solve time.
    complex_phase_ns: (u64, u64),
    /// `(bypass_hits, bypass_misses)` for nonlinear-device companion bypass.
    bypass: (usize, usize),
}

fn run_fixture(path: &str) -> BenchResult {
    let abs_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(path);
    let src = std::fs::read_to_string(&abs_path)
        .unwrap_or_else(|e| panic!("read {}: {}", abs_path.display(), e));
    let circuits = import_spice(&src).expect("import_spice");

    thevenin::sparse::reset_solve_trace();
    let start = Instant::now();
    for _ in 0..REPS {
        if has_control_block_ir(&circuits[0]) {
            let r = execute_control_block_ir(&circuits[0]).expect("control");
            assert_eq!(r.exit_code, 0, "control quit nonzero");
        } else {
            // Dispatch each analysis via the IR-shape top-level
            // simulator (mirrors harness logic).
            for circuit in &circuits {
                let _ = thevenin::circuit::simulate(circuit).expect("simulate");
            }
        }
    }
    BenchResult {
        elapsed: start.elapsed(),
        solves: thevenin::sparse::solve_trace_counts(),
        phase_ns: thevenin::sparse::solve_phase_nanos(),
        cache: thevenin::sparse::sparse_cache_counts(),
        complex_solves: thevenin::sparse::complex_solve_counts(),
        complex_phase_ns: thevenin::sparse::complex_solve_phase_nanos(),
        bypass: thevenin::sparse::bypass_counts(),
    }
}

fn report(name: &str, r: BenchResult) {
    let (tiny, small, sparse) = r.solves;
    let total = tiny + small + sparse;
    let (dense_ns, sparse_ns, stamp_ns) = r.phase_ns;
    let total_ns = r.elapsed.as_nanos() as u64;
    let pct = |part: u64| {
        if total_ns == 0 {
            0.0
        } else {
            100.0 * part as f64 / total_ns as f64
        }
    };
    let (hits, misses) = r.cache;
    let hit_rate = if hits + misses == 0 {
        0.0
    } else {
        100.0 * hits as f64 / (hits + misses) as f64
    };
    let (cx_dense, cx_sparse) = r.complex_solves;
    let (cx_dense_ns, cx_sparse_ns) = r.complex_phase_ns;
    let (bp_hits, bp_misses) = r.bypass;
    let bypass_rate = if bp_hits + bp_misses == 0 {
        0.0
    } else {
        100.0 * bp_hits as f64 / (bp_hits + bp_misses) as f64
    };
    eprintln!(
        "{name}: {} reps in {:?} (avg {:?}/rep)\n  solves: total={total} tiny<16={tiny} small<48={small} sparse>=48={sparse}\n  phases: dense_LU={:.1}% ({:.2}ms)  sparse_LU={:.1}% ({:.2}ms)  device_stamp={:.1}% ({:.2}ms)  other={:.1}%\n  sparse-LU cache: hits={hits} misses={misses} ({:.1}% hit rate)\n  complex solves: dense={cx_dense} ({:.2}ms) sparse={cx_sparse} ({:.2}ms)\n  device bypass: hits={bp_hits} misses={bp_misses} ({:.1}% hit rate)",
        REPS,
        r.elapsed,
        r.elapsed / REPS as u32,
        pct(dense_ns),
        dense_ns as f64 / 1e6,
        pct(sparse_ns),
        sparse_ns as f64 / 1e6,
        pct(stamp_ns),
        stamp_ns as f64 / 1e6,
        100.0 - pct(dense_ns) - pct(sparse_ns) - pct(stamp_ns),
        hit_rate,
        cx_dense_ns as f64 / 1e6,
        cx_sparse_ns as f64 / 1e6,
        bypass_rate,
    );
}

#[test]
fn perf_rca3040() {
    if !enabled() {
        eprintln!("skipped (set THEVENIN_PERF_BENCH=1 to enable)");
        return;
    }
    report(
        "rca3040",
        run_fixture("ngspice-upstream/tests/general/rca3040.cir"),
    );
}

#[test]
fn perf_fourbitadder_transient() {
    if !enabled() {
        eprintln!("skipped (set THEVENIN_PERF_BENCH=1 to enable)");
        return;
    }
    report(
        "fourbitadder/transient",
        run_fixture("ngspice-upstream/tests/transient/fourbitadder.cir"),
    );
}

#[test]
fn perf_mosamp() {
    if !enabled() {
        eprintln!("skipped (set THEVENIN_PERF_BENCH=1 to enable)");
        return;
    }
    report(
        "mosamp",
        run_fixture("ngspice-upstream/tests/general/mosamp.cir"),
    );
}

/// MOS6 bypass fixture: 20 Level 6 MOSFETs in an inverter chain,
/// `.tran 0.5n 150n`. Exercises the MOS6 companion-bypass path which
/// shares the cache-and-tolerance pattern with Level 1 / Level 2.
#[test]
fn perf_mos6inv() {
    if !enabled() {
        eprintln!("skipped (set THEVENIN_PERF_BENCH=1 to enable)");
        return;
    }
    report(
        "mos6inv",
        run_fixture("ngspice-upstream/tests/mos6/mos6inv.cir"),
    );
}

/// MOSFET-bypass fixture: 13 Level 1 MOSFETs in a memory cell, `.tran 20ns 2us`.
/// Used to validate the device-companion bypass (CKTbypass) actually saves
/// model evaluations on a real Level 1 MOSFET-heavy workload. mosamp uses
/// Level 2 and so doesn't exercise the Level 1 bypass path.
#[test]
fn perf_mosmem() {
    if !enabled() {
        eprintln!("skipped (set THEVENIN_PERF_BENCH=1 to enable)");
        return;
    }
    report(
        "mosmem",
        run_fixture("ngspice-upstream/tests/general/mosmem.cir"),
    );
}

/// AC-heavy fixture: VBIC differential amplifier with a `dec 25 1e5..1e9`
/// sweep (~100 points) over 13 BJTs. Exercises the per-frequency reassembly
/// path so the AC stamp-cache change has a clear signal: pre-cache each
/// freq point re-ran `stamp_ac_devices` over every device, post-cache it
/// clones cached triplets and scales the imag ones by ω.
#[test]
fn perf_vbic_diffamp_ac() {
    if !enabled() {
        eprintln!("skipped (set THEVENIN_PERF_BENCH=1 to enable)");
        return;
    }
    report(
        "vbic/diffamp",
        run_fixture("ngspice-upstream/tests/vbic/diffamp.cir"),
    );
}
