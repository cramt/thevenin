//! Integration tests for `.include` / `.lib` resolution.
//!
//! Covers the importer-side preprocessor that splices referenced files into
//! the netlist before parsing. See `cirq-spice-import/src/preprocess.rs`.

use std::fs;

use cirq_spice_import::{ImportError, IncludeOptions, import_spice_with_options};

/// Build an IncludeOptions rooted at `dir`.
fn opts(dir: &std::path::Path) -> IncludeOptions {
    IncludeOptions::new().with_source_dir(dir)
}

#[test]
fn include_resolves_sibling_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("child.cir"), "R_in_child 1 0 1k\n").unwrap();

    let src = "Test\n.include child.cir\nR_in_parent 2 0 2k\n.end\n";
    let circuits = import_spice_with_options(src, &opts(dir)).unwrap();
    assert_eq!(circuits.len(), 1);
    let circ = &circuits[0];

    // Both resistors must show up after the spliced include.
    let names: Vec<_> = circ.elements.iter().map(|e| e.name.as_str()).collect();
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("R_in_child")));
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("R_in_parent")));
}

#[test]
fn nested_include_three_levels() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();

    // C → leaf
    fs::write(dir.join("c.cir"), "R_c 3 0 3k\n").unwrap();
    // B includes C
    fs::write(dir.join("b.cir"), ".include c.cir\nR_b 2 0 2k\n").unwrap();
    // A includes B
    fs::write(dir.join("a.cir"), ".include b.cir\nR_a 1 0 1k\n").unwrap();

    let src = "Nested\n.include a.cir\n.end\n";
    let circuits = import_spice_with_options(src, &opts(dir)).unwrap();
    let names: Vec<_> = circuits[0]
        .elements
        .iter()
        .map(|e| e.name.to_ascii_lowercase())
        .collect();
    assert!(names.contains(&"r_a".to_string()));
    assert!(names.contains(&"r_b".to_string()));
    assert!(names.contains(&"r_c".to_string()));
}

#[test]
fn search_path_resolution_uses_lib_paths() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    let lib_dir = dir.join("lib");
    fs::create_dir_all(&lib_dir).unwrap();
    fs::write(lib_dir.join("models.cir"), "R_from_lib 9 0 9k\n").unwrap();

    let src = "Search\n.include models.cir\n.end\n";
    // Note: the source dir does NOT contain models.cir — only `lib` does.
    let options = IncludeOptions::new()
        .with_source_dir(dir)
        .add_lib_path(&lib_dir);
    let circuits = import_spice_with_options(src, &options).unwrap();
    let names: Vec<_> = circuits[0]
        .elements
        .iter()
        .map(|e| e.name.to_ascii_lowercase())
        .collect();
    assert!(names.contains(&"r_from_lib".to_string()));
}

#[test]
fn lib_two_arg_extracts_named_block_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    let body = "\
.lib tt
R_tt 1 0 1k
.endl tt
.lib ss
R_ss 1 0 2k
.endl ss
.lib ff
R_ff 1 0 3k
.endl ff
";
    fs::write(dir.join("corners.lib"), body).unwrap();

    let src = "PDK\n.lib corners.lib ss\n.end\n";
    let circuits = import_spice_with_options(src, &opts(dir)).unwrap();
    let names: Vec<_> = circuits[0]
        .elements
        .iter()
        .map(|e| e.name.to_ascii_lowercase())
        .collect();
    assert!(names.contains(&"r_ss".to_string()));
    assert!(!names.contains(&"r_tt".to_string()));
    assert!(!names.contains(&"r_ff".to_string()));
}

#[test]
fn lib_one_arg_marker_inside_included_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    // The included file contains `.lib name` / `.endl` markers. The OUTER
    // include selects which section by using the two-arg form.
    let body = "\
.lib tt
R_tt 1 0 1k
.endl tt
.lib ss
R_ss 1 0 2k
.endl ss
";
    fs::write(dir.join("models.lib"), body).unwrap();

    let src = "PDK\n.lib models.lib ss\n.end\n";
    let circuits = import_spice_with_options(src, &opts(dir)).unwrap();
    let names: Vec<_> = circuits[0]
        .elements
        .iter()
        .map(|e| e.name.to_ascii_lowercase())
        .collect();
    assert!(names.contains(&"r_ss".to_string()));
    assert!(!names.contains(&"r_tt".to_string()));
}

#[test]
fn circular_include_is_caught() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("a.cir"), ".include b.cir\n").unwrap();
    fs::write(dir.join("b.cir"), ".include a.cir\n").unwrap();

    let src = "Cycle\n.include a.cir\n.end\n";
    let err = import_spice_with_options(src, &opts(dir)).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("circular") || msg.contains("nesting"),
        "expected circular-include error, got: {msg}",
    );
}

#[test]
fn latin1_encoded_file_does_not_panic() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();

    // Bytes: `* cafÃ©` in Latin-1 (0xE9 is `é`). Write raw bytes so we can
    // smuggle in a non-UTF-8 sequence.
    let bytes: Vec<u8> = b"* caf\xE9\nR_l1 1 0 1k\n".to_vec();
    fs::write(dir.join("latin1.cir"), &bytes).unwrap();

    let src = "Latin1\n.include latin1.cir\n.end\n";
    let circuits = import_spice_with_options(src, &opts(dir)).unwrap();
    let names: Vec<_> = circuits[0]
        .elements
        .iter()
        .map(|e| e.name.to_ascii_lowercase())
        .collect();
    assert!(names.contains(&"r_l1".to_string()));
}

#[test]
fn missing_file_reports_searched_paths() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    let lib = dir.join("libs");
    fs::create_dir_all(&lib).unwrap();
    let options = IncludeOptions::new()
        .with_source_dir(dir)
        .add_lib_path(&lib);

    let src = "Missing\n.include nope.cir\n.end\n";
    let err = import_spice_with_options(src, &options).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("nope.cir"),
        "should name the missing file: {msg}"
    );
    // The error should reference at least one of the directories that were
    // searched. We accept either the source dir or the lib dir.
    let source_str = dir.to_string_lossy().to_string();
    let lib_str = lib.to_string_lossy().to_string();
    assert!(
        msg.contains(&source_str) || msg.contains(&lib_str),
        "should report searched dirs; got: {msg}",
    );
}

#[test]
fn malformed_include_yields_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    let src = "Bad\n.include\n.end\n";
    let err = import_spice_with_options(src, &opts(dir)).unwrap_err();
    match err {
        ImportError::Include(_) => {}
        other => panic!("expected Include error, got {other:?}"),
    }
}

#[test]
fn include_without_options_still_works_for_cwd_relative() {
    // Sanity: if no source_dir is provided, CWD-relative resolution kicks in.
    // We just verify the call signature is usable; we don't depend on CWD.
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    // Use an absolute path inside the .include so CWD doesn't matter.
    let target = dir.join("abs.cir");
    fs::write(&target, "R_abs 1 0 1k\n").unwrap();

    let src = format!("Abs\n.include \"{}\"\n.end\n", target.display());
    let circuits = import_spice_with_options(&src, &IncludeOptions::new()).unwrap();
    let names: Vec<_> = circuits[0]
        .elements
        .iter()
        .map(|e| e.name.to_ascii_lowercase())
        .collect();
    assert!(names.contains(&"r_abs".to_string()));
}

#[test]
fn lib_section_not_found_errors_cleanly() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    fs::write(dir.join("models.lib"), ".lib tt\nR_tt 1 0 1k\n.endl tt\n").unwrap();
    let src = "Missing\n.lib models.lib ss\n.end\n";
    let err = import_spice_with_options(src, &opts(dir)).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("ss"), "should name the missing section: {msg}");
}
