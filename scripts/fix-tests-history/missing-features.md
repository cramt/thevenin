# Missing Features / Infrastructure Tests

## Current status (11 tests)

| Category | Count | Tests |
|---|---|---|
| .control: B-source nodes not in OP | 2 | asrc-tc-1 (v(3)), log-functions-1 (v(b1)) |
| .control: alter/resume commands | 2 | alter-vec (alter cmd), resume-1 (stop/resume) |
| .control: imaginary unit | 1 | ac-resistance (complex 'i' variable) |
| AC sensitivity not implemented | 2 | sens-ac-1/2 (.control works; simulate_sens only has DC path) |
| .control: model binning | 1 | binning-1 (BSIM4 bin selection) |
| .control: node naming | 2 | bxpressn-1 (B-source internal node), xpressn-3 (subcircuit internal node) |
| .control: parameter expressions | 1 | asrc-tc-2 (r={1k + v(9)}) |
| BSIM1/BSIM2 models | 2 | Entire models not implemented |

## Session 115 findings (2026-04-05)

### sens-ac-1/2: Root cause clarified — NOT a .control issue

**Discovered:** The ignore.toml labeled these as ".control: simulator accuracy issue" but the
actual root cause is that **AC sensitivity analysis is not implemented** at all:

1. **exec.rs parser strips "ac" keyword** (line 566-570): `sens v(1) ac lin 1 1e6 1.1e6`
   becomes `["v(1)", "lin", "1", "1e6", "1.1e6"]` — "ac" is silently discarded
2. **simulate_sens checks `output[1] == "ac"`** which is now `false` → falls through to DC path
3. Even if "ac" were preserved, the code returns `Err("AC sensitivity not yet supported")`

The .control interpreter works correctly — the issue is the missing AC sensitivity subsystem.

**Analytical verification:** For the sens-ac-1 circuit (I_dc=1.27A, R=1kΩ, C=100pF at 1MHz):
- DC sensitivity: dV(1)/dR = I_dc = 1.27 ← what our code returns
- AC sensitivity: dV(1)/dR = I_ac/(1 + jωRC)² = complex valued ← what ngspice computes
- The 720× discrepancy (1.27 vs 0.001764) is DC vs AC sensitivity at different operating points

**What's needed to fix:**
- Complex admittance matrix build (Y = G + jωC) in sens analysis
- Complex perturbation/solve (SMPcSolve equivalent)
- Complex result storage and reporting
- ~200-300 LOC of new AC sensitivity code in sens.rs

**What NOT to retry:** Simple numerical accuracy fixes or .control parser changes — the entire
AC sensitivity analysis path needs to be implemented.

### binning-1: Confirmed BSIM4 model binning issue, not .control

The .control interpreter runs correctly (executes `op` command). The failure is from BSIM4
model binning (`.1`/`.2` suffix bin selection by L/W) not being implemented in the model
parameter lookup code. Not a .control issue.
