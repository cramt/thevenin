// AST enrichment for the cirq grammar, in snark-dsl's `ast()` DSL.
//
// Structure (which fields a node has, their types, their cardinality) is DERIVED
// from the grammar. This file only supplies what the grammar can't express:
// enum names for the hidden `choice` rules (underscore-prefixed), so the codegen
// can name the Rust enums instead of inventing something.
ast({
  _top_level: { enum: "TopLevel" },
  _circuit_item: { enum: "CircuitItem" },
  _analysis_item: { enum: "AnalysisItem" },
  _export_item: { enum: "ExportItem" },
  _argument: { enum: "Argument" },
  _expression: { enum: "Expr" },
  _net_ref: { enum: "NetRef" },

  // Fields that bind different node kinds across a rule's alternatives need an
  // explicit enum (the grammar can't express the union). measure_decl's `name`
  // is `identifier` (inline form) or `string_literal` (block form).
  measure_decl: { fields: { name: { enum: "MeasureName" } } },
});
