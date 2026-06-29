//! Post-resolution device-model parameters.
//!
//! Every device-model loader (`DiodeModel::from_params`, `BjtModel::from_params`,
//! …) consumes parameters *after* SPICE expression resolution, when every value
//! is a plain number — historically each loader guarded its match arms with
//! `if let Expr::Num(v) = &p.value`, silently dropping anything still symbolic.
//!
//! [`ModelParams`] is that resolved shape: the model `kind`, its `name`, and the
//! numeric `name = value` pairs. It replaces [`thevenin_types::ModelDef`] (whose
//! values carry the SPICE [`Expr`] AST) at the device boundary so the
//! simulator's device layer no longer depends on `Expr`. `Expr` survives only at
//! the import/export edge (`cirq-spice-import`, `cirq-frontend::to_netlist`) and
//! in the legacy Netlist stamping path, both of which feed `ModelParams` through
//! the adapters below.

use thevenin_types::{Expr, ModelDef, Param};

/// Resolved numeric parameters for a device `.model` card.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelParams {
    /// Model name (e.g. `qnpn`).
    pub name: String,
    /// Model kind token (e.g. `NPN`, `NMOS`, `D`). Loaders that dispatch on
    /// kind compare case-insensitively.
    pub kind: String,
    /// Numeric `name = value` parameters, in declaration order. Names keep
    /// their original casing; loaders match case-insensitively.
    pub params: Vec<(String, f64)>,
}

impl ModelParams {
    /// Project a SPICE [`ModelDef`] to its resolved numeric form, dropping any
    /// parameter whose value is still an unresolved `Expr::Param` /
    /// `Expr::Brace` (matching every loader's historical `if let Expr::Num`
    /// guard).
    pub fn from_model_def(def: &ModelDef) -> Self {
        Self {
            name: def.name.clone(),
            kind: def.kind.clone(),
            params: resolved_params(&def.params),
        }
    }

    /// Project a Cirq IR [`cirq_ir::Model`] directly to resolved numeric
    /// params — the native Circuit-path counterpart to [`Self::from_model_def`]
    /// that skips the `Expr`-shaped `ModelDef` round-trip
    /// (`cirq_frontend::to_netlist::convert_model`). Numeric equivalence with
    /// that path is exact: this mirrors `value_to_expr` + the `Expr::Num`
    /// filter — `Real`/`Integer`/`Bool` become numbers; `String` (brace/param)
    /// values are dropped. The kind string comes from the same
    /// [`cirq_ir::DeviceType::spice_kind`] source of truth `convert_model` uses.
    pub fn from_ir(model: &cirq_ir::Model) -> Self {
        let params = model
            .params
            .iter()
            .filter_map(|(name, value)| ir_value_to_f64(value).map(|v| (name.clone(), v)))
            .collect();
        Self {
            name: model.name.clone(),
            kind: model.device_type.spice_kind(),
            params,
        }
    }

    /// Look up a single parameter by name, case-insensitively.
    pub fn get(&self, name: &str) -> Option<f64> {
        self.params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    }
}

/// Numeric projection of a single IR [`cirq_ir::Value`], matching
/// `cirq_frontend::to_netlist::value_to_expr` followed by the device loaders'
/// `Expr::Num` filter: `Real`/`Integer`/`Bool` yield a number, `String`
/// (brace/param) is dropped, and any future non-exhaustive variant folds to
/// `0.0` (the `value_to_expr` fallback) so the two paths never diverge.
fn ir_value_to_f64(value: &cirq_ir::Value) -> Option<f64> {
    match value {
        cirq_ir::Value::Real(f) => Some(*f),
        cirq_ir::Value::Integer(i) => Some(*i as f64),
        cirq_ir::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        cirq_ir::Value::String(_) => None,
        _ => Some(0.0),
    }
}

/// Project SPICE `Param`s (`name = Expr`) to resolved `(name, f64)` pairs,
/// dropping non-numeric (unresolved) values. Shared by the model-card and
/// element-instance parameter paths so both sides of the device boundary speak
/// the same resolved shape.
pub fn resolved_params(params: &[Param]) -> Vec<(String, f64)> {
    params
        .iter()
        .filter_map(|p| match &p.value {
            Expr::Num(v) => Some((p.name.clone(), *v)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_model_def_keeps_numeric_drops_symbolic() {
        let def = ModelDef {
            name: "d1".into(),
            kind: "D".into(),
            params: vec![
                Param {
                    name: "IS".into(),
                    value: Expr::Num(1e-14),
                },
                Param {
                    name: "RS".into(),
                    value: Expr::Param("rseries".into()),
                },
            ],
        };
        let mp = ModelParams::from_model_def(&def);
        assert_eq!(mp.name, "d1");
        assert_eq!(mp.kind, "D");
        assert_eq!(mp.params, vec![("IS".to_string(), 1e-14)]);
        assert_eq!(mp.get("is"), Some(1e-14));
        assert_eq!(mp.get("RS"), None);
    }

    #[test]
    fn from_ir_matches_convert_model_round_trip() {
        // `from_ir` must be numerically identical to the legacy
        // `from_model_def(&convert_model(..))` path it replaces.
        let model = cirq_ir::Model {
            id: cirq_ir::Id(7),
            name: "qnpn".to_string(),
            device_type: cirq_ir::DeviceType::Npn,
            params: vec![
                ("BF".to_string(), cirq_ir::Value::Real(120.0)),
                ("IS".to_string(), cirq_ir::Value::Integer(2)),
                // String (brace/param) values are dropped, matching the
                // device loaders' `Expr::Num` filter.
                (
                    "RB".to_string(),
                    cirq_ir::Value::String("{rb0}".to_string()),
                ),
            ],
        };
        let native = ModelParams::from_ir(&model);
        let via_netlist =
            ModelParams::from_model_def(&cirq_frontend::to_netlist::convert_model(&model));
        assert_eq!(native, via_netlist);
        assert_eq!(native.kind, "NPN");
        assert_eq!(native.get("BF"), Some(120.0));
        assert_eq!(native.get("IS"), Some(2.0));
        assert_eq!(native.get("RB"), None);
    }

    #[test]
    fn resolved_params_filters_non_numeric() {
        let params = vec![
            Param {
                name: "AREA".into(),
                value: Expr::Num(2.0),
            },
            Param {
                name: "M".into(),
                value: Expr::Brace("n*2".into()),
            },
        ];
        assert_eq!(resolved_params(&params), vec![("AREA".to_string(), 2.0)]);
    }
}
