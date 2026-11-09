//! libFuzzer entry for [`echode_fuzz::Target::Journal`]. Run with `make fuzz FUZZ_TARGET=journal`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| echode_fuzz::Target::Journal.run(data));
