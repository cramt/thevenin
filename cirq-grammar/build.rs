fn main() {
    let src_dir = std::path::Path::new("src");
    let parser_c = src_dir.join("parser.c");
    let scanner_c = src_dir.join("scanner.c");

    println!("cargo:rerun-if-changed={}", parser_c.display());
    println!("cargo:rerun-if-changed={}", scanner_c.display());

    cc::Build::new()
        .include(src_dir)
        .file(&parser_c)
        .file(&scanner_c)
        .warnings(false)
        .compile("tree_sitter_cirq");
}
