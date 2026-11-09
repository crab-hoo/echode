//! libFuzzer entry for [`echode_fuzz::Target::Audio`]. Run with `make fuzz FUZZ_TARGET=audio`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| echode_fuzz::Target::Audio.run(data));
