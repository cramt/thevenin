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
    let mut diags = Vec::new();

    let items = resolve_items(
        source_file.items,
        base_dir,
        search_paths,
        &mut visited,
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

                        if !visited.insert(canonical.clone()) {
                            // Already imported — skip silently (not an error,
                            // just a diamond dependency).
                            continue;
                        }

                        match std::fs::read_to_string(&canonical) {
                            Ok(source) => {
                                let imported_items = parse_and_extract(
                                    &source,
                                    &canonical,
                                    search_paths,
                                    visited,
                                    diags,
                                );
                                result.extend(imported_items);
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

/// Parse an imported file and extract its top-level declarations, recursively
/// resolving any imports within.
fn parse_and_extract(
    source: &str,
    file_path: &Path,
    search_paths: &[PathBuf],
    visited: &mut HashSet<PathBuf>,
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
    let items = resolve_items(sf.items, import_dir, search_paths, visited, diags);

    // Filter to only exportable declarations: modules, models, and circuits.
    // We don't re-export imports (they've already been resolved above).
    items
        .into_iter()
        .filter(|item| {
            matches!(
                item,
                TopLevel::Module(_) | TopLevel::Model(_) | TopLevel::Circuit(_)
            )
        })
        .collect()
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
}
