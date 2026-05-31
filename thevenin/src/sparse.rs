use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use faer::Mat;
use faer::linalg::solvers::{FullPivLu, PartialPivLu, Solve};
use faer::sparse::SparseColMatRef;
use faer::sparse::linalg::solvers::{Lu as FaerLu, SymbolicLu};
use faer::sparse::{SparseColMat, Triplet as FaerTriplet};

// --- Perf instrumentation ---------------------------------------------------
//
// Tracks the number of `LinearSystem::solve()` invocations bucketed by
// matrix dimension AND the cumulative nanoseconds spent inside each major
// phase so we can tell where a workload actually spends its time before
// chasing optimisations there. Counters update unconditionally on every
// solve; they're only consulted via `solve_trace_counts` /
// `solve_phase_nanos`.

static SOLVE_COUNT_TINY: AtomicUsize = AtomicUsize::new(0); // dim < 16
static SOLVE_COUNT_SMALL: AtomicUsize = AtomicUsize::new(0); // 16 <= dim < SPARSE_THRESHOLD
static SOLVE_COUNT_SPARSE: AtomicUsize = AtomicUsize::new(0); // dim >= SPARSE_THRESHOLD
/// Complex (AC / noise / pole-zero) solve invocations, total and by bucket.
static COMPLEX_SOLVE_COUNT_DENSE: AtomicUsize = AtomicUsize::new(0); // dim < SPARSE_THRESHOLD
static COMPLEX_SOLVE_COUNT_SPARSE: AtomicUsize = AtomicUsize::new(0); // dim >= SPARSE_THRESHOLD
/// Cumulative ns spent in the complex sparse / dense solve paths.
static COMPLEX_SOLVE_NANOS_DENSE: AtomicU64 = AtomicU64::new(0);
static COMPLEX_SOLVE_NANOS_SPARSE: AtomicU64 = AtomicU64::new(0);

/// Cumulative ns spent inside the dense LU branch of `solve` (matrix
/// densification + FullPivLu + solve + result extraction).
static SOLVE_NANOS_DENSE: AtomicU64 = AtomicU64::new(0);
/// Cumulative ns spent inside the sparse LU branch of `solve`.
static SOLVE_NANOS_SPARSE: AtomicU64 = AtomicU64::new(0);
/// Cumulative ns spent inside the device-stamping callback in `try_nr`.
/// Recorded externally via [`record_stamp_nanos`] so the sparse module
/// doesn't depend on newton.rs.
static STAMP_NANOS: AtomicU64 = AtomicU64::new(0);
/// Global counters that aggregate cache activity across every
/// `SparseLuCache` instance, so the bench can report a single hit/miss
/// number without threading caches around.
static GLOBAL_CACHE_HITS: AtomicUsize = AtomicUsize::new(0);
static GLOBAL_CACHE_MISSES: AtomicUsize = AtomicUsize::new(0);

/// Nonlinear-device companion-bypass hit/miss counters (CKTbypass).
static BYPASS_HITS: AtomicUsize = AtomicUsize::new(0);
static BYPASS_MISSES: AtomicUsize = AtomicUsize::new(0);

/// Increment the companion-bypass hit counter. Called from each device
/// family's stamping loop when the cached companion is reused.
pub fn record_bypass_hit() {
    BYPASS_HITS.fetch_add(1, Ordering::Relaxed);
}

/// Increment the companion-bypass miss counter. Called when the cached
/// companion is invalid or terminal voltages have moved beyond tolerance.
pub fn record_bypass_miss() {
    BYPASS_MISSES.fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the device-bypass hit/miss counters since process start (or
/// the last `reset_solve_trace`).
pub fn bypass_counts() -> (usize, usize) {
    (
        BYPASS_HITS.load(Ordering::Relaxed),
        BYPASS_MISSES.load(Ordering::Relaxed),
    )
}

fn record_solve_dim(dim: usize) {
    let bucket = if dim < 16 {
        &SOLVE_COUNT_TINY
    } else if dim < SPARSE_THRESHOLD {
        &SOLVE_COUNT_SMALL
    } else {
        &SOLVE_COUNT_SPARSE
    };
    bucket.fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the per-bucket solve counts since process start (or the last
/// `reset_solve_trace`). Returns `(tiny, small, sparse)`.
pub fn solve_trace_counts() -> (usize, usize, usize) {
    (
        SOLVE_COUNT_TINY.load(Ordering::Relaxed),
        SOLVE_COUNT_SMALL.load(Ordering::Relaxed),
        SOLVE_COUNT_SPARSE.load(Ordering::Relaxed),
    )
}

/// Snapshot the cumulative ns spent in each phase. Returns
/// `(dense_solve_ns, sparse_solve_ns, stamp_ns)`.
pub fn solve_phase_nanos() -> (u64, u64, u64) {
    (
        SOLVE_NANOS_DENSE.load(Ordering::Relaxed),
        SOLVE_NANOS_SPARSE.load(Ordering::Relaxed),
        STAMP_NANOS.load(Ordering::Relaxed),
    )
}

/// Snapshot the complex (AC / noise / pole-zero) solve counts, returning
/// `(dense, sparse)`.
pub fn complex_solve_counts() -> (usize, usize) {
    (
        COMPLEX_SOLVE_COUNT_DENSE.load(Ordering::Relaxed),
        COMPLEX_SOLVE_COUNT_SPARSE.load(Ordering::Relaxed),
    )
}

/// Snapshot the complex (AC / noise / pole-zero) solve phase nanos, returning
/// `(dense_ns, sparse_ns)`.
pub fn complex_solve_phase_nanos() -> (u64, u64) {
    (
        COMPLEX_SOLVE_NANOS_DENSE.load(Ordering::Relaxed),
        COMPLEX_SOLVE_NANOS_SPARSE.load(Ordering::Relaxed),
    )
}

/// Add `ns` to the device-stamping bucket. Called from `try_nr` around
/// each invocation of the `load_system` closure.
pub fn record_stamp_nanos(ns: u64) {
    STAMP_NANOS.fetch_add(ns, Ordering::Relaxed);
}

/// Snapshot the global sparse-LU cache hit/miss counters since process
/// start (or the last `reset_solve_trace`).
pub fn sparse_cache_counts() -> (usize, usize) {
    (
        GLOBAL_CACHE_HITS.load(Ordering::Relaxed),
        GLOBAL_CACHE_MISSES.load(Ordering::Relaxed),
    )
}

/// Reset all solve-trace counters to zero. Useful between bench runs.
pub fn reset_solve_trace() {
    SOLVE_COUNT_TINY.store(0, Ordering::Relaxed);
    SOLVE_COUNT_SMALL.store(0, Ordering::Relaxed);
    SOLVE_COUNT_SPARSE.store(0, Ordering::Relaxed);
    SOLVE_NANOS_DENSE.store(0, Ordering::Relaxed);
    SOLVE_NANOS_SPARSE.store(0, Ordering::Relaxed);
    STAMP_NANOS.store(0, Ordering::Relaxed);
    GLOBAL_CACHE_HITS.store(0, Ordering::Relaxed);
    GLOBAL_CACHE_MISSES.store(0, Ordering::Relaxed);
    COMPLEX_SOLVE_COUNT_DENSE.store(0, Ordering::Relaxed);
    COMPLEX_SOLVE_COUNT_SPARSE.store(0, Ordering::Relaxed);
    COMPLEX_SOLVE_NANOS_DENSE.store(0, Ordering::Relaxed);
    COMPLEX_SOLVE_NANOS_SPARSE.store(0, Ordering::Relaxed);
    BYPASS_HITS.store(0, Ordering::Relaxed);
    BYPASS_MISSES.store(0, Ordering::Relaxed);
}
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SparseMatrixError {
    #[error("matrix is singular, cannot solve")]
    Singular,
    #[error("singular matrix: {0}")]
    SingularMatrix(String),
    #[error(
        "dimension mismatch: matrix is {matrix_dim}x{matrix_dim} but RHS has {rhs_len} elements"
    )]
    DimensionMismatch { matrix_dim: usize, rhs_len: usize },
}

/// Dimension threshold: systems smaller than this use dense full-pivoting LU;
/// larger systems use sparse LU for O(nnz) instead of O(n^3).
const SPARSE_THRESHOLD: usize = 48;

/// A triplet (row, col, value) for assembling a sparse matrix.
#[derive(Debug, Clone, Copy)]
pub struct Triplet {
    pub row: usize,
    pub col: usize,
    pub value: f64,
}

/// A sparse matrix builder using triplet-form (COO) assembly.
///
/// Supports dynamic insertion of entries. Duplicate (row, col) entries
/// are summed together during assembly, which is the standard behavior
/// for MNA stamp accumulation.
#[derive(Debug, Clone)]
pub struct SparseMatrix {
    dim: usize,
    triplets: Vec<Triplet>,
}

impl SparseMatrix {
    /// Create a new sparse matrix of the given dimension.
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            triplets: Vec::new(),
        }
    }

    /// Return the dimension (number of rows/columns).
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Add a value to position (row, col). If multiple values are added
    /// at the same position, they are summed (standard MNA stamp behavior).
    ///
    /// Zero-valued stamps are kept — they create a structural placeholder at
    /// `(row, col)` so the matrix's nonzero pattern stays stable across NR
    /// iterations even when a device transitions through exactly zero. The
    /// stable pattern is what lets `SparseLuCache` reuse the symbolic
    /// factorization across iterations (which is the dominant cost for
    /// sparse-LU workloads — see `tests/perf_sparse_lu.rs`).
    ///
    /// # Panics
    /// Panics if row or col >= dim.
    pub fn add(&mut self, row: usize, col: usize, value: f64) {
        assert!(
            row < self.dim,
            "row {row} out of bounds for dim {}",
            self.dim
        );
        assert!(
            col < self.dim,
            "col {col} out of bounds for dim {}",
            self.dim
        );
        self.triplets.push(Triplet { row, col, value });
    }

    /// Convert triplet form to a dense faer matrix (summing duplicates).
    pub fn to_dense(&self) -> Mat<f64> {
        let mut mat = Mat::zeros(self.dim, self.dim);
        for t in &self.triplets {
            mat[(t.row, t.col)] += t.value;
        }
        mat
    }

    /// Convert to a faer sparse column matrix (CSC format).
    ///
    /// Duplicate entries at the same (row, col) are summed automatically by
    /// faer's `try_new_from_triplets`, matching MNA stamp accumulation semantics.
    fn to_sparse_col(&self) -> Result<SparseColMat<usize, f64>, SparseMatrixError> {
        let triplets: Vec<FaerTriplet<usize, usize, f64>> = self
            .triplets
            .iter()
            .map(|t| FaerTriplet::new(t.row, t.col, t.value))
            .collect();
        SparseColMat::try_new_from_triplets(self.dim, self.dim, &triplets).map_err(|e| {
            SparseMatrixError::SingularMatrix(format!("sparse matrix construction failed: {e}"))
        })
    }

    /// Clear all entries, keeping the dimension.
    pub fn clear(&mut self) {
        self.triplets.clear();
    }

    /// Iterate over the stored triplets.
    pub fn triplets(&self) -> &[Triplet] {
        &self.triplets
    }
}

impl thevenin_xspice::MatrixStamp for SparseMatrix {
    fn add(&mut self, row: usize, col: usize, value: f64) {
        self.add(row, col, value);
    }
}

/// Cached sparse-LU symbolic factorization that survives across NR
/// iterations.
///
/// For sparse-LU-dominated workloads (e.g. `fourbitadder`, where 94% of
/// runtime is in `sp_lu`), reusing the symbolic factor across NR iterations
/// turns each iteration's LU into a numeric-only refactorization — typically
/// 3-5× faster than computing symbolic + numeric every iteration.
///
/// The cache is keyed on a hash of the matrix sparsity pattern; when the
/// caller's structure changes (different `(row, col)` set after dedup), the
/// symbolic factor is automatically recomputed. NR iterations of a fixed
/// circuit topology always hit the cache.
///
/// Use via [`LinearSystem::solve_with_cache`].
#[derive(Default)]
pub struct SparseLuCache {
    /// `(pattern_hash, symbolic_factor)`. `None` until first warm-up.
    inner: Option<(u64, SymbolicLu<usize>)>,
    /// Hits / misses for tests + bench reporting.
    hits: usize,
    misses: usize,
}

impl SparseLuCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of times the cached symbolic was reused.
    pub fn hits(&self) -> usize {
        self.hits
    }

    /// Number of times the symbolic had to be (re-)computed.
    pub fn misses(&self) -> usize {
        self.misses
    }

    /// Discard the cached symbolic. Forces the next call through
    /// [`LinearSystem::solve_with_cache`] to recompute it.
    pub fn invalidate(&mut self) {
        self.inner = None;
    }
}

/// Hash a sparse matrix's CSC pattern (col_ptr + row_idx) into a single
/// u64 fingerprint. Two matrices with the same hash have (with overwhelming
/// probability) the same sparsity pattern.
fn pattern_hash(mat: SparseColMatRef<'_, usize, f64>) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write_usize(mat.ncols());
    hasher.write_usize(mat.nrows());
    for &p in mat.col_ptr() {
        hasher.write_usize(p);
    }
    for &r in mat.row_idx() {
        hasher.write_usize(r);
    }
    hasher.finish()
}

/// A linear system Ax = b assembled in triplet form.
#[derive(Debug, Clone)]
pub struct LinearSystem {
    pub matrix: SparseMatrix,
    pub rhs: Vec<f64>,
}

impl LinearSystem {
    /// Create a new linear system of the given dimension.
    pub fn new(dim: usize) -> Self {
        Self {
            matrix: SparseMatrix::new(dim),
            rhs: vec![0.0; dim],
        }
    }

    /// Return the dimension.
    pub fn dim(&self) -> usize {
        self.matrix.dim()
    }

    /// Solve the system using LU factorization.
    /// Returns the solution vector x such that Ax = b.
    ///
    /// For small systems (< 48 unknowns), uses dense partial-pivoting LU.
    /// For larger systems, uses sparse LU which exploits the O(N) sparsity
    /// pattern typical of MNA circuit matrices.
    /// Solve `Ax = b` reusing a cached sparse symbolic LU factor when
    /// possible.
    ///
    /// Wraps [`solve`](Self::solve), routing the sparse-path branch
    /// (`dim >= SPARSE_THRESHOLD`) through the cache. Dense-path systems
    /// fall through to the regular path; the cache is unused for them.
    /// Callers that only solve small systems get no speedup but pay nothing
    /// either (cache stays empty).
    ///
    /// Pattern stability: the cache validates the new matrix's sparsity
    /// against the one used to compute the cached symbolic. A pattern
    /// mismatch silently triggers a symbolic refactor (counted as a miss).
    /// Topology changes between NR iterations are rare; the typical NR
    /// loop hits the cache on every iteration after the first.
    pub fn solve_with_cache(
        &self,
        cache: &mut SparseLuCache,
    ) -> Result<Vec<f64>, SparseMatrixError> {
        self.solve_with_cache_refined(cache, false)
    }

    /// Like [`solve_with_cache`](Self::solve_with_cache) but optionally applies one pass of iterative
    /// refinement after the LU solve.
    ///
    /// Iterative refinement is cheap insurance against ill-conditioning:
    /// after computing `x = LU^{-1} b`, evaluate the residual `r = b - A*x`,
    /// solve `LU * dx = r` for the correction, then update `x = x + dx`.
    /// For well-conditioned matrices this barely moves `x` (the initial
    /// solve is already accurate to working precision); for ill-conditioned
    /// matrices it can recover 1-2 digits of accuracy at the cost of one
    /// extra triangular solve. We do exactly one refinement pass — the
    /// canonical "cheap insurance" form — rather than iterating to
    /// convergence.
    ///
    /// The factorization itself is reused (Arc-cloned via the cache), so
    /// the only additional cost is one matrix-vector product for the
    /// residual and one triangular solve for the correction.
    pub fn solve_with_cache_refined(
        &self,
        cache: &mut SparseLuCache,
        refine: bool,
    ) -> Result<Vec<f64>, SparseMatrixError> {
        let dim = self.matrix.dim();
        if self.rhs.len() != dim {
            return Err(SparseMatrixError::DimensionMismatch {
                matrix_dim: dim,
                rhs_len: self.rhs.len(),
            });
        }
        if dim == 0 {
            return Ok(vec![]);
        }

        // Dense path stays untouched — small systems don't benefit from
        // symbolic reuse (the symbolic computation is most of the LU at
        // that size). Avoid double-counting by deferring `record_solve_dim`
        // to `solve` for the dense branch.
        if dim < SPARSE_THRESHOLD {
            return self.solve_refined(refine);
        }
        record_solve_dim(dim);

        let t0 = Instant::now();
        let mut b = Mat::zeros(dim, 1);
        for (i, &val) in self.rhs.iter().enumerate() {
            b[(i, 0)] = val;
        }
        let sparse_mat = self.matrix.to_sparse_col()?;
        let hash = pattern_hash(sparse_mat.as_ref());

        // Look up the cached symbolic. If pattern matches, reuse it
        // (Arc clone is O(1)); otherwise rebuild.
        let symbolic = match &cache.inner {
            Some((cached_hash, sym)) if *cached_hash == hash => {
                cache.hits += 1;
                GLOBAL_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
                sym.clone()
            }
            _ => {
                cache.misses += 1;
                GLOBAL_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
                let sym = SymbolicLu::try_new(sparse_mat.as_ref().symbolic()).map_err(|e| {
                    SparseMatrixError::SingularMatrix(format!(
                        "symbolic LU factorization failed: {e}"
                    ))
                })?;
                cache.inner = Some((hash, sym.clone()));
                sym
            }
        };

        let lu = FaerLu::try_new_with_symbolic(symbolic, sparse_mat.as_ref()).map_err(|e| {
            SparseMatrixError::SingularMatrix(format!("numeric LU refactorization failed: {e}"))
        })?;
        let x = lu.solve(&b);
        let mut result: Vec<f64> = (0..dim).map(|i| x[(i, 0)]).collect();

        if refine {
            // Residual r = b - A * x, evaluated against the original triplet
            // form so we benefit from the same matrix the LU saw.
            let mut residual = vec![0.0; dim];
            for (i, &val) in self.rhs.iter().enumerate() {
                residual[i] = val;
            }
            for t in self.matrix.triplets() {
                residual[t.row] -= t.value * result[t.col];
            }
            let mut r_mat = Mat::zeros(dim, 1);
            for (i, &val) in residual.iter().enumerate() {
                r_mat[(i, 0)] = val;
            }
            let dx = lu.solve(&r_mat);
            for i in 0..dim {
                result[i] += dx[(i, 0)];
            }
        }

        SOLVE_NANOS_SPARSE.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
        if result.iter().any(|v: &f64| v.is_nan() || v.is_infinite()) {
            return Err(SparseMatrixError::Singular);
        }
        Ok(result)
    }

    pub fn solve(&self) -> Result<Vec<f64>, SparseMatrixError> {
        self.solve_refined(false)
    }

    /// Like [`solve`](Self::solve) but optionally applies one pass of iterative
    /// refinement.  See [`solve_with_cache_refined`](Self::solve_with_cache_refined)
    /// for rationale.  This path is used for one-shot (uncached) solves
    /// and for the dense branch.
    pub fn solve_refined(&self, refine: bool) -> Result<Vec<f64>, SparseMatrixError> {
        let dim = self.matrix.dim();
        if self.rhs.len() != dim {
            return Err(SparseMatrixError::DimensionMismatch {
                matrix_dim: dim,
                rhs_len: self.rhs.len(),
            });
        }

        if dim == 0 {
            return Ok(vec![]);
        }

        // Perf instrumentation: count solves per dim bucket. Used by
        // benches to validate which code path dominates before chasing
        // optimisations there. Atomic ops on the hot path are cheap; the
        // counters are only consulted when an explicit `solve_trace_counts`
        // call is made.
        record_solve_dim(dim);

        // Build faer column vector from rhs.
        let mut b = Mat::zeros(dim, 1);
        for (i, &val) in self.rhs.iter().enumerate() {
            b[(i, 0)] = val;
        }

        if dim < SPARSE_THRESHOLD {
            // Dense path: full pivoting (row + column permutations) provides
            // superior numerical stability for ill-conditioned MNA matrices
            // compared to partial pivoting (row only).  The extra cost of
            // full pivoting is negligible at these dimensions and matches
            // ngspice's Markowitz solver behaviour more closely for circuits
            // with negative conductances (e.g. HFET GGR terms) that make
            // the Jacobian ill-conditioned during NR convergence.
            let t0 = Instant::now();
            let a = self.matrix.to_dense();
            let lu = FullPivLu::new(a.as_ref());
            let x = lu.solve(&b);
            let mut result: Vec<f64> = (0..dim).map(|i| x[(i, 0)]).collect();

            if refine {
                let mut residual = self.rhs.clone();
                for t in self.matrix.triplets() {
                    residual[t.row] -= t.value * result[t.col];
                }
                let mut r_mat = Mat::zeros(dim, 1);
                for (i, &val) in residual.iter().enumerate() {
                    r_mat[(i, 0)] = val;
                }
                let dx = lu.solve(&r_mat);
                for i in 0..dim {
                    result[i] += dx[(i, 0)];
                }
            }

            SOLVE_NANOS_DENSE.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
            if result.iter().any(|v: &f64| v.is_nan() || v.is_infinite()) {
                return Err(SparseMatrixError::Singular);
            }
            Ok(result)
        } else {
            // Sparse path: exploits O(N) nonzero structure of circuit matrices.
            // For 500+ node circuits this is 10-100x faster than dense LU.
            let t0 = Instant::now();
            let sparse_mat = self.matrix.to_sparse_col()?;
            let lu = sparse_mat.sp_lu().map_err(|e| {
                SparseMatrixError::SingularMatrix(format!("sparse LU factorization failed: {e}"))
            })?;
            let x = lu.solve(&b);
            let mut result: Vec<f64> = (0..dim).map(|i| x[(i, 0)]).collect();

            if refine {
                let mut residual = self.rhs.clone();
                for t in self.matrix.triplets() {
                    residual[t.row] -= t.value * result[t.col];
                }
                let mut r_mat = Mat::zeros(dim, 1);
                for (i, &val) in residual.iter().enumerate() {
                    r_mat[(i, 0)] = val;
                }
                let dx = lu.solve(&r_mat);
                for i in 0..dim {
                    result[i] += dx[(i, 0)];
                }
            }

            SOLVE_NANOS_SPARSE.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
            if result.iter().any(|v: &f64| v.is_nan() || v.is_infinite()) {
                return Err(SparseMatrixError::Singular);
            }
            Ok(result)
        }
    }
}

/// A complex-valued linear system for AC analysis.
///
/// Uses separate real and imaginary sparse matrices (G + jB)
/// assembled in triplet form, solved as a single complex system.
#[derive(Debug, Clone)]
pub struct ComplexLinearSystem {
    dim: usize,
    /// Real part of the matrix (conductance G).
    pub real: SparseMatrix,
    /// Imaginary part of the matrix (susceptance B = wC etc.).
    pub imag: SparseMatrix,
    /// Real part of the RHS vector.
    pub rhs_real: Vec<f64>,
    /// Imaginary part of the RHS vector.
    pub rhs_imag: Vec<f64>,
}

impl ComplexLinearSystem {
    /// Create a new complex linear system of the given dimension.
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            real: SparseMatrix::new(dim),
            imag: SparseMatrix::new(dim),
            rhs_real: vec![0.0; dim],
            rhs_imag: vec![0.0; dim],
        }
    }

    /// Return the dimension.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Build a combined complex faer sparse matrix from real + imaginary triplets.
    fn to_complex_sparse_col(
        &self,
        transpose: bool,
    ) -> Result<SparseColMat<usize, faer::c64>, SparseMatrixError> {
        use faer::c64;

        let mut triplets: Vec<FaerTriplet<usize, usize, c64>> =
            Vec::with_capacity(self.real.triplets().len() + self.imag.triplets().len());

        if transpose {
            for t in self.real.triplets() {
                triplets.push(FaerTriplet::new(t.col, t.row, c64::new(t.value, 0.0)));
            }
            for t in self.imag.triplets() {
                triplets.push(FaerTriplet::new(t.col, t.row, c64::new(0.0, t.value)));
            }
        } else {
            for t in self.real.triplets() {
                triplets.push(FaerTriplet::new(t.row, t.col, c64::new(t.value, 0.0)));
            }
            for t in self.imag.triplets() {
                triplets.push(FaerTriplet::new(t.row, t.col, c64::new(0.0, t.value)));
            }
        }

        SparseColMat::try_new_from_triplets(self.dim, self.dim, &triplets).map_err(|e| {
            SparseMatrixError::SingularMatrix(format!(
                "complex sparse matrix construction failed: {e}"
            ))
        })
    }

    /// Solve the complex system using dense LU (for small systems).
    fn solve_dense(&self, transpose: bool) -> Result<Vec<(f64, f64)>, SparseMatrixError> {
        use faer::c64;

        COMPLEX_SOLVE_COUNT_DENSE.fetch_add(1, Ordering::Relaxed);
        let t0 = Instant::now();
        let mut mat = Mat::<c64>::zeros(self.dim, self.dim);
        if transpose {
            for t in self.real.triplets() {
                mat[(t.col, t.row)] += c64::new(t.value, 0.0);
            }
            for t in self.imag.triplets() {
                mat[(t.col, t.row)] += c64::new(0.0, t.value);
            }
        } else {
            for t in self.real.triplets() {
                mat[(t.row, t.col)] += c64::new(t.value, 0.0);
            }
            for t in self.imag.triplets() {
                mat[(t.row, t.col)] += c64::new(0.0, t.value);
            }
        }

        let mut b = Mat::<c64>::zeros(self.dim, 1);
        for i in 0..self.dim {
            b[(i, 0)] = c64::new(self.rhs_real[i], self.rhs_imag[i]);
        }

        let lu = PartialPivLu::new(mat.as_ref());
        let x = lu.solve(&b);

        let result: Vec<(f64, f64)> = (0..self.dim)
            .map(|i| (x[(i, 0)].re, x[(i, 0)].im))
            .collect();

        COMPLEX_SOLVE_NANOS_DENSE.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);

        if result
            .iter()
            .any(|(re, im)| re.is_nan() || re.is_infinite() || im.is_nan() || im.is_infinite())
        {
            return Err(SparseMatrixError::Singular);
        }

        Ok(result)
    }

    /// Solve the complex system using sparse LU (for larger systems).
    fn solve_sparse(&self, transpose: bool) -> Result<Vec<(f64, f64)>, SparseMatrixError> {
        use faer::c64;

        COMPLEX_SOLVE_COUNT_SPARSE.fetch_add(1, Ordering::Relaxed);
        let t0 = Instant::now();
        let sparse_mat = self.to_complex_sparse_col(transpose)?;
        let lu = sparse_mat.sp_lu().map_err(|e| {
            SparseMatrixError::SingularMatrix(format!(
                "complex sparse LU factorization failed: {e}"
            ))
        })?;

        let mut b = Mat::<c64>::zeros(self.dim, 1);
        for i in 0..self.dim {
            b[(i, 0)] = c64::new(self.rhs_real[i], self.rhs_imag[i]);
        }

        let x = lu.solve(&b);

        let result: Vec<(f64, f64)> = (0..self.dim)
            .map(|i| (x[(i, 0)].re, x[(i, 0)].im))
            .collect();

        COMPLEX_SOLVE_NANOS_SPARSE.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);

        if result
            .iter()
            .any(|(re, im)| re.is_nan() || re.is_infinite() || im.is_nan() || im.is_infinite())
        {
            return Err(SparseMatrixError::Singular);
        }

        Ok(result)
    }

    /// Solve the complex system (G + jB)x = (rhs_real + j*rhs_imag).
    /// Returns pairs of (real, imag) for each unknown.
    ///
    /// Uses dense partial-pivoting LU for small systems and sparse LU
    /// for larger systems.
    pub fn solve(&self) -> Result<Vec<(f64, f64)>, SparseMatrixError> {
        if self.dim == 0 {
            return Ok(vec![]);
        }

        if self.dim < SPARSE_THRESHOLD {
            self.solve_dense(false)
        } else {
            self.solve_sparse(false)
        }
    }

    /// Solve the adjoint (transposed) complex system (G + jB)^T x = rhs.
    /// Returns pairs of (real, imag) for each unknown.
    pub fn solve_transpose(&self) -> Result<Vec<(f64, f64)>, SparseMatrixError> {
        if self.dim == 0 {
            return Ok(vec![]);
        }

        if self.dim < SPARSE_THRESHOLD {
            self.solve_dense(true)
        } else {
            self.solve_sparse(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn test_3x3_system() {
        // Solve:
        //  2x + y - z = 8
        // -3x - y + 2z = -11
        // -2x + y + 2z = -3
        //
        // Solution: x=2, y=3, z=-1
        let mut sys = LinearSystem::new(3);

        sys.matrix.add(0, 0, 2.0);
        sys.matrix.add(0, 1, 1.0);
        sys.matrix.add(0, 2, -1.0);

        sys.matrix.add(1, 0, -3.0);
        sys.matrix.add(1, 1, -1.0);
        sys.matrix.add(1, 2, 2.0);

        sys.matrix.add(2, 0, -2.0);
        sys.matrix.add(2, 1, 1.0);
        sys.matrix.add(2, 2, 2.0);

        sys.rhs[0] = 8.0;
        sys.rhs[1] = -11.0;
        sys.rhs[2] = -3.0;

        let x = sys.solve().unwrap();
        assert_abs_diff_eq!(x[0], 2.0, epsilon = 1e-12);
        assert_abs_diff_eq!(x[1], 3.0, epsilon = 1e-12);
        assert_abs_diff_eq!(x[2], -1.0, epsilon = 1e-12);
    }

    #[test]
    fn test_triplet_sum_duplicates() {
        // Adding to the same position should sum values
        let mut m = SparseMatrix::new(2);
        m.add(0, 0, 3.0);
        m.add(0, 0, 4.0);
        let dense = m.to_dense();
        assert_abs_diff_eq!(dense[(0, 0)], 7.0, epsilon = 1e-15);
    }

    #[test]
    fn test_identity_solve() {
        // Identity matrix: solution equals RHS
        let mut sys = LinearSystem::new(3);
        sys.matrix.add(0, 0, 1.0);
        sys.matrix.add(1, 1, 1.0);
        sys.matrix.add(2, 2, 1.0);
        sys.rhs[0] = 5.0;
        sys.rhs[1] = -3.0;
        sys.rhs[2] = 7.0;

        let x = sys.solve().unwrap();
        assert_abs_diff_eq!(x[0], 5.0, epsilon = 1e-12);
        assert_abs_diff_eq!(x[1], -3.0, epsilon = 1e-12);
        assert_abs_diff_eq!(x[2], 7.0, epsilon = 1e-12);
    }

    #[test]
    fn test_singular_matrix() {
        // All zeros => singular
        let sys = LinearSystem::new(2);
        // rhs is [0, 0] and matrix is all zeros
        // This might solve to zeros rather than error, so let's make it clearly singular
        // with a non-zero rhs
        let mut sys2 = LinearSystem::new(2);
        sys2.rhs[0] = 1.0;
        let result = sys2.solve();
        assert!(
            result.is_err() || {
                // Some LU implementations return NaN/Inf for singular
                let x = result.unwrap();
                x.iter().any(|v| v.is_nan() || v.is_infinite())
            }
        );

        // Trivial zero system with zero rhs should still "solve" to zero
        let x = sys.solve().unwrap_or_else(|_| vec![0.0, 0.0]);
        // Either way is fine for a zero system
        let _ = x;
    }

    #[test]
    fn test_empty_system() {
        let sys = LinearSystem::new(0);
        let x = sys.solve().unwrap();
        assert!(x.is_empty());
    }

    #[test]
    fn test_complex_system_solve() {
        // Solve (1+j)*x = 2+j
        // x = (2+j)/(1+j) = (2+j)(1-j)/((1+j)(1-j)) = (2+j-2j-j^2)/(1+1) = (3-j)/2
        // x = 1.5 - 0.5j
        let mut sys = ComplexLinearSystem::new(1);
        sys.real.add(0, 0, 1.0);
        sys.imag.add(0, 0, 1.0);
        sys.rhs_real[0] = 2.0;
        sys.rhs_imag[0] = 1.0;

        let result = sys.solve().unwrap();
        assert_abs_diff_eq!(result[0].0, 1.5, epsilon = 1e-12);
        assert_abs_diff_eq!(result[0].1, -0.5, epsilon = 1e-12);
    }

    /// Iterative refinement on a well-conditioned dense-path system should
    /// produce essentially the same answer as no refinement — the initial
    /// solve is already at working precision, so the refinement correction
    /// is at noise level. This guards against the refinement pass corrupting
    /// good solutions.
    #[test]
    fn test_iterative_refinement_well_conditioned_matches_baseline() {
        let mut sys = LinearSystem::new(3);
        sys.matrix.add(0, 0, 2.0);
        sys.matrix.add(0, 1, 1.0);
        sys.matrix.add(0, 2, -1.0);
        sys.matrix.add(1, 0, -3.0);
        sys.matrix.add(1, 1, -1.0);
        sys.matrix.add(1, 2, 2.0);
        sys.matrix.add(2, 0, -2.0);
        sys.matrix.add(2, 1, 1.0);
        sys.matrix.add(2, 2, 2.0);
        sys.rhs[0] = 8.0;
        sys.rhs[1] = -11.0;
        sys.rhs[2] = -3.0;

        let baseline = sys.solve_refined(false).unwrap();
        let refined = sys.solve_refined(true).unwrap();

        for i in 0..3 {
            assert_abs_diff_eq!(baseline[i], refined[i], epsilon = 1e-13);
        }
        // And both should equal the analytical solution.
        assert_abs_diff_eq!(refined[0], 2.0, epsilon = 1e-12);
        assert_abs_diff_eq!(refined[1], 3.0, epsilon = 1e-12);
        assert_abs_diff_eq!(refined[2], -1.0, epsilon = 1e-12);
    }

    /// Iterative refinement on an ill-conditioned matrix (wide value spread)
    /// should produce a tighter residual than the unrefined solve. The
    /// matrix below mixes a tiny diagonal (1e-10) with O(1) off-diagonals;
    /// the unrefined LU loses precision on the small pivot, and one
    /// refinement pass cleans up the residual.
    #[test]
    fn test_iterative_refinement_ill_conditioned_improves_residual() {
        // Build a 64x64 system (sparse path) with a deliberately wide
        // dynamic range. Diagonal alternates between 1e-10 and 1e10; sub
        // and super diagonals are 1.0. The huge spread between diagonal
        // and off-diagonal entries stresses partial pivoting.
        let n = 64;
        let mut sys = LinearSystem::new(n);
        for i in 0..n {
            let diag = if i.is_multiple_of(2) { 1e10 } else { 1e-10 };
            sys.matrix.add(i, i, diag);
            if i > 0 {
                sys.matrix.add(i, i - 1, 1.0);
            }
            if i + 1 < n {
                sys.matrix.add(i, i + 1, 1.0);
            }
            sys.rhs[i] = 1.0;
        }

        let unrefined = sys.solve_refined(false).unwrap();
        let refined = sys.solve_refined(true).unwrap();

        // Compute residual ||Ax - b||_inf for each.
        let compute_residual = |x: &[f64]| -> f64 {
            let mut r = vec![0.0_f64; n];
            for i in 0..n {
                r[i] = -sys.rhs[i];
            }
            for t in sys.matrix.triplets() {
                r[t.row] += t.value * x[t.col];
            }
            r.iter().fold(0.0_f64, |acc, v| acc.max(v.abs()))
        };

        let res_unrefined = compute_residual(&unrefined);
        let res_refined = compute_residual(&refined);

        // Refinement should not make things worse, and on an ill-conditioned
        // matrix typically improves the residual by 1-2 orders of magnitude.
        assert!(
            res_refined <= res_unrefined,
            "refinement made residual worse: unrefined={res_unrefined:e}, refined={res_refined:e}"
        );
    }

    /// Ensure sparse LU path works for systems above the threshold.
    #[test]
    fn test_large_system_sparse_lu() {
        // Build a 64x64 tridiagonal system (above SPARSE_THRESHOLD).
        // -2 on diagonal, 1 on sub/super-diagonals (discretized Laplacian).
        let n = 64;
        let mut sys = LinearSystem::new(n);
        for i in 0..n {
            sys.matrix.add(i, i, -2.0);
            if i > 0 {
                sys.matrix.add(i, i - 1, 1.0);
            }
            if i + 1 < n {
                sys.matrix.add(i, i + 1, 1.0);
            }
            sys.rhs[i] = 1.0;
        }

        let x = sys.solve().unwrap();
        // Verify by checking residual: Ax - b should be near zero.
        for i in 0..n {
            let mut ax_i = -2.0 * x[i];
            if i > 0 {
                ax_i += x[i - 1];
            }
            if i + 1 < n {
                ax_i += x[i + 1];
            }
            assert_abs_diff_eq!(ax_i, 1.0, epsilon = 1e-10);
        }
    }
}
