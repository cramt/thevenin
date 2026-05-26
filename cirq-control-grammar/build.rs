fn main() {
    let src_dir = std::path::Path::new("src");
    let parser_c = src_dir.join("parser.c");

    println!("cargo:rerun-if-changed={}", parser_c.display());

    cc::Build::new()
        .include(src_dir)
        .file(&parser_c)
        .warnings(false)
        .compile("tree_sitter_control");
}
