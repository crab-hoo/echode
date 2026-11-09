//! libFuzzer entry for [`echode_fuzz::Target::Region`]. Run with `make fuzz FUZZ_TARGET=region`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| echode_fuzz::Target::Region.run(data));
