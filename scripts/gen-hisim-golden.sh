#!/usr/bin/env bash
# Regenerate the HiSIM2 golden-reference CSVs from ngspice.
#
# ngspice is invoked via nixpkgs (ngspice-45, compiled with HiSIM2). The model
# card is thevenin/tests/fixtures/hisim2/nch.model; outputs are written next to
# it. thevenin/tests/hisim_golden.rs diffs the simulator against these files.
#
# Usage:  ./scripts/gen-hisim-golden.sh
set -euo pipefail
cd "$(dirname "$0")/../thevenin/tests/fixtures/hisim2"

run() { nix run nixpkgs#ngspice -- -b "$1"; }

# ── Id-Vds family: Vg = 0.6 .. 1.2 step 0.2, Vds 0 .. 1.2 step 0.05 ──────────
cat > _idvds.cir <<'DECK'
HiSIM2 NMOS Id-Vds family
.include nch.model
Vd d 0 0
Vg g 0 0
Vs s 0 0
Vb b 0 0
M1 d g s b nch w=10u l=1u
.dc Vd 0 1.2 0.05 Vg 0.6 1.2 0.2
.control
run
wrdata idvds.csv -i(Vd)
.endc
.end
DECK
run _idvds.cir

# ── Id-Vgs transfer: Vds = 0.05 (lin) and 1.2 (sat); Vgs 0 .. 1.2 step 0.025 ─
cat > _idvgs.cir <<'DECK'
HiSIM2 NMOS Id-Vgs transfer
.include nch.model
Vd d 0 1.2
Vg g 0 0
Vs s 0 0
Vb b 0 0
M1 d g s b nch w=10u l=1u
.dc Vg 0 1.2 0.025 Vd 0.05 1.2 1.15
.control
run
wrdata idvgs.csv -i(Vd)
.endc
.end
DECK
run _idvgs.cir

# ── Body effect: Id-Vgs at Vbs = 0, -0.5, -1.0; Vds = 1.2 ────────────────────
cat > _idvbs.cir <<'DECK'
HiSIM2 NMOS body effect
.include nch.model
Vd d 0 1.2
Vg g 0 0
Vs s 0 0
Vb b 0 0
M1 d g s b nch w=10u l=1u
.dc Vg 0 1.2 0.05 Vb 0 -1.0 -0.5
.control
run
wrdata idvbs.csv -i(Vd)
.endc
.end
DECK
run _idvbs.cir

rm -f _idvds.cir _idvgs.cir _idvbs.cir
echo "HiSIM2 golden CSVs regenerated."
