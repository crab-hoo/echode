fn main() {
    if std::env::args().nth(1).as_deref() == Some("sanitizer-header") {
        let output = std::env::args().nth(2).expect("sanitizer-header OUTPUT");
        cbindgen::Builder::new()
            .with_src(concat!(env!("CARGO_MANIFEST_DIR"), "/sanitizers/probe.rs"))
            .with_language(cbindgen::Language::C)
            .generate()
            .expect("sanitizer control header")
            .write_to_file(output);
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("c-header") {
        let output = std::env::args().nth(2).expect("c-header OUTPUT");
        cbindgen::Builder::new()
            .with_crate(env!("CARGO_MANIFEST_DIR"))
            .with_config(
                cbindgen::Config::from_file(concat!(env!("CARGO_MANIFEST_DIR"), "/cbindgen.toml"))
                    .expect("cbindgen config"),
            )
            .generate()
            .expect("C ABI header")
            .write_to_file(output);
        return;
    }
    uniffi::uniffi_bindgen_main();
}
