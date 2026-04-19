; Cirq tree-sitter tags
; ============================================================
; Tags for symbol navigation (go-to-definition, symbol outline).

(circuit_decl
  name: (identifier) @name) @definition.class

(module_decl
  name: (identifier) @name) @definition.class

(model_decl
  name: (identifier) @name) @definition.class

(analysis_decl
  kind: (identifier) @name) @definition.method

(element_inst
  name: (identifier) @name) @definition.var

(module_inst
  name: (identifier) @name) @definition.var

(param_decl
  name: (identifier) @name) @definition.var

(let_decl
  name: (identifier) @name) @definition.var

(port_decl
  name: (identifier) @name) @definition.var
