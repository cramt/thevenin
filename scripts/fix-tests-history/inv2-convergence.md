# inv2 Convergence Investigation (2026-04-04)

## Circuit

Floating-body SOI CMOS inverter (`bsim3soidd/inv2.cir`, also FD/PD variants):

```
vin in 0 dc 2.5
vdd dd 0 dc 2.5
vss ss 0 dc 0
ve  e  0 dc 1.25
m1 out in dd e p1 w=20u l=0.25u   ; PMOS pull-up
m2 out in ss e n1 w=10u l=0.25u   ; NMOS pull-down
.option itl1=500 gmin=1e-25 noacct
.dc vin 0 2.5 0.01
```

Both transistors have **no body contact** (floating body). The circuit specifies
an extremely small `gmin=1e-25`.

## Symptom

All four NR convergence strategies fail:
- Direct NR: 500 iterations (ITL1)
- Dynamic gmin stepping: stuck at `diag_gmin ≈ 3.88e-4`
- New gmin stepping: stuck at `dev_gmin ≈ 3.36e-4`
- Source stepping Phase 2: stuck at same bifurcation

ngspice converges this circuit and produces a clean inverter transfer
characteristic (251 sweep points, V(out) transitions from 2.5V to 0V).

## Root Cause Chain

### 1. Negative body node diagonal from impact ionization

At the DC operating point with Vin=2.5V (NMOS on, PMOS off), the NMOS (m2)
has significant impact ionization. The body node diagonal element is:

```
gbbb = -(gbbs) = -(gii_b - gbs_jct - gbd_jct)
```

Measured values at the intermediate NR state:

| Quantity      | Value      | Meaning                                  |
|---------------|------------|------------------------------------------|
| `gii_b`      | 8.81e-14   | dIii/dVbs (impact ionization body deriv)  |
| `gbs_jct`    | 4.19e-15   | dIbs/dVbs (source junction conductance)   |
| `gbd_jct`    | ≈ 0        | dIbd/dVbs (drain junction, deep reverse)  |
| `gbbs`       | +8.39e-14  | Net body current sensitivity (positive!)  |
| **`gbbb`**   | **-8.39e-14** | **Body diagonal is NEGATIVE**          |

A negative diagonal means the body node has **positive feedback**: increasing
Vbs increases the net current flowing *into* the body, which raises Vbs further.
This is the classic SOI "kink effect" — impact ionization generates holes that
charge the floating body, lowering threshold, increasing drain current, which
increases impact ionization.

### 2. NR limit cycle from body voltage limiting

The body voltage is limited to ±0.2V per iteration (matching ngspice
`B3SOIDDlimit`), and `SmartVbs` clamps Vbs ≥ 0 for floating body in DC.

When the body diagonal is negative, the NR solver wants to push the body
voltage far from the current value (the linear system says "go to -∞" or
"+∞"). The 0.2V limit clamps this. But the output node (Vout) responds to
the body change by swinging ~1.5V per iteration (through the drain current
feedback). This creates a stable **limit cycle**:

```
At dev_gmin = 3.278e-4:
  iter 0: max_diff = 0.27V
  iter 1: max_diff = 1.48V
  iter 2: max_diff = 1.49V   ← locked into oscillation
  iter 3: max_diff = 1.49V
  ...

At dev_gmin = 3.940e-4:
  iter 0: max_diff = 0.014V
  iter 1: max_diff = 2.1e-5  ← converges
```

The oscillation amplitude increases as gmin decreases. At `dev_gmin ≈ 3.36e-4`,
the extra regularization from gmin is no longer enough to damp the body-output
feedback loop.

### 3. Source stepping Phase 1 works; Phase 2 fails

Source stepping ramps all independent sources from 0 → full with elevated
gmin = 1e-2. This **works perfectly** — all 11 steps converge in 2-5 iterations.
At elevated gmin, the body-source coupling `(1e-2 * 1e-6) = 1e-8` easily
overwhelms the negative `gbbb ≈ -8.4e-14`, keeping the body diagonal positive.

Phase 2 then tries to reduce gmin from 1e-2 to the target (1e-25). A direct
jump to gmin=0 fails (500 iterations of oscillation). Gradual reduction also
gets stuck at `gmin ≈ 3.36e-4`, same as standalone gmin stepping.

### 4. At the converged solution, the instability vanishes

At the true converged operating point:
- Vin=2.5V → Vout ≈ 0V (NMOS on, PMOS off)
- NMOS: Vds ≈ 0V → `diff_vdsii ≈ 0` → `Iii ≈ 0` → `gii_b ≈ 0`
- Body diagonal becomes positive: `gbbb ≈ gbs_jct > 0`

The instability only exists at **intermediate NR states** where Vout hasn't
settled to its final value yet. The solver needs to get *past* the unstable
region to reach the stable converged point.

## What ngspice does differently

### NR loop mode transitions (NIiter.c lines 296-360)

ngspice's NR loop has three phases:
1. **MODEINITJCT** (1 iteration): device initialization with built-in potentials
2. **MODEINITFIX** (until convergence): normal NR but device `Check` flag is
   **ignored** — only global convergence (NIconvTest) matters
3. **MODEINITFLOAT** (until convergence): full NR with device convergence checks

Our code jumps directly from InitJct to Float after one iteration. We don't have
MODEINITFIX. However, since we don't implement device-level `Check` at all (our
convergence is purely global), this shouldn't be the key difference.

### Node damping (NIiter.c lines 296-322)

ngspice has optional node damping (`CKTnodeDamping`) that scales voltage updates
when any node changes by > 10V. This is **off by default** and not enabled by
the inv2 circuit, so it's not the mechanism ngspice uses here.

### gillespie source stepping (cktop.c)

ngspice's `gillespie_src` is the source stepping implementation. We haven't
compared our `source_stepping` Phase 2 gmin reduction with ngspice's post-source
stepping gmin handling in detail. This is the most likely area where ngspice
handles the transition differently.

### new_gmin steps CKTgmin, not CKTdiagGmin (cktop.c line 379)

In ngspice's `new_gmin()`, `CKTgmin` itself is temporarily elevated (from 1e-2
stepping down to `.option gmin`). During stepping, device models see the elevated
`CKTgmin`, so body_gmin = `CKTgmin * 1e-6` = `(elevated) * 1e-6`. The diagonal
`CKTdiagGmin` is NOT changed (stays at 0 for DC OP).

Our `new_gmin_stepping` matches this — `dev_gmin` is stepped while `diag_gmin`
stays at `options.diag_gmin`. The load closure passes `max(dev_gmin, options.gmin)`
to device stamps. So the mechanism is correct, but the stepping still gets stuck
at the bifurcation point.

## Fixes applied (committed)

### 1. SOI body node initialization (simulate.rs)

Added post-solve body node voltage initialization in `jct_initial_guess` for
floating-body BSIM3SOI-DD/FD/PD devices: `result[body_int_idx] = result[source_idx]`.
This prevents the immediate singular matrix that occurred before when the body
node had no conductance coupling at all.

### 2. Body gmin floor (bsim3soi_{dd,fd,pd}.rs)

Changed floating-body gmin coupling from `gmin * 1e-6` to `(gmin * 1e-6).max(1e-20)`.
When circuit gmin = 1e-25, the unflored value of 1e-31 is too small to provide
any regularization.

### 3. new_gmin_stepping zero-start + InitJct (newton.rs)

Zeroed the solution vector before new_gmin stepping (matching ngspice which zeros
`CKTrhsOld` and `CKTstate0`) and used InitJct mode for the first step.

### 4. Full chain-rule impact ionization derivatives (bsim3soi_dd.rs)

Replaced simplified `gii_b = t1 * gmbs` with ngspice's full decomposed chain-rule
computation including `dVdseffii/dV*` derivatives and `T1*Gm0 + Ids*dT1_dVg` /
`T1*Gmb0 + Ids*dT1_dVb` decomposition. The simplified version missed the
`Ids * dT1/dV` terms from how the ionization field depends on terminal voltages
through the Vdseffii → Vdsatii chain.

## Approaches tried that didn't work

### NR solution damping

Added voltage-node damping (scale update when max_diff > 3.5V) to `try_nr`.
This broke fourbitadder transient — damped voltages create inconsistent states
that produce singular matrices on the next load evaluation. Damping only voltage
nodes (not branch currents) didn't help either.

### Larger body_gmin floor (1e-12)

Increasing body_gmin to 1e-12 made things worse — at high device gmin during
stepping, the 1e-12 floor fights the device physics and prevents convergence
at gmin levels where it previously worked.

### Forced source stepping for floating-body SOI

Source stepping Phase 1 works perfectly (2-5 iters/step). But Phase 2 (gmin
reduction from 1e-2 to 1e-25) hits the same bifurcation. Accepting the
Phase 1 solution (at elevated gmin) fixes the initial OP but later DC sweep
points still fail when they hit the body instability region.

## Key breakthrough: ngspice's gillespie_src has NO gmin reduction

Reading ngspice's `gillespie_src` (cktop.c lines 481-658) revealed the
fundamental difference: **ngspice never reduces gmin after source stepping**.

```c
// ngspice cktop.c line 643 — after source ramp completes:
ckt->CKTdiagGmin = ckt->CKTgmin = gminstart;  // just restore, no reduction!
```

ngspice ramps sources 0→100% at the **target gmin** (1e-25 for inv2), with
adaptive step sizing (starting at 0.1%, backing off on failure). Since gmin
is never elevated, there's no Phase 2 reduction and no bifurcation.

This was implemented and works for the initial DC OP. Source stepping reaches
src=1.0 successfully with ~45 steps, each converging in 2-5 iterations.

### Current status after gillespie rewrite

**Works**: Initial DC OP converges via gillespie source stepping.

**Still fails**: DC sweep points near the inverter threshold (Vin ≈ 0.86V)
fail when the NR fallback chain reaches source stepping. The second gillespie
attempt gets stuck at src ≈ 0.34 (Vdd=0.85V) where the inverter is in the
transition region and body instability is strongest.

**Root cause of sweep failure**: DC sweep continuation points that fail NR go
through `newton_raphson_solve_with_mode` → gmin_stepping → new_gmin → source
stepping. Source stepping starts from scratch (zeros the solution), losing the
previous sweep point's solution. At partial source levels near the threshold,
the body instability prevents convergence.

## Remaining work to un-ignore inv2

### Step 1: Force gillespie for first DC sweep point (HIGH PRIORITY)

In `simulate.rs` `solve_nonlinear_op_with_guess`, when `initial_guess.is_none()`
and the circuit has floating-body SOI devices, use `source_stepping_solve` instead
of `newton_raphson_solve_with_mode`. This ensures the initial OP uses gillespie.

```rust
// simulate.rs around line 566
let has_floating_soi = mna.bsim3soi_dds.iter().any(|b| b.body_idx.is_none())
    || mna.bsim3soi_fds.iter().any(|b| b.body_idx.is_none())
    || mna.bsim3soi_pds.iter().any(|b| b.body_idx.is_none());
let force_source_stepping =
    (has_tlines && !mna.mosfets.is_empty()) || many_nonlinear
    || (has_floating_soi && initial_guess.is_none());
```

### Step 2: Fix DC sweep continuation convergence (MAIN CHALLENGE)

DC sweep continuation points use `newton_raphson_solve_with_mode` with the
previous solution as initial guess. When a point fails, the fallback chain
(gmin → new_gmin → source stepping) all fail because of the body bifurcation.

**Approach A — Skip failing sweep points**: ngspice's `dctrcurv.c` has logic
to skip non-converging sweep points and continue from the last good solution.
Study `ngspice-upstream/src/spicelib/analysis/dctrcurv.c` and implement similar
skip-and-continue logic in `simulate.rs`'s DC sweep loop.

**Approach B — Use elevated gmin for sweep continuation**: For sweep points
that fail direct NR, retry with a slightly elevated gmin (e.g. 1e-6) just for
that point. The elevated gmin regularizes the body without significantly
affecting voltage accuracy. This is a pragmatic approach that avoids the full
bifurcation.

**Approach C — Continuation method**: Instead of independent NR at each sweep
point, use a continuation/arc-length method that tracks the solution path
through the bifurcation region. This is the most robust but most complex
approach.

### Step 3: Performance optimization (LOW PRIORITY)

The gillespie rewrite caused fourbitadder to slow from 12s to 21s because
the adaptive stepping starts at 0.1% instead of 10% fixed steps. Options:

- Use the old elevated-gmin source stepping as primary, gillespie as fallback
  (only when the old approach fails)
- Start gillespie with a larger initial raise (e.g. 0.01 = 1%) for circuits
  where gmin is not pathologically small
- Add a fast path: if gmin >= 1e-15, use the old 10-step approach

## Key code locations

| File | Lines | What |
|------|-------|------|
| `thevenin/src/bsim3soi_dd.rs` | 2495-2525 | Impact ionization Iii and gii_* derivatives |
| `thevenin/src/bsim3soi_dd.rs` | 2558-2571 | Body current derivatives gbbs/gbds/gbgs/gbes |
| `thevenin/src/bsim3soi_dd.rs` | 3070-3074 | Body gmin coupling stamp |
| `thevenin/src/bsim3soi_dd.rs` | 3086-3110 | Body node Jacobian matrix stamps |
| `thevenin/src/bsim3soi_dd.rs` | 3260-3298 | Voltage limiting (0.2V body, SmartVbs) |
| `thevenin/src/newton.rs` | 310-400 | new_gmin_stepping |
| `thevenin/src/newton.rs` | 427-585 | source_stepping (gillespie algorithm) |
| `thevenin/src/simulate.rs` | 467-590 | solve_nonlinear_op_with_guess |
| `thevenin/src/simulate.rs` | ~860-920 | DC sweep loop (sweep continuation) |
| `ngspice cktop.c` | 481-658 | gillespie_src (reference implementation) |
| `ngspice dctrcurv.c` | — | DC sweep with convergence failure handling |
