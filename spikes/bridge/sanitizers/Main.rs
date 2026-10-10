#[path = "probe.rs"]
mod probe;

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("rust-memory") => {
            // Intentional fault, executed only as an expected-failing child.
            unsafe { probe::eb_sanitizer_memory_fault() };
        }
        _ => {
            assert_eq!(probe::eb_sanitizer_clean(), 42);
            println!("PASS Rust sanitizer runtime");
        }
    }
}
