# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/cramt/thevenin/compare/thevenin-v0.3.0...thevenin-v0.5.0) - 2026-07-12

### Added

- update ngspice submodule (1a621eb -> 037b657), pass its new convergence test
- *(hisim)* faithful HiSIM2 I-V core (Phases 2+3), un-ignore golden tests
- *(hisim)* faithful HiSIM2 constants + full-port plan (Phase 1)
- add GSHUNT option, complete the .options survey
- complete Goal C — SPICE importer accepts arbitrary netlists
- *(cirq)* native measure expression syntax (= expr form)
- *(expr)* short-circuit ternary `cond ? then : else`
- *(cirq)* ternary operator + sim-context constants (temper, time, freq, hertz)
- *(control)* source command, measure command, vector arithmetic in print
- *(options)* DEFAD/DEFAS/DEFL/DEFW + NOOPALTER + GMINPRIORITY
- *(numerics)* iterative refinement, TRTOL, PIVTOL/PIVREL
- *(meas)* ERR/ERR1/ERR2/ERR3 + IF conditional + FILE= output
- *(transient)* .options METHOD=trap|gear|euler with BDF2 for L/C
- *(urc)* SPICE U element + URC model with importer-side expansion
- *(hisim)* port HiSIM2 (LEVEL=68) surface-potential MOSFET (partial)
- *(bsim2)* port BSIM2 MOSFET (level 5) from ngspice - initial build
- *(bsim1)* port BSIM1 MOSFET (level 4) from ngspice
- *(fourier)* .four (DFT harmonics + THD) and .fft (windowed FFT) post-processing of .tran
- *(vdmos)* port vertical-DMOS power MOSFET from ngspice
- *(mos3)* port MOSFET Level 3 (semi-empirical short-channel) from ngspice
- *(output)* ngspice raw file format (binary + ASCII) + CSV + write command
- *(tline)* add T element (ideal lossless transmission line)
- *(sens)* AC sensitivity analysis (.sens v(...) ac ...) — tests and verification
- *(options)* RSHUNT, GMINSTEPS, NOOPITER
- *(importer)* MOS3 warning, line continuation, .step diagnostic, TEMPER + ternary in brace eval
- *(switches)* add S/W voltage- and current-controlled switches
- *(importer)* R/L/C tc=, option scale, .width, graceful unknown directives
- *(meas)* typed MeasureExpr in IR, add PARAM=/LAST/TD support
- *(control)* retire SimContext::netlist cache; dispatch .control via Circuit
- *(control)* implement stop/resume + unignore resume-1.cir (101/0/6)
- *(mna)* sens Netlist-free; multi-temp on Circuit; SimContext Netlist private
- *(mna)* add Circuit-input top-level simulate dispatcher; CLI uses it
- *(mna)* TF / PZ / Noise also fully Netlist-free on Circuit-input path
- *(mna)* add Circuit-input entry points for tf/pz/noise/sens
- *(mna)* transient analysis fully Netlist-free on Circuit-input path
- *(mna)* DC and AC analyses fully Netlist-free on Circuit-input path
- *(mna)* route harness through mna_ir; complete _with_mna surface
- *(mna)* route DC/AC/TRAN through mna_ir on Circuit-input path
- *(mna)* direct IR -> MNA path supports LTRA/TXL/CPL/XSPICE (full coverage)
- *(mna)* direct IR -> MNA path supports BehavioralSource + Coupling
- *(mna)* direct IR -> MNA path supports JFET / MESA family
- *(mna)* direct IR -> MNA path supports MOSFET family
- *(mna)* direct IR -> MNA path supports BJT (level 1 + VBIC)
- *(mna)* direct IR -> MNA path supports diodes
- *(mna)* share OP solve+format between Netlist and IR direct paths
- *(mna)* assemble MnaSystem directly from Cirq IR (linear subset)
- *(control)* add IR-shaped .control entry point (Stage 4 / Phase A)
- *(circuit)* extend direct IR -> MNA path to dependent sources
- *(circuit)* land direct IR -> MNA path for linear-only DC OP
- graduate Circuit-input simulation API into thevenin
- support BSIM4 model binning through Cirq IR round-trip
- make Cirq IR the default harness route, quarantine 5 round-trip failures
- route ngspice harness through Cirq IR via THEVENIN_VIA_CIRQ
- wire .ic/UIC, .nodeset, .meas, multi-temp, and mutual coupling

### Fixed

- guard two stack-overflow DoS paths reachable from untrusted input
- *(bjt)* remove double-counted substrate cap, un-ignore rtlinv — 0 ignored tests
- PULSE PER defaults to TSTOP; BJT substrate cap + absolute charge integration
- *(bsim3soidd)* honor debug=-1 quasi-static semantics, un-ignore RampVg2
- *(bjt)* use ngspice's CONSTKoverQ thermal voltage, not legacy SPICE3 KboQ
- *(bsim3soidd)* numerically stable Phi^1.5/Phi^2.5 differences in CAPMOD=3
- un-ignore hfet/inverter with corrected reference from patched ngspice
- *(bsim3soidd)* re-resolve XJ sentinel against the model card's TSI
- *(bsim1)* default NRD/NRS to 1 per ngspice b1set.c, un-ignore bsim1 harness fixture
- *(raw_output)* explicit plotnames for .four/.fft/.disto; flag unknown
- *(fourier)* ngspice 200-sample DFT grid, window gain, narrower visibility
- *(transient)* only count bootstrap-driven BE steps toward Gear budget
- *(hisim)* drop CLM unit double-conversion; demote checklist to partial
- *(vdmos)* use raw signed vds in lambda factor
- *(bsim1)* unconditional dvth_dvbs across Vbs=0 seam
- *(mna_ir)* repair merge artefact in nr_options_from_circuit boolean dispatch
- *(hisim)* residual-based convergence + flag in solve_surface_potential
- *(bsim2)* repair bad rebase merge of bsim1/bsim2 dispatch blocks
- *(bsim3soi-dd)* correct DELTA_VCSCV from 1e-5 to 4e-4 to match ngspice
- populate netlist.source on Cirq IR -> Netlist round-trip
- restore Expr::Brace on Cirq IR -> Netlist round-trip
- add RHS history coupling for mutual inductors in transient

### Other

- *(release)* unify workspace to 0.5.0 for first crates.io release
- un-ignore schmitt with bounded-error tolerance; add harness profile mode
- refresh HiSIM dispatch comments for the faithful port
- un-ignore bsim2 harness fixture, now passing against ngspice reference
- *(hisim)* add HiSIM2 golden-reference scaffold (Phase 0)
- IR-native source DC + waveforms, drop convert_source_spec from the simulator
- make mna_ir device loading IR-native, drop convert_model/extra_params
- [**breaking**] remove the legacy Netlist stamping path
- route in-crate unit tests through the IR path via test_support
- native ModelParams::from_ir, drop convert_model on Circuit device loads
- *(thevenin)* neutral ModelParams boundary, Expr-free device layer
- rustdoc polish, README refresh, getting-started guide
- polish docs.rs landing pages, examples, and metadata
- *(thevenin)* demote assemble_mna(&Netlist) to pub(crate)
- *(thevenin)* mark CircuitSimError #[non_exhaustive]
- *(release)* README pass, CHANGELOG, api-stability + #[non_exhaustive] on public enums
- *(bsim2)* set vbb=-3 in integration test model for proper body effect
- *(bsim2)* rustfmt single-line mos_limit calls
- *(bsim2)* adjust cutoff threshold for subthreshold conduction
- *(bsim2)* apply rustfmt + extend ignore.toml note for bsim2/test.cir
- *(bsim2)* add integration tests + update docs
- *(switch,fourier)* correct stale documentation
- *(fourier)* satisfy clippy neg_cmp_op_on_partial_ord + useless_format
- *(fourier)* relax pure-sine unit-test tolerances for interpolation leakage
- Merge importer hygiene (R tc=, option scale, .width, unknown directives)
- Merge doc archival
- archive cirq-plan and migration plans
- *(xspice)* add Circuit-input simulate_op_with_xspice, retire Netlist wrapper
- demote Netlist-shaped simulator APIs to pub(crate)
- *(stamp)* extend companion bypass to MOS6 (Level 6)
- *(stamp)* record why BJT companion bypass isn't enabled
- *(stamp)* companion bypass for Level 1 + Level 2 MOSFETs (ngspice CKTbypass)
- *(ac)* hoist omega-independent stamps into AcStampCache; bench complex solves
- *(sparse)* reuse symbolic LU across NR iterations and timesteps
- *(bsim3soi-dd)* refine RampVg2 diagnosis after chain audit
- *(soi-dd)* correct RampVg2 diagnosis — physics, not convergence
- add AC analysis test for coupled inductors

## [0.3.0](https://github.com/cramt/thevenin/compare/thevenin-v0.2.0...thevenin-v0.3.0) - 2026-04-26

### Added

- behavioral resistor R n+ n- r={expr} to B-source conversion
- implement Level 2 MOSFET (Grove-Frohman) model
- implement BSIM3SOI-DD CAPMOD=3 charge model
- add per-test abs_tol support, un-ignore CPL transmission line tests
- implement BSIM4 model binning and level=54 mapping
- BSIM3SOI-DD intrinsic 4-terminal charge integration for transient
- implement BSIM3SOI-DD body charge (capMod=2) and B-E transient cap
- implement ngspice order upgrade check for BE→Trap transition
- alter command, B-source tc1/tc2, complex vecexpr, pole() function
- implement V= behavioral voltage sources in MNA system
- per-test tolerance overrides, dynamic gmin stepping, agent strategy update
- *(control)* implement .control block interpreter
- *(output)* implement @device[param] queries for .print directives
- *(parser,output)* support non-parenthesized PULSE and arithmetic print expressions
- *(test-infra)* add triage script, vacuous pass detection, and structured test output
- *(newton)* add MODEINITJCT/MODEINITFLOAT convergence modes

### Fixed

- use sort_by_key for clippy stable compatibility
- add version fields to all path deps and enable publishing
- close three critical pipeline gaps (ic, uic, tline terminals)
- BSIM3SOI-DD CAPMOD=3 transient NR convergence
- resolve HFET DCFL inverter convergence — ngspice inverse flag bug confirmed
- complete reset_from_solution for all device types
- move VBIC FO from ignore to tolerance override (641 pass, 11 skip)
- restore tolerances.toml — 7 tests recovered from FAIL to PASS
- correct VBIC Re/Rs thermal reverse-coupling stamp directions
- correct PMOS InitJct formula + restore junction ceq type sign for SOI models
- fix harness triage: case-sensitive "Expected" now categorized as NEAR_MISS
- rewrite source_stepping as gillespie algorithm (no gmin reduction)
- full chain-rule impact ionization derivatives for BSIM3SOI-DD
- SOI body node init, body_gmin floor, new_gmin zero-start
- align gmin stepping with ngspice + un-ignore VBIC diffamp unit test
- correct HFET model type — level=5 is HFET1, not HFET2
- implement VBIC transient junction charge integration, un-ignore diffamp
- add new_gmin stepping, source stepping InitJct, and SINE parser
- adaptive numerical Jacobian step for B-source AC analysis
- rewrite HFET gate leakage to match ngspice HFET2 model
- VBIC pnjlim/InitJct to use IS_T-based vcrit matching ngspice; un-ignore CEamp
- add SOI device stamps to jct_initial_guess + source stepping direct jump
- elevate device-model gmin during NR gmin stepping
- HFET InitJct initialization and phib parsing to match ngspice
- un-ignore BSIM3SOI-DD t3 test with tolerance override
- *(bsim3soi-dd)* add minIsub convergence aid matching ngspice
- *(bsim3soi-dd)* add missing KCL-balancing SP column entries for Iii/GIDL
- *(bjt)* add geqcb BE charge cross-coupling from Vbc in transient
- propagate circuit .OPTIONS to transient and OP analysis
- *(bsim3soi)* remove incorrect type sign from junction/Iii/GIDL ceq stamps
- BSIM3SOI-PD BJT params, Ic collector current, and impact ionization model
- DC sweep MODEINITJCT bypass for voltage-source-pinned circuits
- revert VBIC MODEINITJCT change that broke PNP convergence
- correct DEVpnjlim, DEVfetlim, and VBIC junction limiting to match ngspice
- use total_cmp for NaN safety, error on unimplemented alter, fix stale comment
- fix benches
- *(bsim3soi-dd)* correct 19 wrong parameter defaults to match ngspice
- *(bsim3soi-dd)* add missing Vds-dependent junction Jacobian derivatives
- *(vbic)* add missing -Iciei term in Ith_Vbci AC thermal derivative
- *(output)* use radians for ph() output matching ngspice default
- *(vbic)* add missing AC self-heating thermal stamps and cross-term stamps
- *(bsim3soi)* correct DD vfbb sign and add dVbseff cross-derivatives
- *(output)* use SPICE-standard additive tolerance in harness comparison
- *(bsim3soi)* rewrite DD BJT current formulation matching ngspice
- *(bjt)* normalize diffusion charge by qb matching ngspice bjtload.c
- *(bsim3soi)* implement VBSA-dependent csieff/qsieff in DD model
- *(bsim3soi)* correct poly gate depletion coefficient in PD model (1e18→1e6)
- *(bsim3soi)* implement kb3/dvbd0/dvbd1 parameter binning for FD and DD
- *(bsim3soi)* scale body-node Gmin by 1e-6 to match ngspice
- *(dc)* reset prev_solution between outer sweep steps in nested DC sweep
- BSIM3SOI-FD Vgsteff chain-rule + DD impact ionization Vdseffii
- un-ignore 5 previously-failing unit tests that now pass
- un-ignore bugs-2 and test-noise-2 via vector indexing and flicker noise
- un-ignore ac-zero and if-elseif tests via two targeted fixes
- *(tran)* fix breakpoint check that caused timestep collapse in large circuits
- *(bjt)* correct junction depletion charge formula exponent
- *(tests)* un-ignore bsim3soipd/RampVg2 test that now passes
- *(bsim3soi)* correct junction current ceq sign convention in DD and PD models
- *(tests)* un-ignore 9 tests that now pass
- *(vbic)* add missing SCALE factor to thermal conductance and capacitance
- *(vbic)* correct Vre/Vrs sign convention in self-heating thermal stamps
- *(bsim3soi)* correct IGIDL/junction width and mobility derivatives for DD/FD/PD
- *(bsim3soi-dd)* correct junction current width from wdios/wdiod to weff
- *(vbic)* correct base charge (qb) formula for non-default QBM
- *(bsim3soi)* preserve Vdseff smooth derivatives on clamping
- *(bsim3soi)* correct impact ionization and FD floating-body gmin stamp
- *(harness)* enable slope-aware tolerance universally, un-ignore rc and sensitivity tests
- *(vbic)* add AC charge-thermal cross-coupling stamps for self-heating
- *(vbic)* add missing Irbp cross-derivatives for Vbep and Vbci
- *(waveform)* change PULSE PW default from 0 to TSTOP
- *(bsim3soi-fd)* add dVbseff/dVg and dVbseff/dVd derivative chain
- *(bsim3soi,vbic)* correct abulk formula ordering and add full thermal Jacobian
- *(vbic)* correct four model equation bugs found by ngspice comparison
- *(vbic)* use self-heating-adjusted model for external resistance stamps
- *(output)* support .plot-only circuits in formatter and filter
- *(vbic)* match ngspice Ith addition order and I*V formula
- *(bsim3soi-pd)* correct junction temperature scaling exp(1) bug
- *(bsim3soi)* correct rds0 wr exponent in DD/FD/PD variants
- *(bsim3soi-fd,cpl)* correct FD Abulk T9 parameter and CPL R_m clamping
- *(bsim3soi-fd)* correct Vbsdio to unconditionally use Vbs0eff
- *(txl)* correct h1 convolution accumulation in update_cnv_txl
- *(cpl)* correct polint Neville tableau path for 0-based indexing
- *(cpl)* use f64 for delayed time indices matching ngspice double types
- *(transient)* use divided-difference LTE matching ngspice CKTterr
- *(bjt)* respect OFF flag in MODEINITJCT initialization
- *(bsim3soi-fd)* correct csieff, litl, and add Abeff computation
- *(cpl)* correct convolution accumulation and timing order in CPL model
- *(bsim3soi)* correct temperature scaling and DeltVthtemp in DD/FD/PD

### Other

- cargo fmt
- session 142 — update ignore reasons for accuracy, confirm all 9 tests intractable
- session 137 re-triage confirms all 9 ignored tests intractable
- record session 131 findings — deep investigation of remaining 12 intractable tests
- sparse LU solver, partial pivoting, and parallel AC sweeps
- Add BSIM3SOI-DD 5-terminal E-node charge coupling for transient analysis
- clean up dead state and fragile patterns from fix-tests run
- Tighten 3 tolerance overrides after accumulated improvements
- update DD RampVg2 ignore reason and history after charge integration
- un-ignore bsim3soipd/inv2: combined derivative stamps fix KCL imbalance
- un-ignore bsim3soidd/inv2: tolerance override + cboxt correctness fix
- tighten tolerance overrides for 5 harness tests
- implement AC sensitivity analysis, un-ignore sens-ac-1/2
- clarify sens-ac ignore reasons (AC sensitivity not implemented, not .control)
- un-ignore ltra2_2_line via tolerance override (rel_tol=1e-2)
- un-ignore 2 SOI unit tests that now pass after accumulated solver fixes
- un-ignore bsim3soifd/inv2.cir — now passes after accumulated solver fixes
- fix 24 broken intra-doc links causing cargo doc warnings
- fork netlists at analysis boundaries — one Netlist per analysis
- ergonomic public API with VectorData enum, Index impls, and simulate() dispatcher
- update cpl_ibm2 ignore reason with sign reversal finding
- cargo fmt
- update DD t3 ignore reason with analytical body voltage finding
- add gii_e, restructure body stamps to match ngspice
- format
- fix junction temp scaling, litl constant, and add Gme
- match ngspice junction exponential threshold (100→30)
- Merge pull request #14 from cramt/dependabot/cargo/rust-deps-5929f93d27
- format
- Fix harness tests: un-ignore diffpair/fourbitadder, fix VBIC AC thermal stamps, add BSIM3SOI-DD Gmc chain
- Fix BSIM3SOI-PD missing reverse bias recombination current (T11 term)
- correct VBIC error analysis and document session 81 findings
- update test status with session 80 exhaustive VBIC verification
- a
- *(ignore)* update ignore.toml error descriptions after slope tolerance fix
- document VBIC kernel parameter audit and test re-verification
- document BJT analytical charge investigation and VBIC CEamp improvement
- Merge pull request #6 from cramt/dependabot/cargo/rust-deps-bd726cdb98

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
