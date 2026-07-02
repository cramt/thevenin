# HiSIM2 full-port plan (checklist A1)

Working reference for porting the HiSIM2 (LEVEL=68) surface-potential MOSFET
from ngspice (`ngspice-upstream/src/spicelib/devices/hisim2/`) into
`thevenin/src/hisim.rs`. Line numbers are into the ngspice-45 source vendored
under `ngspice-upstream/` (gitignored — clone ngspice to follow along).

The existing `hisim.rs` ships a *simplified* surface-potential core that is
75–92% off ngspice on the golden corpus (see Phase 0). This plan replaces it
with a faithful port, driven by the golden TDD harness.

## TDD harness (Phase 0 — DONE)

- `scripts/gen-hisim-golden.sh` → ngspice-45 (nixpkgs, real HiSIM2) golden CSVs.
- `thevenin/tests/fixtures/hisim2/{nch.model,idvds.csv,idvgs.csv,idvbs.csv}`.
- `thevenin/tests/hisim_golden.rs` compares `HisimModel::companion().cdrain`
  against the reference (`#[ignore]`'d; run with `--ignored`).

**Key strategy — finite-difference Jacobian.** The golden test compares only
`cdrain` at exact bias points, which depends solely on the *forward* model.
Implement a forward-only `eval_ids(vgs,vds,vbs) -> f64` faithfully, then build
the companion's `gm`/`gds`/`gmbs` by finite-differencing it (central diff,
h≈1e-4·max(1,|V|)). This gives Jacobians automatically consistent with
`cdrain` (exactly what outer-loop NR needs) and removes the need to hand-port
ngspice's hundreds of analytic-derivative lines. Refine to analytic only if a
profile shows the 4× forward-eval cost matters (it won't for correctness).

## Constants (Phase 1 — `hsm2temp.c`)

All at the effective channel doping `Nsub`. For the long-channel core the
pocket-implant blend can start as `Nsub ≈ NSUBC`; add the blend in Phase 3.

| Quantity | Formula | ngspice |
|---|---|---|
| `beta` | `q/(kB·T)` (≈38.68 /V at 300.15K) | temp.c:564 |
| `beta_inv` | `1/beta` (= Vt) | |
| `Nin` | `C_Nin0·Tratio^1.5·exp(...)`, `C_Nin0=1.04e16` m⁻³ (hsm2evalenv.h — an earlier revision of this table wrongly said 1.45e16) | temp.c:572 |
| `Cox` | `εox/TOX` (`εox=3.9·ε0`) | |
| `Nsub` (long-ch) | pocket blend `(NSUBC·(Lg−LP)+Nsubps·LP)/Lg`; start ≈NSUBC | temp.c:363 |
| `cnst0` | `sqrt(2·εsi·q·Nsub/beta)` (εsi=11.7·ε0) | temp.c:645 |
| `cnst1` | `(Nin/Nsub)²` | temp.c:648 |
| `Pb2` | `(2/beta)·ln(Nsub/Nin)` (= 2φB) | temp.c:653 |
| `Vfb` | `VFBC` (param, directly) | temp.c:1136 |
| `fac1` | `cnst0/Cox` (the body-charge coefficient) | eval.c:1548 |

## Surface potentials (Phase 2 — `hsm2eval.c`) — DONE 2026-07-02

> Phases 2 and 3 landed together (commit `f8995ec`): the CORECIP=1 default
> for VERSION=2.80 makes the SCE loop, pocket dVth, and CLM Lred mandatory
> for golden agreement. Result: ≤0.0011% max rel-err on all three golden
> sweeps. Phases 4 (charges/AC) and 5 (HiSIMHV) remain.

`Vgp = Vgs − Vfb − dVth` (long-channel: `dVth≈0`; SCE/pocket in Phase 3).

### Ps0 (source-side) NR — eval.c:3526–3593

Solve `Fs0(Ps0)=0`:
```
Chi  = beta·(Ps0 − Vbs)
# strong inversion branch (the common one):
fs01 = cnst1·(exp(beta·Ps0) − exp(beta·Vbs))      # eval.c:3572
fs02 = sqrt(Chi − 1 + fs01)                        # eval.c:3575
Fs0  = Vgp − Ps0 − fac1·fs02                        # eval.c:3579
Fs0_dPs0 = −1 − fac1·(beta + cnst1·beta·exp(beta·Ps0))/(2·fs02)
dPs0 = −Fs0/Fs0_dPs0   (limit |dPs0| ≤ dPlim)      # eval.c:3584
```
Initial guess `Ps0_iniA` — eval.c:3405–3408:
```
TX  = 1 + 4·(beta·(Vgp−Vbs) − 1)/(fac1²·beta²)
Ps0_iniA = Vgp + fac1²·beta·0.5·(1 − sqrt(max(TX,ε)))
```
There is also a subthreshold branch (`fs01 = cfs1·(exp(Chi)−1)`, eval.c:3567)
selected when `Chi` is small; `cfs1 = cnst1` essentially. Use the exp form.

### Psl (drain-side) — same residual at the drain

Compute `Vdseff` first (smooth clamp of Vds to Vdsat), eval.c:3915–3979:
`Vdseff = Vds / (1 + (Vds/Vdsat)^δ)^(1/δ)`-style. Then solve the same
`Fsl(Psl)=0` with `Vgp` shifted by `Vdseff`:
`Fsl = Vgp − Psl − fac1·sqrt(beta·(Psl−Vbs)−1 + cnst1·(exp(beta·Psl)−exp(beta·(Vbs+Vdseff))))`.

`Pds = Psl − Ps0`.

### Idd current — eval.c:4368–4460

```
Xi0  = beta·(Ps0 − Vbs) − 1   ;  Xi0p12 = sqrt(Xi0)
Xil  = beta·(Psl − Vbs) − 1   ;  Xilp32 = Xil^1.5
Eta  = beta·Pds/Xi0
Eta1 = Eta+1 ; Eta1p12=√Eta1 ; Eta1p32=Eta1p12·Eta1
Zeta12 = 1/(Eta1p12+1) ; Zeta32 = 1/(Eta1p32+1)
F00 = Zeta12/Xi0p12
F10 = (2/3)·Xi0p12·Zeta32·(3 + Eta·(3+Eta))
Fdd = beta·Cox·(Vgp + 1/beta − 0.5·(2·Ps0+Pds)) + beta·cnst0·(F00 − F10)
Idd = Pds·Fdd
```
(F30/F11 only needed for charges — eval.c:4415–4436.)

### Charges for mobility field — eval.c:4604, 4651–4663

`Qbu = beta·Qbnm/Fdd` (Qbnm at eval.c:4585), `Qiu = (2/3)·VgVt·Qinm/Qidn·Cox`
with `Alpha`, `VgVt = Vgp − Ps0` (verify against eval.c — search `VgVt =`).
`Qn0` = source-side inversion charge (for Lch/Eeff). For a first pass the
mobility field `Eeff` can use `Qbu`,`Qiu` directly.

### Universal mobility — eval.c:4792–4893

```
Eeff = (Eeff_coef_b·Qbu + Eeff_coef_i·Qiu)/(1 + Pdsz·NINVD)   # MUEPH coefs
Rns  = Qiu/(q·1e4)
Muun = 1 / ( 1/(MUECB0 + MUECB1·Rns/1e11)
             + MPHN0·Eeff^MUEPH0 + Eeff^MUESR/MUESR1 )   # then /1e4 → MKS
# velocity saturation (BB=2 for electrons):
Em = Muun·Ey ;  Ey = sqrt(TY² + (0.2·Vmax/Muun)²) ;  TY = Idd/(beta·Qn0·Lch)
Mu = Muun / (1 + (Em/Vmax)^BB)^(1/BB)
```
Default coefficients live in `hsm2mpar.c` (MUEPH0, MUESR, MPHN0, BB=2, NINVD…).

### Final current — eval.c:4900–4906

```
betaWL = Weff·(1/beta)/Lch          # Lch = Leff − Lred (CLM); long-ch Lred≈0
Ids0   = betaWL · Idd · Mu
cdrain = mode · Ids0                 # mode = ±1 (drain/source swap)
```

## Phases 3–5

- **Phase 3** — `dVth` short-channel (SCE/RSCE, eval.c ~1450–1840), pocket
  implant `Nsubp` blend, narrow-width, poly-depletion (`Vgp` correction),
  QME (quantum) `Tox` increase, CLM `Lred` (eval.c:4756). Validate W/L corners.
- **Phase 4** — terminal charges Qg/Qd/Qs/Qb (Qdrat, eval.c:4677–4690) +
  `hsm2acld.c`; wire into transient/AC. Validate C-V vs `.ac`.
- **Phase 5** — HiSIMHV (LEVEL=73): RDRIFT drift resistance, body R, breakdown
  on top of the core. Fix the inert `gmbs` path; un-`#[ignore]`
  `body_effect_reduces_current`.

## Default model parameters

Full default table is `hsm2mpar.c`. The golden card (`nch.model`) sets
TOX, NSUBC, NSUBP, VFBC, MUECB0/1, MUEPH1, MUESR1, VMAX, VERSION=2.80; all
other params take HiSIM2 defaults — port the defaults `from_params` needs as
they surface during validation.
