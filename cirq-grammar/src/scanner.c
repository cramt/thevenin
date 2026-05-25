#include "tree_sitter/parser.h"

// External scanner for the cirq grammar.
//
// Currently scans a single token type: `code_body`, the raw verbatim text
// between the braces of `code "lang" { ... }`.
//
// The body may contain arbitrary embedded-language code, including nested
// `{` / `}` and string literals that themselves contain braces. A naive
// regex that consumes "any character except `}`" terminates at the first
// closing brace and corrupts anything more interesting than the simplest
// snippet. This scanner walks the body character-by-character, tracking
// brace depth and skipping over string literals so that the body extends
// exactly to the matching `}` of the `code` block.
//
// ─────────────────────────────────────────────────────────────────────
// KNOWN LIMITATIONS
// ─────────────────────────────────────────────────────────────────────
//
// The brace counter understands only the lowest-common-denominator
// lexical features shared across most embedded languages. The cases
// listed below are NOT handled, and a `}` appearing inside one of them
// will prematurely terminate the `code` block.
//
// 1. Line comments. `// }` (C/JS/Rust/...), `# }` (Python/shell/...),
//    `-- }` (SQL/Lua/Haskell), `; }` (Lisp/asm). The scanner treats
//    every brace as code.
//
// 2. Block comments. `/* } */`, `(* } *)`, `<!-- } -->`, `#| } |#`.
//    Same problem.
//
// 3. Multiline string forms beyond simple `"..."` / `'...'`:
//      - Python triple-quoted strings: `"""..."""`, `'''...'''`
//      - JS template literals: `` `...${ x }...` `` (also has its own
//        `${...}` interpolation that nests arbitrarily)
//      - Rust raw strings: `r"..."`, `r#"..."#`, `r##"..."##`
//      - C++ raw strings: `R"delim(...)delim"`
//      - Shell here-docs: `<<EOF ... EOF`
//      - Lua long brackets: `[[...]]`, `[==[...]==]`
//
// 4. Regex literals (`/.../` in JS) can contain braces in character
//    classes (`/[}]/`). The scanner does not distinguish division from
//    regex.
//
// 5. Character literals in languages where `'}'` is one character
//    rather than a string. The single-quote handling treats it as a
//    string starting at `'`, which works for `'}'` because the closing
//    quote is reached before the brace is acted on — but `'\}'` style
//    escapes inside character literals are out of scope.
//
// In practice, the common authoring cases — object literals, function
// bodies, control flow blocks, and `"..."`-delimited strings with
// braces — work correctly. The cases above are real bugs for users of
// those languages and should be fixed before any claim of full embedded-
// language support. Of these, line comments are the cheapest to handle
// and likely the highest-payoff (`// }` and `# }` are common in
// real-world code); they're a good first follow-up.
//
// ─────────────────────────────────────────────────────────────────────

enum TokenType {
    CODE_BODY,
};

void *tree_sitter_cirq_external_scanner_create(void) {
    return NULL;
}

void tree_sitter_cirq_external_scanner_destroy(void *payload) {
    (void)payload;
}

unsigned tree_sitter_cirq_external_scanner_serialize(void *payload, char *buffer) {
    (void)payload;
    (void)buffer;
    return 0;
}

void tree_sitter_cirq_external_scanner_deserialize(
    void *payload, const char *buffer, unsigned length
) {
    (void)payload;
    (void)buffer;
    (void)length;
}

// Consume characters of a string literal up to and including the closing
// quote, honoring backslash escapes. Assumes the opening quote is the
// current lookahead.
static void consume_string(TSLexer *lexer) {
    int32_t quote = lexer->lookahead;
    lexer->advance(lexer, false);

    while (!lexer->eof(lexer)) {
        int32_t c = lexer->lookahead;

        if (c == '\\') {
            // Skip the backslash and whatever it escapes (any byte,
            // including a quote or another backslash).
            lexer->advance(lexer, false);
            if (!lexer->eof(lexer)) {
                lexer->advance(lexer, false);
            }
            continue;
        }

        if (c == quote) {
            lexer->advance(lexer, false);
            return;
        }

        lexer->advance(lexer, false);
    }
}

bool tree_sitter_cirq_external_scanner_scan(
    void *payload, TSLexer *lexer, const bool *valid_symbols
) {
    (void)payload;

    if (!valid_symbols[CODE_BODY]) {
        return false;
    }

    // Skip leading whitespace without including it in the token. We don't
    // want the body to start with a run of spaces/newlines, because the
    // grammar's `extras` would normally handle them at this position;
    // refusing to consume them here lets the parser apply extras and
    // call us back at the first non-whitespace character.
    while (!lexer->eof(lexer)) {
        int32_t c = lexer->lookahead;
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r') {
            lexer->advance(lexer, true);
        } else {
            break;
        }
    }

    // If the next character is the closing brace of the `code` block at
    // depth 0, the body is empty. Refuse the token so that the optional()
    // in the grammar lets the outer `}` close the block directly.
    if (lexer->eof(lexer) || lexer->lookahead == '}') {
        return false;
    }

    int depth = 0;
    bool consumed_any = false;

    while (!lexer->eof(lexer)) {
        int32_t c = lexer->lookahead;

        if (c == '}' && depth == 0) {
            // Reached the closing brace of the `code` block. Stop without
            // consuming it — the outer grammar will match the `}` token.
            break;
        }

        consumed_any = true;

        switch (c) {
            case '"':
            case '\'':
                consume_string(lexer);
                break;
            case '{':
                depth++;
                lexer->advance(lexer, false);
                break;
            case '}':
                // depth > 0 here; this `}` closes a nested block inside
                // the embedded language.
                depth--;
                lexer->advance(lexer, false);
                break;
            default:
                lexer->advance(lexer, false);
                break;
        }
    }

    if (!consumed_any) {
        return false;
    }

    lexer->result_symbol = CODE_BODY;
    return true;
}
