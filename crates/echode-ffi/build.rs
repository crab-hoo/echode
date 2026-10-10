fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("echode.h");
    let config = cbindgen::Config::from_file("cbindgen.toml").expect("cbindgen config");
    cbindgen::Builder::new()
        .with_crate(crate_dir)
        .with_config(config)
        .generate()
        .expect("generate the C header")
        .write_to_file(out);
}
