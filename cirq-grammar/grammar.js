/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

/**
 * Tree-sitter grammar for the Cirq circuit description language.
 *
 * This grammar defines the concrete syntax tree (CST) for Cirq.
 * The CST is then lowered to an AST in the cirq-ast Rust crate.
 */

module.exports = grammar({
  name: "cirq",

  extras: ($) => [/\s/, /;/, $.line_comment, $.block_comment],

  word: ($) => $.identifier,

  rules: {
    source_file: ($) => repeat($._top_level),

    _top_level: ($) =>
      choice($.circuit_decl, $.module_decl, $.import_decl, $.export_decl, $.model_decl, $.func_decl),

    // ── Circuit ──────────────────────────────────────────────────────

    circuit_decl: ($) =>
      seq(
        repeat($.attribute),
        "circuit",
        field("name", $.identifier),
        "{",
        repeat($._circuit_item),
        "}"
      ),

    _circuit_item: ($) =>
      choice(
        $.param_decl,
        $.let_decl,
        $.element_inst,
        $.module_inst,
        $.module_decl,
        $.model_decl,
        $.analysis_decl,
        $.global_decl,
        $.options_decl,
        $.temp_decl,
        $.save_decl,
        $.func_decl,
        $.ic_decl,
        $.coupled_line_decl,
        $.code_decl,
        $.measure_decl
      ),

    // ── Module ───────────────────────────────────────────────────────

    module_decl: ($) =>
      seq(
        repeat($.attribute),
        "module",
        field("name", $.identifier),
        optional(seq(":", field("base", $.identifier))),
        "{",
        repeat(choice($.port_decl, $._circuit_item)),
        "}"
      ),

    port_decl: ($) =>
      seq(
        repeat($.attribute),
        "port",
        field("name", $.identifier),
        ":",
        field("direction", $.port_direction)
      ),

    port_direction: (_$) => choice("in", "out", "inout"),

    // ── Elements ─────────────────────────────────────────────────────

    element_inst: ($) =>
      seq(
        repeat($.attribute),
        field("name", $.identifier),
        ":",
        field("type", $.identifier),
        "(",
        optional($.argument_list),
        ")"
      ),

    module_inst: ($) =>
      seq(
        repeat($.attribute),
        field("name", $.identifier),
        ":",
        field("module", $.qualified_name),
        "(",
        optional($.argument_list),
        ")"
      ),

    argument_list: ($) => seq($.argument, repeat(seq(",", $.argument)), optional(",")),

    argument: ($) =>
      choice(
        $.named_argument,
        $.connection,
        $.named_connection,
        $._expression
      ),

    named_argument: ($) =>
      seq(field("name", $.identifier), ":", field("value", $._expression)),

    connection: ($) =>
      seq(field("from", $._net_ref), "->", field("to", $._net_ref)),

    named_connection: ($) =>
      seq(
        field("name", $.identifier),
        ":",
        field("from", $._net_ref),
        "->",
        field("to", $._net_ref)
      ),

    _net_ref: ($) => choice($.identifier, $.gnd),

    // ── Parameters ───────────────────────────────────────────────────

    param_decl: ($) =>
      seq(
        repeat($.attribute),
        "param",
        field("name", $.identifier),
        optional(seq(":", field("type", $.identifier))),
        optional(seq("=", field("value", $._expression)))
      ),

    let_decl: ($) =>
      seq(
        repeat($.attribute),
        "let",
        field("name", $.identifier),
        "=",
        field("value", $._expression)
      ),

    global_decl: ($) => seq("global", field("name", $.identifier)),

    // ── User-defined functions ──────────────────────────────────────

    func_decl: ($) =>
      seq(
        field("name", $.identifier),
        "(",
        optional($.func_params),
        ")",
        "=",
        field("body", $._expression)
      ),

    func_params: ($) =>
      seq($.identifier, repeat(seq(",", $.identifier)), optional(",")),

    // ── Initial conditions ──────────────────────────────────────────

    ic_decl: ($) =>
      seq(
        "ic",
        "{",
        repeat($.ic_entry),
        "}"
      ),

    ic_entry: ($) =>
      seq(
        "v",
        "(",
        field("node", $.identifier),
        ")",
        "=",
        field("value", $._expression)
      ),

    // ── Coupled transmission lines ─────────────────────────────────

    coupled_line_decl: ($) =>
      seq(
        "coupled_line",
        field("name", $.identifier),
        "{",
        repeat($.coupled_line_field),
        "}"
      ),

    coupled_line_field: ($) =>
      seq(
        field("key", $.identifier),
        ":",
        field("value", $._expression)
      ),

    // ── Code block (verbatim embedded language) ──────────────────────

    code_decl: ($) =>
      seq("code", field("language", $.string_literal), "{", optional(field("body", $.code_body)), "}"),

    // Raw content between { and } — a single atomic token so that
    // extras (whitespace, semicolons, comments) don't interfere with
    // the embedded language lines inside.
    code_body: (_$) => token(prec(-1, /[^}]+/)),

    // ── Measure block ───────────────────────────────────────────────

    measure_decl: ($) =>
      seq(
        "measure",
        field("analysis_kind", $.identifier),
        field("name", $.string_literal),
        "{",
        repeat($.measure_field),
        "}"
      ),

    measure_field: ($) =>
      seq(
        field("key", $.identifier),
        ":",
        field("value", $.string_literal)
      ),

    // ── Options and Temperature ─────────────────────────────────────

    options_decl: ($) =>
      seq(
        "options",
        "{",
        repeat($.options_setting),
        "}"
      ),

    options_setting: ($) =>
      seq(field("name", $.identifier), ":", field("value", $._expression)),

    temp_decl: ($) =>
      seq("temp", field("value", $._expression)),

    save_decl: ($) =>
      seq(
        "save",
        "{",
        repeat($.save_target),
        "}"
      ),

    save_target: ($) =>
      choice(
        // v(node) or v(node1, node2) — voltage probe
        seq(
          field("type", "v"),
          "(",
          field("node", $.identifier),
          optional(seq(",", field("node2", $.identifier))),
          ")"
        ),
        // i(element) — current probe
        seq(
          field("type", "i"),
          "(",
          field("element", $.identifier),
          ")"
        ),
        // Bare identifier — raw save target
        field("name", $.identifier)
      ),

    // ── Models ──────────────────────────────────────────────────────

    model_decl: ($) =>
      seq(
        repeat($.attribute),
        "model",
        field("name", $.identifier),
        ":",
        field("device_type", $.identifier),
        "{",
        repeat($.model_param),
        "}"
      ),

    model_param: ($) =>
      seq(field("name", $.identifier), "=", field("value", $._expression)),

    // ── Analysis ─────────────────────────────────────────────────────

    analysis_decl: ($) =>
      seq(
        "analysis",
        field("kind", $.identifier),
        "{",
        repeat($._analysis_item),
        "}"
      ),

    _analysis_item: ($) => choice($.analysis_setting, $.sweep_spec),

    analysis_setting: ($) =>
      seq(field("name", $.identifier), ":", field("value", $._expression)),

    sweep_spec: ($) =>
      seq(
        "sweep",
        field("source", $.identifier),
        ":",
        field("start", $._expression),
        "..",
        field("stop", $._expression),
        "step",
        field("step", $._expression)
      ),

    // ── Import ───────────────────────────────────────────────────────

    import_decl: ($) =>
      choice(
        // Named import: import { name1, name2 } from "path"
        seq(
          "import",
          "{",
          field("names", $.import_names),
          "}",
          "from",
          field("path", $.string_literal)
        ),
        // Plain or aliased import: import "path" [as alias]
        seq(
          "import",
          field("path", $.string_literal),
          optional(seq("as", field("alias", $.identifier)))
        )
      ),

    import_names: ($) =>
      seq($.identifier, repeat(seq(",", $.identifier)), optional(",")),

    // ── Export ───────────────────────────────────────────────────────

    export_decl: ($) =>
      seq(
        "export",
        field("name", $.identifier),
        "{",
        repeat($._export_item),
        "}"
      ),

    _export_item: ($) =>
      choice($.model_decl, $.module_decl, $.func_decl, $.param_decl),

    // ── Attributes ───────────────────────────────────────────────────

    attribute: ($) =>
      seq(
        "@",
        field("name", $.identifier),
        optional(seq("(", optional($.argument_list), ")"))
      ),

    // ── Expressions ──────────────────────────────────────────────────

    _expression: ($) =>
      choice(
        $.binary_expression,
        $.unary_expression,
        $.call_expression,
        $.paren_expression,
        $.number_literal,
        $.string_literal,
        $.boolean_literal,
        $.identifier,
        $.qualified_name,
        $.list_literal,
        $.block_literal,
        $.gnd
      ),

    binary_expression: ($) =>
      choice(
        // Right-associative exponentiation
        prec.right(
          7,
          seq(
            field("left", $._expression),
            field("operator", "**"),
            field("right", $._expression)
          )
        ),
        // Left-associative operators
        ...[
          ["||", 1],
          ["&&", 2],
          ["==", 3],
          ["!=", 3],
          ["<", 4],
          [">", 4],
          ["<=", 4],
          [">=", 4],
          ["+", 5],
          ["-", 5],
          ["*", 6],
          ["/", 6],
          ["%", 6],
        ].map(([op, p]) =>
          prec.left(
            /** @type {number} */ (p),
            seq(
              field("left", $._expression),
              field("operator", /** @type {string} */ (op)),
              field("right", $._expression)
            )
          )
        )
      ),

    unary_expression: ($) =>
      prec(
        8,
        seq(
          field("operator", choice("-", "!")),
          field("operand", $._expression)
        )
      ),

    call_expression: ($) =>
      prec(
        9,
        seq(
          field("function", $.identifier),
          "(",
          optional(
            seq($._expression, repeat(seq(",", $._expression)), optional(","))
          ),
          ")"
        )
      ),

    paren_expression: ($) =>
      seq("(", $._expression, repeat(seq(",", $._expression)), optional(","), ")"),

    list_literal: ($) =>
      seq("[", optional(seq($._expression, repeat(seq(",", $._expression)), optional(","))), "]"),

    block_literal: ($) =>
      seq(
        "{",
        optional(seq(
          $.block_entry,
          repeat(seq(",", $.block_entry)),
          optional(",")
        )),
        "}"
      ),

    block_entry: ($) =>
      seq(field("key", $.identifier), ":", field("value", $._expression)),

    // ── Literals ─────────────────────────────────────────────────────

    number_literal: (_$) =>
      token(
        seq(
          choice(
            // Hex
            seq("0", choice("x", "X"), /[0-9a-fA-F][0-9a-fA-F_]*/),
            // Binary
            seq("0", choice("b", "B"), /[01][01_]*/),
            // Decimal float/int
            seq(
              choice(
                seq(/[0-9][0-9_]*/, optional(seq(".", /[0-9][0-9_]*/))),
                seq(".", /[0-9][0-9_]*/)
              ),
              optional(seq(choice("e", "E"), optional(choice("+", "-")), /[0-9][0-9_]*/))
            )
          ),
          // Optional SI suffix
          optional(choice("T", "G", "Meg", "M", "k", "m", "u", "n", "p", "f"))
        )
      ),

    string_literal: (_$) =>
      token(seq('"', repeat(choice(/[^"\\]/, seq("\\", /./),)), '"')),

    boolean_literal: (_$) => choice("true", "false"),

    gnd: (_$) => "gnd",

    // ── Names ────────────────────────────────────────────────────────

    identifier: (_$) => /[a-zA-Z_][a-zA-Z0-9_]*/,

    qualified_name: ($) =>
      seq($.identifier, repeat1(seq(".", $.identifier))),

    // ── Comments ─────────────────────────────────────────────────────

    line_comment: (_$) => token(seq("//", /[^\n]*/)),

    block_comment: (_$) => token(seq("/*", /[^*]*\*+([^/*][^*]*\*+)*/, "/")),
  },
});
