//! Pure-math expansion plan for URC (uniform distributed RC) lines.
//!
//! A URC element is a **macro**: it does not exist as a device in the
//! simulator. Both the SPICE importer (for the `U` element + `.model URC`) and
//! the Cirq frontend (for the native `urc` element) expand it at compile time
//! into a ladder of R / C — or R / C / D when the model has `ISPERL > 0` — that
//! mirrors ngspice's `urcsetup.c`.
//!
//! This module is the single source of truth for that ladder. [`plan`] computes
//! a typed, *node-relative* description ([`UrcPlan`]); each caller materialises
//! it into its own element representation by mapping the abstract
//! [`UrcNode`]s onto concrete nets and naming the elements in its own scheme.
//! Keeping the arithmetic here means the importer and the frontend can never
//! drift apart.

use std::f64::consts::PI;

/// Per-unit-length URC model parameters, with ngspice's defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UrcParams {
    /// Geometric propagation constant for the lump progression.
    pub k: f64,
    /// Maximum frequency of interest (Hz), used to auto-size the lump count.
    pub fmax: f64,
    /// Resistance per unit length (Ω/m).
    pub rperl: f64,
    /// Capacitance per unit length (F/m).
    pub cperl: f64,
    /// Diode saturation current per unit length (A/m). When `> 0` the shunts
    /// become diodes instead of capacitors.
    pub isperl: f64,
    /// Diode series resistance per unit length (Ω/m).
    pub rsperl: f64,
}

impl Default for UrcParams {
    fn default() -> Self {
        Self {
            k: 1.5,
            fmax: 1.0e9,
            rperl: 1000.0,
            cperl: 1.0e-12,
            isperl: 0.0,
            rsperl: 0.0,
        }
    }
}

/// An abstract terminal in a [`UrcPlan`]. The caller maps `Pos`/`Neg`/`Gnd` to
/// the URC element's three nets and `Internal` to a synthesised internal net
/// (the string is a stable per-lump suffix such as `"lo1"` / `"hi3"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrcNode {
    Pos,
    Neg,
    Gnd,
    Internal(String),
}

/// A shunt element from a midpoint node to ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UrcShunt {
    /// A capacitor of the given value (F).
    Cap(f64),
    /// A diode to the plan's synthesised [`UrcDiodeModel`].
    Diode,
}

/// A series resistor in one of the two ladder chains.
#[derive(Debug, Clone, PartialEq)]
pub struct UrcResistor {
    /// Stable name suffix (e.g. `"rlo1"`), unique within a single URC.
    pub suffix: String,
    pub from: UrcNode,
    pub to: UrcNode,
    /// Resistance (Ω).
    pub value: f64,
}

/// A shunt-to-ground element hanging off a midpoint node.
#[derive(Debug, Clone, PartialEq)]
pub struct UrcShuntElem {
    /// Stable name suffix (e.g. `"clo1"` / `"dlo1"`).
    pub suffix: String,
    pub node: UrcNode,
    pub shunt: UrcShunt,
}

/// Parameters of the diode model synthesised when `ISPERL > 0`. Matches the
/// `.model … D (IS=… CJO=… RS=…)` the importer emits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UrcDiodeModel {
    pub is: f64,
    pub cjo: f64,
    pub rs: f64,
}

/// A fully-resolved URC ladder, ready to materialise into concrete elements.
#[derive(Debug, Clone, PartialEq)]
pub struct UrcPlan {
    /// Number of lumps (stages).
    pub lumps: usize,
    pub resistors: Vec<UrcResistor>,
    pub shunts: Vec<UrcShuntElem>,
    /// Present iff `ISPERL > 0` — the model the diode shunts reference.
    pub diode_model: Option<UrcDiodeModel>,
}

/// Number of lumps for a URC, matching ngspice `urcsetup.c`. An explicit
/// `user_lumps >= 1` wins; otherwise it is derived from `FMAX`, `K`, and the
/// total RC, with a floor of 3.
pub fn lump_count(p: &UrcParams, length: f64, user_lumps: Option<f64>) -> usize {
    match user_lumps {
        Some(n) if n >= 1.0 => n as usize,
        _ => {
            let r0 = length * p.rperl;
            let c0 = length * p.cperl;
            let wnorm = p.fmax * r0 * c0 * 2.0 * PI;
            if wnorm < 35.0 {
                3
            } else {
                let n = (wnorm * ((p.k - 1.0) / p.k).powi(2)).ln() / p.k.ln();
                (n.ceil() as usize).max(3)
            }
        }
    }
}

/// Build the R / C(/ D) ladder for one URC element.
///
/// The topology mirrors `urcsetup.c`: two resistor chains run inward from the
/// `pos` and `neg` terminals, meeting at a middle node; each non-final stage
/// drops a shunt (cap or diode) from both its `lo` and `hi` midpoints, and the
/// final stage's two paths collapse onto a single node with one shunt.
pub fn plan(p: &UrcParams, length: f64, user_lumps: Option<f64>) -> UrcPlan {
    let lumps = lump_count(p, length, user_lumps);
    let n_f = lumps as f64;
    let k = p.k;

    let r0 = length * p.rperl;
    let c0 = length * p.cperl;
    let i0 = length * p.isperl;

    // Per-stage values, from urcsetup.c lines 90-93.
    let r1 = (r0 * (k - 1.0)) / (2.0 * k.powf(n_f) - 2.0);
    let c1 = (c0 * (k - 1.0)) / (k.powf(n_f - 1.0) * (k + 1.0) - 2.0);
    let i1 = (i0 * (k - 1.0)) / (k.powf(n_f - 1.0) * (k + 1.0) - 2.0);
    let rd = length * n_f * p.rsperl;

    let has_diodes = p.isperl > 0.0;
    let shunt_kind = if has_diodes {
        UrcShunt::Diode
    } else {
        UrcShunt::Cap(c1)
    };

    let mut resistors = Vec::with_capacity(2 * lumps);
    let mut shunts = Vec::with_capacity(2 * lumps);

    let mut lowl = UrcNode::Pos;
    let mut hir = UrcNode::Neg;
    for i in 1..=lumps {
        let lo_i = UrcNode::Internal(format!("lo{i}"));
        let hi_i = UrcNode::Internal(format!("hi{i}"));
        // At the final lump the two paths meet — both collapse onto `hi{i}`.
        let (lowr_node, hil_node) = if i == lumps {
            (hi_i.clone(), hi_i.clone())
        } else {
            (lo_i.clone(), hi_i.clone())
        };

        resistors.push(UrcResistor {
            suffix: format!("rlo{i}"),
            from: lowl.clone(),
            to: lowr_node.clone(),
            value: r1,
        });
        resistors.push(UrcResistor {
            suffix: format!("rhi{i}"),
            from: hil_node.clone(),
            to: hir.clone(),
            value: r1,
        });

        shunts.push(UrcShuntElem {
            suffix: if has_diodes {
                format!("dlo{i}")
            } else {
                format!("clo{i}")
            },
            node: lowr_node,
            shunt: shunt_kind,
        });
        if i != lumps {
            shunts.push(UrcShuntElem {
                suffix: if has_diodes {
                    format!("dhi{i}")
                } else {
                    format!("chi{i}")
                },
                node: hil_node,
                shunt: shunt_kind,
            });
        }

        lowl = lo_i;
        hir = hi_i;
    }

    let diode_model = has_diodes.then_some(UrcDiodeModel {
        is: i1,
        cjo: c1,
        rs: rd,
    });

    UrcPlan {
        lumps,
        resistors,
        shunts,
        diode_model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Below the `wnorm < 35` threshold the lump count floors at 3.
    #[test]
    fn small_rc_defaults_to_three_lumps() {
        let p = UrcParams::default();
        // length 1: r0=1k, c0=1p → wnorm = 1e9*1e3*1e-12*2π ≈ 6.28 < 35.
        assert_eq!(lump_count(&p, 1.0, None), 3);
    }

    /// An explicit lump count wins over the auto-sizing.
    #[test]
    fn explicit_lumps_override() {
        let p = UrcParams::default();
        assert_eq!(lump_count(&p, 1.0, Some(16.0)), 16);
        assert_eq!(lump_count(&p, 1e6, Some(8.0)), 8);
    }

    /// Large RC drives the lump count above the floor via the FMAX formula.
    #[test]
    fn large_rc_grows_lump_count() {
        let p = UrcParams::default();
        let n = lump_count(&p, 1e4, None); // r0=1e7, c0=1e-8 → wnorm huge
        assert!(n > 3, "expected auto-sized lump count > 3, got {n}");
    }

    /// The per-stage R/C values match the urcsetup.c closed forms, and the
    /// ladder has the expected element counts (2 resistors per lump; one shunt
    /// per midpoint with the final lump sharing a node).
    #[test]
    fn ladder_shape_and_values() {
        let p = UrcParams::default();
        let pl = plan(&p, 1.0, Some(4.0));
        assert_eq!(pl.lumps, 4);
        assert_eq!(pl.resistors.len(), 8); // 2 per lump
        // Shunts: lo on every lump (4) + hi on non-final lumps (3) = 7.
        assert_eq!(pl.shunts.len(), 7);
        assert!(pl.diode_model.is_none());

        let n_f = 4.0_f64;
        let k = 1.5_f64;
        let r0 = 1.0 * 1000.0;
        let c0 = 1.0 * 1.0e-12;
        let r1 = (r0 * (k - 1.0)) / (2.0 * k.powf(n_f) - 2.0);
        let c1 = (c0 * (k - 1.0)) / (k.powf(n_f - 1.0) * (k + 1.0) - 2.0);
        assert!((pl.resistors[0].value - r1).abs() < 1e-12 * r1.abs().max(1.0));
        match pl.shunts[0].shunt {
            UrcShunt::Cap(c) => assert!((c - c1).abs() < 1e-24),
            UrcShunt::Diode => panic!("expected cap shunt"),
        }
    }

    /// The first/last resistors anchor to the Pos/Neg terminals; the two chains
    /// meet at the final `hi{lumps}` node.
    #[test]
    fn endpoints_anchor_to_terminals() {
        let pl = plan(&UrcParams::default(), 1.0, Some(3.0));
        assert_eq!(pl.resistors[0].from, UrcNode::Pos);
        // rhi1 returns to Neg.
        let rhi1 = pl.resistors.iter().find(|r| r.suffix == "rhi1").unwrap();
        assert_eq!(rhi1.to, UrcNode::Neg);
        // The final lump's two resistors share the hi{lumps} node.
        let rlo3 = pl.resistors.iter().find(|r| r.suffix == "rlo3").unwrap();
        let rhi3 = pl.resistors.iter().find(|r| r.suffix == "rhi3").unwrap();
        assert_eq!(rlo3.to, UrcNode::Internal("hi3".into()));
        assert_eq!(rhi3.from, UrcNode::Internal("hi3".into()));
    }

    /// `ISPERL > 0` turns shunts into diodes and synthesises a diode model with
    /// IS/CJO/RS matching the importer's `.model … D (...)`.
    #[test]
    fn isperl_produces_diode_model() {
        let p = UrcParams {
            isperl: 1.0e-9,
            rsperl: 10.0,
            ..UrcParams::default()
        };
        let pl = plan(&p, 1.0, Some(3.0));
        let dm = pl.diode_model.expect("diode model present");
        assert!(matches!(pl.shunts[0].shunt, UrcShunt::Diode));
        // Recompute the closed forms.
        let n_f = 3.0_f64;
        let k = 1.5_f64;
        let i1 = ((1.0 * 1.0e-9) * (k - 1.0)) / (k.powf(n_f - 1.0) * (k + 1.0) - 2.0);
        let rd = 1.0 * n_f * 10.0;
        assert!((dm.is - i1).abs() < 1e-24);
        assert!((dm.rs - rd).abs() < 1e-12);
    }
}
