//! Import resolution — reads imported files, parses them, and merges their
//! top-level declarations into the importing file's AST before IR lowering.
//!
//! Handles:
//! - `import "path.cirq"` — merges all modules, models, and circuits
//! - `import "path.cirq" as name` — planned for namespace-qualified access
//! - Cycle detection via a visited-path set
//! - Recursive resolution (imports inside imported files)

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cirq_ast::{SourceFile, TopLevel};

use crate::diagnostics::Diagnostic;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve all `import` declarations in a [`SourceFile`] by reading, parsing,
/// and merging imported files.
///
/// `base_dir` is the directory containing the source file being compiled.
/// `search_paths` are additional directories to search for imports.
///
/// Returns the merged [`SourceFile`] with imports replaced by the imported
/// declarations, plus any diagnostics from the import process.
pub fn resolve_imports(
    source_file: SourceFile,
    base_dir: &Path,
    search_paths: &[PathBuf],
) -> (SourceFile, Vec<Diagnostic>) {
    let mut visited = HashSet::new();
    let mut in_progress = HashSet::new();
    let mut diags = Vec::new();

    let items = resolve_items(
        source_file.items,
        base_dir,
        search_paths,
        &mut visited,
        &mut in_progress,
        &mut diags,
    );

    let resolved = SourceFile {
        items,
        span: source_file.span,
    };

    (resolved, diags)
}

// ---------------------------------------------------------------------------
// Internal resolution
// ---------------------------------------------------------------------------

fn resolve_items(
    items: Vec<TopLevel>,
    base_dir: &Path,
    search_paths: &[PathBuf],
    visited: &mut HashSet<PathBuf>,
    in_progress: &mut HashSet<PathBuf>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<TopLevel> {
    let mut result = Vec::new();

    for item in items {
        match item {
            TopLevel::Import(ref import) => {
                let resolved_path = resolve_path(&import.path, base_dir, search_paths);

                match resolved_path {
                    Some(path) => {
                        let canonical = match path.canonicalize() {
                            Ok(c) => c,
                            Err(e) => {
                                diags.push(
                                    Diagnostic::error(format!(
                                        "cannot resolve import path `{}`: {e}",
                                        import.path
                                    ))
                                    .with_span(import.span),
                                );
                                continue;
                            }
                        };

                        // Cycle guard: `in_progress` tracks files currently on the
                        // resolution stack. Unlike the `visited` diamond-dedup set
                        // (which named imports intentionally bypass so distinct names
                        // can be re-requested), this is consulted for *every* import.
                        // Without it, a self- or mutually-referential named import
                        // re-parses the same file forever and overflows the stack.
                        if in_progress.contains(&canonical) {
                            diags.push(
                                Diagnostic::error(format!(
                                    "circular import detected: `{}`",
                                    import.path
                                ))
                                .with_span(import.span),
                            );
                            continue;
                        }

                        // For named imports we still need to read the file even
                        // if it has been visited before (different names may be
                        // requested). For plain imports, diamond dedup applies.
                        if import.names.is_empty() && !visited.insert(canonical.clone()) {
                            continue;
                        }
                        // Record it for plain-import dedup regardless.
                        visited.insert(canonical.clone());

                        match std::fs::read_to_string(&canonical) {
                            Ok(source) => {
                                in_progress.insert(canonical.clone());
                                if import.names.is_empty() {
                                    // Plain import — merge bare items (not inside exports).
                                    let imported_items = parse_and_extract(
                                        &source,
                                        &canonical,
                                        search_paths,
                                        visited,
                                        in_progress,
                                        diags,
                                    );
                                    result.extend(imported_items);
                                } else {
                                    // Named import — extract only named export blocks.
                                    let imported_items = parse_and_extract_named(
                                        &source,
                                        &canonical,
                                        &import.names,
                                        import.span,
                                        search_paths,
                                        visited,
                                        in_progress,
                                        diags,
                                    );
                                    result.extend(imported_items);
                                }
                                in_progress.remove(&canonical);
                            }
                            Err(e) => {
                                diags.push(
                                    Diagnostic::error(format!(
                                        "cannot read import `{}`: {e}",
                                        import.path
                                    ))
                                    .with_span(import.span),
                                );
                            }
                        }
                    }
                    None => {
                        diags.push(
                            Diagnostic::error(format!("import file not found: `{}`", import.path))
                                .with_span(import.span),
                        );
                    }
                }
            }
            other => result.push(other),
        }
    }

    result
}

/// Parse an imported file and return its resolved items (with imports
/// resolved recursively). Internal helper shared by plain and named import.
fn parse_and_resolve(
    source: &str,
    file_path: &Path,
    search_paths: &[PathBuf],
    visited: &mut HashSet<PathBuf>,
    in_progress: &mut HashSet<PathBuf>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<TopLevel> {
    let tree = match crate::parser::parse(source) {
        Some(t) => t,
        None => {
            diags.push(Diagnostic::error(format!(
                "tree-sitter failed to parse imported file: {}",
                file_path.display()
            )));
            return Vec::new();
        }
    };

    let (sf, parse_diags) = crate::lower::lower(&tree, source);
    diags.extend(parse_diags);

    let import_dir = file_path.parent().unwrap_or(Path::new("."));

    // Recursively resolve imports within the imported file.
    resolve_items(
        sf.items,
        import_dir,
        search_paths,
        visited,
        in_progress,
        diags,
    )
}

/// Parse an imported file and extract bare (non-exported) top-level
/// declarations. Export blocks are kept opaque — their contents are only
/// reachable via named imports.
fn parse_and_extract(
    source: &str,
    file_path: &Path,
    search_paths: &[PathBuf],
    visited: &mut HashSet<PathBuf>,
    in_progress: &mut HashSet<PathBuf>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<TopLevel> {
    let items = parse_and_resolve(source, file_path, search_paths, visited, in_progress, diags);

    // Keep bare modules, models, circuits, funcs — skip Export blocks.
    items
        .into_iter()
        .filter(|item| {
            matches!(
                item,
                TopLevel::Module(_) | TopLevel::Model(_) | TopLevel::Circuit(_) | TopLevel::Func(_)
            )
        })
        .collect()
}

/// Parse an imported file and extract only items from the named export blocks.
///
/// `import { tt, ff } from "pdk.cirq"` ↦ items from `export tt { ... }` and
/// `export ff { ... }`.
#[allow(clippy::too_many_arguments)]
fn parse_and_extract_named(
    source: &str,
    file_path: &Path,
    names: &[cirq_ast::Ident],
    import_span: cirq_ast::span::Span,
    search_paths: &[PathBuf],
    visited: &mut HashSet<PathBuf>,
    in_progress: &mut HashSet<PathBuf>,
    diags: &mut Vec<Diagnostic>,
) -> Vec<TopLevel> {
    let items = parse_and_resolve(source, file_path, search_paths, visited, in_progress, diags);

    let mut result = Vec::new();
    let wanted: std::collections::HashSet<&str> = names.iter().map(|n| n.name.as_str()).collect();
    let mut found: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for item in &items {
        if let TopLevel::Export(export) = item
            && wanted.contains(export.name.name.as_str())
        {
            found.insert(&export.name.name);
            result.extend(export.items.clone());
        }
    }

    // Report any requested names that weren't found.
    for name in names {
        if !found.contains(name.name.as_str()) {
            diags.push(
                Diagnostic::error(format!(
                    "export `{}` not found in `{}`",
                    name.name,
                    file_path.display()
                ))
                .with_span(import_span),
            );
        }
    }

    result
}

/// Try to find the import file, first relative to `base_dir`, then in each
/// search path.
fn resolve_path(import_path: &str, base_dir: &Path, search_paths: &[PathBuf]) -> Option<PathBuf> {
    // Try relative to base_dir first.
    let candidate = base_dir.join(import_path);
    if candidate.is_file() {
        return Some(candidate);
    }

    // Try each search path.
    for dir in search_paths {
        let candidate = dir.join(import_path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: write files to a temp dir, parse the main file, resolve imports.
    fn resolve_test(files: &[(&str, &str)], main_file: &str) -> (SourceFile, Vec<Diagnostic>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path();

        for (name, content) in files {
            let path = base.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, content).unwrap();
        }

        let main_source = files
            .iter()
            .find(|(name, _)| *name == main_file)
            .expect("main file not in files list")
            .1;

        let tree = crate::parser::parse(main_source).expect("parse");
        let (sf, _parse_diags) = crate::lower::lower(&tree, main_source);

        resolve_imports(sf, base, &[])
    }

    #[test]
    fn import_module_from_file() {
        let (sf, diags) = resolve_test(
            &[
                (
                    "lib.cirq",
                    r#"
                    module inverter {
                        port inp: in
                        port outp: out
                        port vdd: inout
                        port vss: inout
                        R1: resistor(inp -> outp, 1000)
                    }
                    "#,
                ),
                (
                    "main.cirq",
                    r#"
                    import "lib.cirq"

                    circuit test {
                        V1: vsource(vdd -> gnd, dc: 3.3)
                    }
                    "#,
                ),
            ],
            "main.cirq",
        );

        // Should have no errors.
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::diagnostics::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        // The module from lib.cirq should be merged into the source file.
        let has_module = sf
            .items
            .iter()
            .any(|item| matches!(item, TopLevel::Module(m) if m.name.name == "inverter"));
        assert!(
            has_module,
            "imported module 'inverter' not found in merged AST"
        );

        // The circuit should still be present.
        let has_circuit = sf
            .items
            .iter()
            .any(|item| matches!(item, TopLevel::Circuit(c) if c.name.name == "test"));
        assert!(has_circuit, "circuit 'test' not found");
    }

    #[test]
    fn import_model_from_file() {
        let (sf, diags) = resolve_test(
            &[
                (
                    "models.cirq",
                    r#"
                    model nch: nmos {
                        vto = 0.7
                        kp = 110e-6
                    }
                    "#,
                ),
                (
                    "main.cirq",
                    r#"
                    import "models.cirq"

                    circuit test {
                        R1: resistor(a -> gnd, 1000)
                    }
                    "#,
                ),
            ],
            "main.cirq",
        );

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::diagnostics::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        let has_model = sf
            .items
            .iter()
            .any(|item| matches!(item, TopLevel::Model(m) if m.name.name == "nch"));
        assert!(has_model, "imported model 'nch' not found");
    }

    #[test]
    fn diamond_import_deduplicates() {
        let (sf, diags) = resolve_test(
            &[
                (
                    "base.cirq",
                    r#"
                    model nch: nmos { vto = 0.7 }
                    "#,
                ),
                (
                    "lib_a.cirq",
                    r#"
                    import "base.cirq"
                    module buf_a {
                        port i: inout
                        port o: inout
                        R1: resistor(i -> o, 1000)
                    }
                    "#,
                ),
                (
                    "lib_b.cirq",
                    r#"
                    import "base.cirq"
                    module buf_b {
                        port i: inout
                        port o: inout
                        R1: resistor(i -> o, 2000)
                    }
                    "#,
                ),
                (
                    "main.cirq",
                    r#"
                    import "lib_a.cirq"
                    import "lib_b.cirq"

                    circuit test {
                        V1: vsource(vdd -> gnd, dc: 3.3)
                    }
                    "#,
                ),
            ],
            "main.cirq",
        );

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::diagnostics::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        // base.cirq's model should appear exactly once (diamond dedup).
        let model_count = sf
            .items
            .iter()
            .filter(|item| matches!(item, TopLevel::Model(m) if m.name.name == "nch"))
            .count();
        assert_eq!(
            model_count, 1,
            "diamond import should deduplicate: got {model_count} copies of 'nch'"
        );
    }

    #[test]
    fn missing_import_produces_error() {
        let (_, diags) = resolve_test(
            &[(
                "main.cirq",
                r#"
                import "nonexistent.cirq"
                circuit test {}
                "#,
            )],
            "main.cirq",
        );

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::diagnostics::Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "should produce an error for missing import"
        );
        assert!(
            errors[0].message.contains("not found"),
            "error should mention file not found: {:?}",
            errors[0].message
        );
    }

    #[test]
    fn named_import_selects_export_block() {
        let (sf, diags) = resolve_test(
            &[
                (
                    "pdk.cirq",
                    r#"
                    export tt {
                        model nch: nmos { vto = 0.4 }
                        model pch: pmos { vto = -0.4 }
                    }

                    export ff {
                        model nch: nmos { vto = 0.35 }
                        model pch: pmos { vto = -0.35 }
                    }

                    // Bare item — not inside any export block.
                    model common_diode: diode { is = 1e-14 }
                    "#,
                ),
                (
                    "main.cirq",
                    r#"
                    import { tt } from "pdk.cirq"

                    circuit test {
                        R1: resistor(a -> gnd, 1000)
                    }
                    "#,
                ),
            ],
            "main.cirq",
        );

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::diagnostics::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        // Should have the two models from export tt.
        let model_names: Vec<_> = sf
            .items
            .iter()
            .filter_map(|item| match item {
                TopLevel::Model(m) => Some(m.name.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(model_names.contains(&"nch"), "nch from export tt not found");
        assert!(model_names.contains(&"pch"), "pch from export tt not found");

        // Should NOT have the bare model (not in any export).
        assert!(
            !model_names.contains(&"common_diode"),
            "bare model should not be imported via named import"
        );
    }

    #[test]
    fn named_import_missing_export_produces_error() {
        let (_, diags) = resolve_test(
            &[
                (
                    "pdk.cirq",
                    r#"
                    export tt {
                        model nch: nmos { vto = 0.4 }
                    }
                    "#,
                ),
                (
                    "main.cirq",
                    r#"
                    import { ss } from "pdk.cirq"
                    circuit test {}
                    "#,
                ),
            ],
            "main.cirq",
        );

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::diagnostics::Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "should produce error for missing export"
        );
        assert!(
            errors[0].message.contains("export `ss` not found"),
            "error should mention missing export: {:?}",
            errors[0].message
        );
    }

    #[test]
    fn plain_import_skips_export_blocks() {
        let (sf, diags) = resolve_test(
            &[
                (
                    "lib.cirq",
                    r#"
                    model base_model: nmos { vto = 0.5 }

                    export corner {
                        model corner_model: nmos { vto = 0.4 }
                    }
                    "#,
                ),
                (
                    "main.cirq",
                    r#"
                    import "lib.cirq"
                    circuit test {}
                    "#,
                ),
            ],
            "main.cirq",
        );

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::diagnostics::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        let model_names: Vec<_> = sf
            .items
            .iter()
            .filter_map(|item| match item {
                TopLevel::Model(m) => Some(m.name.name.as_str()),
                _ => None,
            })
            .collect();

        // Plain import should get bare items.
        assert!(
            model_names.contains(&"base_model"),
            "bare model should be imported"
        );
        // But NOT export block items.
        assert!(
            !model_names.contains(&"corner_model"),
            "export block items should not be imported via plain import"
        );
    }

    #[test]
    fn named_import_multiple_exports() {
        let (sf, diags) = resolve_test(
            &[
                (
                    "pdk.cirq",
                    r#"
                    export tt {
                        model nch_tt: nmos { vto = 0.4 }
                    }
                    export ff {
                        model nch_ff: nmos { vto = 0.35 }
                    }
                    export ss {
                        model nch_ss: nmos { vto = 0.45 }
                    }
                    "#,
                ),
                (
                    "main.cirq",
                    r#"
                    import { tt, ff } from "pdk.cirq"
                    circuit test {}
                    "#,
                ),
            ],
            "main.cirq",
        );

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::diagnostics::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        let model_names: Vec<_> = sf
            .items
            .iter()
            .filter_map(|item| match item {
                TopLevel::Model(m) => Some(m.name.name.as_str()),
                _ => None,
            })
            .collect();

        assert!(
            model_names.contains(&"nch_tt"),
            "tt export should be imported"
        );
        assert!(
            model_names.contains(&"nch_ff"),
            "ff export should be imported"
        );
        assert!(
            !model_names.contains(&"nch_ss"),
            "ss export should NOT be imported"
        );
    }

    #[test]
    fn nested_import_resolution() {
        let (sf, diags) = resolve_test(
            &[
                (
                    "primitives.cirq",
                    r#"
                    model nch: nmos { vto = 0.7 }
                    "#,
                ),
                (
                    "cells.cirq",
                    r#"
                    import "primitives.cirq"
                    module inv {
                        port i: inout
                        port o: inout
                        R1: resistor(i -> o, 1000)
                    }
                    "#,
                ),
                (
                    "main.cirq",
                    r#"
                    import "cells.cirq"
                    circuit test {
                        V1: vsource(vdd -> gnd, dc: 3.3)
                    }
                    "#,
                ),
            ],
            "main.cirq",
        );

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::diagnostics::Severity::Error)
            .collect();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");

        // Both the model from primitives.cirq and the module from cells.cirq
        // should be present.
        let has_model = sf
            .items
            .iter()
            .any(|item| matches!(item, TopLevel::Model(m) if m.name.name == "nch"));
        let has_module = sf
            .items
            .iter()
            .any(|item| matches!(item, TopLevel::Module(m) if m.name.name == "inv"));
        assert!(has_model, "transitively imported model 'nch' not found");
        assert!(has_module, "imported module 'inv' not found");
    }

    #[test]
    fn named_import_cycle_terminates_with_diagnostic() {
        // Two files whose named exports import each other. Before the
        // in-progress cycle guard, named imports bypassed the `visited`
        // dedup set entirely and this re-parsed each file forever until the
        // native stack overflowed. It must now terminate with a diagnostic.
        let (_sf, diags) = resolve_test(
            &[
                (
                    "a.cirq",
                    r#"
                    import { b_cell } from "b.cirq"
                    export a_cell {
                        model nch: nmos { vto = 0.4 }
                    }
                    "#,
                ),
                (
                    "b.cirq",
                    r#"
                    import { a_cell } from "a.cirq"
                    export b_cell {
                        model pch: pmos { vto = -0.4 }
                    }
                    "#,
                ),
                (
                    "main.cirq",
                    r#"
                    import { a_cell } from "a.cirq"

                    circuit test {
                        R1: resistor(a -> gnd, 1000)
                    }
                    "#,
                ),
            ],
            "main.cirq",
        );

        assert!(
            diags.iter().any(|d| d.message.contains("circular import")),
            "expected a circular-import diagnostic, got: {diags:?}"
        );
    }

    #[test]
    fn named_self_import_terminates() {
        // A file that names-imports itself must not recurse forever.
        let (_sf, diags) = resolve_test(
            &[(
                "main.cirq",
                r#"
                    import { thing } from "main.cirq"
                    export thing {
                        model nch: nmos { vto = 0.4 }
                    }
                    "#,
            )],
            "main.cirq",
        );

        assert!(
            diags.iter().any(|d| d.message.contains("circular import")),
            "expected a circular-import diagnostic for self-import, got: {diags:?}"
        );
    }
}
