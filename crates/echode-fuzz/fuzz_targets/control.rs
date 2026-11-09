//! libFuzzer entry for [`echode_fuzz::Target::Control`]. Run with `make fuzz FUZZ_TARGET=control`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| echode_fuzz::Target::Control.run(data));
