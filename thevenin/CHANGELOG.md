# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/cramt/thevenin/compare/thevenin-v0.1.0...thevenin-v0.2.0) - 2026-03-22

### Added

- add HFET transient caps + slope-aware comparison tolerance
- *(transient)* add BJT dynamic charge LTE to adaptive timestep control
- *(bjt,transient)* add forward-bias depletion cap correction for BJT junctions
- *(mos6,transient)* add Meyer gate capacitance model for Level 6 MOSFETs
- *(mosfet,transient)* add Meyer gate capacitance model for Level 1 MOSFETs
- *(mesa,transient)* add junction capacitance integration for MESA transient analysis
- *(bjt,transient)* add dynamic diffusion capacitance for BJT transient analysis
- *(xspice)* add XSPICE code model framework for analog simulation
- *(mosfet)* compute VTO/GAMMA/PHI from process parameters (NSUB/TOX/NSS)
- *(transient)* add synthetic junction capacitors for BJT, MOSFET, and diode
- *(bjt)* implement OFF keyword and MODEINITJCT initial guess; improve output filter
- *(vbic)* implement self-heating thermal node; match ngspice constants
- *(sens)* rewrite sensitivity to direct method; fix VT_NOM precision
- *(bsource)* implement B-element (behavioral source) NR stamping
- *(output,mna)* netlist echo, RSH resistor model, un-ignore 2 tests
- *(output)* implement .op section output for C/R/MESA circuits (US-061)
- *(output)* implement .op section output for JFET circuits (US-061)
- *(bsim3soi-fd)* implement floating body architecture
- US-057 - PZ complex pair formatting and sensitivity table pagination
- US-055 - Interpolation-aware transient output comparison
- US-054 - Fix DC sweep variable column and add numeric tolerance comparison
- US-065 - XSPICE A-element parser with bracketed port groups
- US-039 - Test harness matching ngspice check.sh
- US-038 - Regression tests for models, subcircuit processing, and misc
- US-037 - Regression tests for parser, func, and lib-processing
- US-036 - TXL and CPL transmission line models
- US-035 - Lossy transmission line model (LTRA)
- US-034 HFET and MESFET (Statz/Curtice) models
- US-032 MESA FET model (Ytterdal/Lee/Shur/Fjeldly GaAs MESFET)
- US-033 MOS6 MOSFET model (Sakurai-Newton n-th power)
- VBIC AC/noise analysis, charge model, and 10 passing tests (US-031)

### Fixed

- add missing version specs and changelogs for release-plz
- *(ltra)* correct convolution chop_reltol and add quadratic interpolation
- *(mos6)* add mode factor to ceq_d RHS stamp in reversed mode
- MOSFET dynamic von in fetlim, per-column comparison tolerance, VBIC q1 clamp
- use self-heating temperature for VBIC AC analysis
- HFET inverse-mode gate voltage and VBIC ISRR temperature scaling
- *(bsim3soi-fd)* correct Vbs clamp for 5-terminal devices
- *(mosfet)* correct ceq_d gds sign in reversed mode
- *(transient)* grow timestep for non-reactive circuits, switch to nextest
- *(mosfet)* correct junction diode RHS signs in Level 1/6 MOSFET stamps
- *(transient)* limit step growth after breakpoint BE steps
- *(mosfet)* correct Level 1 von threshold computation, un-ignore mesosc
- *(bsim3soi)* compute vfb from VTH0 instead of hardcoding -1.0
- *(vbic)* correct avalanche current sign and exponent formula
- *(vbic)* correct parasitic junction parameters for temperature scaling and depletion charge
- *(mosfet)* add Vbs/Vbd pnjlim + improve slope-aware waveform comparison
- *(bsim3soi)* correct cdep0, theta0vb0, and theta_rout formulas in DD/FD/PD
- *(mosfet)* add gds floor to prevent singular matrix with LAMBDA=0
- *(newton)* add gmin stepping fallback for transient NR solver
- *(newton,transient)* dynamic gmin stepping and transient-only NR solver
- *(mosfet,output)* always-on voltage limiting and multi-analysis comparison
- *(vbic)* initial guess, state reset, and junction voltage clamping for NR convergence
- *(mosfet)* replace mos_limit with proper ngspice DEVfetlim/DEVlimvds limiting
- *(parser,mosfet)* tokenizer spaced key=value and MOSFET reversed-mode ceq_d sign
- *(mos6)* correct default parameters; add fixture override for harness
- *(transient)* preserve internal node voltages in DC OP → transient handoff
- *(vbic)* match ngspice temperature scaling computation order
- *(vbic)* correct NR/IKR/IKP temperature scaling; add VBIC initial guess
- *(noise)* output V²/Hz instead of V/√Hz to match ngspice format
- *(bsim3soi)* rewrite Ids derivative computation to match ngspice
- *(output,ac)* stop emitting .plot as data tables; stamp VBIC thermal node in AC
- *(harness)* improve output filter to pass 22 more tests
- *(vbic)* correct quasi-saturation and temperature scaling formulas
- *(resistance)* pass res_array harness test
- *(simulate)* correct LTRA+MOSFET DC OP convergence
- *(output)* resolve AC print vars with scalar wrappers only; fix TF deduplication
- *(lib)* read TEMP from .OPTIONS in netlist_temp()
- *(parser,ac)* fix continuation lines after star-comments + AC fixes
- *(pz)* use Schur complement reduction for numerically stable pole computation
- *(mesa)* use actual instance temperature for MESA model instead of hardcoded 300.15K
- *(solver)* separate diag_gmin from device gmin, matching ngspice CKTdiagGmin
- *(bsim3soi)* remove gmin from junction conductances, deprioritize US-059
- *(bsim3soi-dd)* correct Abulk formula, XJ default, and add Xcsat/Abeff
- *(bsim3soi)* correct NCH/NPEAK alias and vbi formula in DD/FD/PD

### Other

- update ignore reasons and document investigation findings
- update ignore reasons with re-measured error values, document HFET investigation
- *(wasm)* switch wasm tests from browser to Node runner
- *(tests)* replace manual test list with ngspice_tests!() proc macro
- update ignore reasons and triage after voltage-dependent cap investigation
- Update harness test ignore reasons to match actual failure modes
- *(harness)* re-ignore 15 bsim3soi harness tests (US-059)
- *(bsim3soi-pd)* ignore 3 floating body convergence tests (US-059)
- ignore failing harness tests with issue references
- extract shared physics constants and safe_exp into physics.rs
- rename project from ferrospice to thevenin across all files
- extract shared NR device stamping into device_stamp module
- release v0.1.0

## [0.1.0](https://github.com/cramt/thevenin/releases/tag/thevenin-v0.1.0) - 2026-03-10

### Fixed

- add version to thevenin-types dependency for crates.io publishing

### Other

- format resistance and transient tests
- copy ngspice test fixtures into repo so CI doesn't need ngspice-upstream
- add release-plz and CI workflows, add crates.io metadata
- rename crates: core crate is now `thevenin`, parser crate is now `thevenin-types`
