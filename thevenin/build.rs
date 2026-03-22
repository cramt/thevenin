fn main() {
    // Ensure cargo rebuilds test binaries when the ignore list changes.
    // The proc macro `thevenin_test_macro::ngspice_tests!()` reads this file
    // at compile time to decide which harness tests get `#[ignore]`, but cargo
    // doesn't automatically track proc macro filesystem reads.
    println!("cargo::rerun-if-changed=tests/ignore.toml");
}
