fn main() {
    println!("Hello, world!");
}

#[cfg(test)]
mod tests {
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    #[test]
    fn test_hello() {
        assert_eq!(1 + 1, 2);
    }
}
