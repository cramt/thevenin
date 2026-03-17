//! Attempt to parse every `.cir` fixture file through the SPICE → IR path.
//! Any parse failure is a test failure.

use std::path::PathBuf;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("thevenin/tests/fixtures")
}

fn collect_cir_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_cir_files(&path));
            } else if path.extension().is_some_and(|e| e == "cir") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[test]
fn all_fixture_cir_files_parse_to_ir() {
    let dir = fixture_dir();
    let files = collect_cir_files(&dir);
    assert!(
        !files.is_empty(),
        "no .cir files found in {}",
        dir.display()
    );

    let mut failures: Vec<(PathBuf, String)> = Vec::new();

    for path in &files {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                failures.push((path.clone(), format!("read error: {e}")));
                continue;
            }
        };

        match thevenin_cirq::from_spice::from_spice(&src) {
            Ok(circuit) => {
                // Sanity: name should be non-empty
                assert!(
                    !circuit.name.is_empty(),
                    "{}: circuit name is empty",
                    path.display()
                );
            }
            Err(e) => {
                failures.push((path.clone(), format!("{e}")));
            }
        }
    }

    if !failures.is_empty() {
        let mut msg = format!("\n{} / {} files failed:\n", failures.len(), files.len());
        for (path, err) in &failures {
            let rel = path.strip_prefix(&dir).unwrap_or(path);
            msg.push_str(&format!("  {} — {}\n", rel.display(), err));
        }
        panic!("{msg}");
    }

    eprintln!("Successfully parsed {} .cir files to IR", files.len());
}
