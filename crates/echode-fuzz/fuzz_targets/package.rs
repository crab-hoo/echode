//! libFuzzer entry for [`echode_fuzz::Target::Package`]. Run with `make fuzz FUZZ_TARGET=package`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| echode_fuzz::Target::Package.run(data));
