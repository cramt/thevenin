//! Proc macro that auto-discovers ngspice-upstream test cases at compile time.
//!
//! Walks `ngspice-upstream/tests/`, finds all `.cir`/`.out` pairs, resolves
//! `.include` directives, collects auxiliary files (`.lib`, `.mod`, etc.),
//! and generates one `#[test]` function per test case with all file contents
//! embedded as string literals — no filesystem access needed at runtime.

extern crate proc_macro;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use facet::Facet;
use proc_macro2::{Literal, TokenStream as TokenStream2};
use quote::{format_ident, quote};

/// Per-test tolerance override parsed from `tolerances.toml`.
#[derive(Debug, Facet)]
struct ToleranceOverride {
    rel_tol: f64,
}

/// Generates integration tests for all `.cir` files in `ngspice-upstream/tests/`.
///
/// Usage (in a test file):
/// ```ignore
/// thevenin_test_macro::ngspice_tests!();
/// ```
///
/// Each `.cir` file with a matching `.out` reference becomes a `#[test]` function.
/// Tests listed in `tests/ignore.toml` get `#[ignore = "reason"]`.
///
/// All file contents (`.cir` with includes resolved, `.out` reference, auxiliary
/// `.lib`/`.mod` files) are embedded at compile time so tests work on WASM too.
#[proc_macro]
pub fn ngspice_tests(_input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    match generate_tests() {
        Ok(tokens) => tokens.into(),
        Err(e) => {
            let msg = format!("ngspice_tests! error: {e}");
            quote! { compile_error!(#msg); }.into()
        }
    }
}

fn generate_tests() -> Result<TokenStream2, String> {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").map_err(|_| "CARGO_MANIFEST_DIR not set")?,
    );
    let ngspice_tests_dir = manifest_dir
        .parent()
        .ok_or("no parent dir")?
        .join("ngspice-upstream")
        .join("tests");
    let fixture_root = manifest_dir.join("tests").join("fixtures");
    let ignore_path = manifest_dir.join("tests").join("ignore.toml");
    let tolerances_path = manifest_dir.join("tests").join("tolerances.toml");

    // If ngspice-upstream isn't checked out, generate a single warning test.
    if !ngspice_tests_dir.is_dir() {
        return Ok(quote! {
            #[test]
            fn ngspice_upstream_not_found() {
                eprintln!("WARNING: ngspice-upstream/tests/ not found, skipping all harness tests");
            }
        });
    }

    // Load ignore list (flat TOML table: "path/to/file.cir" = "reason")
    let ignores: BTreeMap<String, String> = if ignore_path.exists() {
        let content = std::fs::read_to_string(&ignore_path)
            .map_err(|e| format!("failed to read ignore.toml: {e}"))?;
        facet_toml::from_str(&content).map_err(|e| format!("failed to parse ignore.toml: {e}"))?
    } else {
        BTreeMap::new()
    };

    // Load per-test tolerance overrides (TOML table: "path" = { rel_tol = 0.005 })
    let tolerances: BTreeMap<String, ToleranceOverride> = if tolerances_path.exists() {
        let content = std::fs::read_to_string(&tolerances_path)
            .map_err(|e| format!("failed to read tolerances.toml: {e}"))?;
        facet_toml::from_str(&content)
            .map_err(|e| format!("failed to parse tolerances.toml: {e}"))?
    } else {
        BTreeMap::new()
    };

    // Collect all .cir files
    let mut cir_files = Vec::new();
    collect_cir_files(&ngspice_tests_dir, &mut cir_files);

    let mut tests = Vec::new();

    for cir_path in &cir_files {
        let rel_path = cir_path
            .strip_prefix(&ngspice_tests_dir)
            .map_err(|e| format!("strip_prefix: {e}"))?;
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");

        // Find .out file (fixture override first, then upstream)
        let out_path = {
            let fixture_out = fixture_root.join(rel_path).with_extension("out");
            if fixture_out.exists() {
                fixture_out
            } else {
                cir_path.with_extension("out")
            }
        };
        if !out_path.exists() {
            continue; // skip .cir without reference output
        }

        // Read and resolve .include directives at compile time
        let cir_content = std::fs::read_to_string(cir_path)
            .map_err(|e| format!("read {}: {e}", cir_path.display()))?;
        let cir_dir = cir_path.parent().unwrap();
        let resolved_cir = resolve_includes(&cir_content, cir_dir);

        // Read reference output
        let out_content = std::fs::read_to_string(&out_path)
            .map_err(|e| format!("read {}: {e}", out_path.display()))?;

        // Collect auxiliary files (everything in the directory that isn't .cir or .out)
        let aux_files = collect_aux_files(cir_dir);

        // Derive test function name
        let test_name = derive_test_name(&rel_str);

        // Check ignore list
        let ignore_reason = ignores.get(&rel_str);

        // Check tolerance overrides (only applies if test is NOT ignored)
        let tol_override = if ignore_reason.is_none() {
            tolerances.get(&rel_str)
        } else {
            None
        };

        tests.push(generate_test_fn(
            &test_name,
            &rel_str,
            &resolved_cir,
            &out_content,
            &aux_files,
            ignore_reason,
            tol_override,
        ));
    }

    // Emit include_str! for config files so cargo tracks them as dependencies
    // and recompiles when they change.
    let ignore_str = ignore_path.to_string_lossy().to_string();
    let ignore_lit = Literal::string(&ignore_str);
    let tol_str = tolerances_path.to_string_lossy().to_string();
    let tol_lit = Literal::string(&tol_str);

    Ok(quote! {
        #[allow(dead_code)]
        const _IGNORE_TOML: &str = include_str!(#ignore_lit);
        #[allow(dead_code)]
        const _TOLERANCES_TOML: &str = include_str!(#tol_lit);
        #(#tests)*
    })
}

/// Recursively collect all `.cir` files under `dir`, sorted deterministically.
fn collect_cir_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_cir_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "cir") {
            out.push(path);
        }
    }
}

/// Resolve `.include` / `.inc` directives by inlining file contents.
///
/// Done at compile time so the resolved content can be embedded as a string literal.
fn resolve_includes(content: &str, base_dir: &Path) -> String {
    let mut result = String::new();
    for line in content.lines() {
        let trimmed = line.trim().to_lowercase();
        if trimmed.starts_with(".include") || trimmed.starts_with(".inc") {
            let parts: Vec<&str> = line.trim().splitn(2, char::is_whitespace).collect();
            if parts.len() == 2 {
                let filename = parts[1].trim().trim_matches('"').trim_matches('\'');
                if let Some(included) = read_include(base_dir, filename) {
                    result.push_str(&included);
                    result.push('\n');
                    continue;
                }
                // If we can't find the include, leave the directive as-is
                // (will produce a runtime error, which is the expected behavior)
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

/// Try to read an include file, with case-insensitive fallback.
fn read_include(dir: &Path, filename: &str) -> Option<String> {
    // Try exact path first
    let path = dir.join(filename);
    if let Ok(content) = std::fs::read_to_string(&path) {
        return Some(content);
    }
    // Case-insensitive fallback
    let lower = filename.to_lowercase();
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().to_lowercase() == lower {
            return std::fs::read_to_string(entry.path()).ok();
        }
    }
    None
}

/// Collect all non-.cir non-.out files in a directory as (filename, content) pairs.
///
/// These are the auxiliary files (.lib, .mod, etc.) needed by `.lib` processing
/// at runtime. Collecting everything in the directory is simple and robust.
fn collect_aux_files(dir: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        // Skip .cir and .out files (already handled separately)
        if ext == "cir" || ext == "out" {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            files.push((name, content));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// Derive a Rust test function name from a relative `.cir` path.
///
/// `"bsim3soidd/t5.cir"` → `"harness_bsim3soidd_t5"`
/// `"regression/lib-processing/ex1a.cir"` → `"harness_regression_lib_processing_ex1a"`
fn derive_test_name(rel_path: &str) -> String {
    let stem = rel_path.strip_suffix(".cir").unwrap_or(rel_path);
    let name: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    // Collapse multiple underscores and strip leading/trailing ones
    let mut collapsed = String::with_capacity(name.len() + 8);
    collapsed.push_str("harness_");
    let mut prev_underscore = true; // treat start as after underscore to skip leading _
    for c in name.chars() {
        if c == '_' {
            if !prev_underscore {
                collapsed.push('_');
            }
            prev_underscore = true;
        } else {
            collapsed.push(c);
            prev_underscore = false;
        }
    }
    // Strip trailing underscore
    if collapsed.ends_with('_') {
        collapsed.pop();
    }
    collapsed
}

/// Generate a single `#[test]` function with embedded file contents.
fn generate_test_fn(
    test_name: &str,
    rel_path: &str,
    cir_content: &str,
    out_content: &str,
    aux_files: &[(String, String)],
    ignore_reason: Option<&String>,
    tol_override: Option<&ToleranceOverride>,
) -> TokenStream2 {
    let fn_name = format_ident!("{}", test_name);
    let path_lit = Literal::string(rel_path);
    let cir_lit = Literal::string(cir_content);
    let out_lit = Literal::string(out_content);

    let aux_names: Vec<Literal> = aux_files.iter().map(|(n, _)| Literal::string(n)).collect();
    let aux_contents: Vec<Literal> = aux_files.iter().map(|(_, c)| Literal::string(c)).collect();

    let ignore_attr = ignore_reason.map(|reason| {
        let reason_lit = Literal::string(reason);
        quote! { #[ignore = #reason_lit] }
    });

    let rel_tol_arg = match tol_override {
        Some(tol) => {
            let val = Literal::f64_unsuffixed(tol.rel_tol);
            quote! { Some(#val) }
        }
        None => quote! { None },
    };

    quote! {
        #[test]
        #ignore_attr
        fn #fn_name() {
            run_embedded_test(
                #path_lit,
                #cir_lit,
                #out_lit,
                &[#( (#aux_names, #aux_contents) ),*],
                #rel_tol_arg,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_name() {
        assert_eq!(
            derive_test_name("bsim3soidd/t5.cir"),
            "harness_bsim3soidd_t5"
        );
        assert_eq!(
            derive_test_name("regression/lib-processing/ex1a.cir"),
            "harness_regression_lib_processing_ex1a"
        );
        assert_eq!(
            derive_test_name("filters/lowpass.cir"),
            "harness_filters_lowpass"
        );
        assert_eq!(
            derive_test_name("regression/misc/ac-zero.cir"),
            "harness_regression_misc_ac_zero"
        );
    }
}
