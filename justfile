# Task runner for Echode. `make` exposes the same targets when `just` is not installed.

check:
    cargo build --workspace
    cargo fmt --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

fmt:
    cargo fmt

spike-isolation:
    cargo build --release --workspace
    cargo test --release -p echode-plugin-host --test isolation -- --nocapture

spike-draw:
    cargo build --release -p echode-draw
    cargo test --release -p echode-draw -- --nocapture
    cd spikes/draw && swift run -c release -- --bench --report ../../target/spike-s1.txt

spike-history:
    cargo test --release -p echode-history --test roundtrip -- --test-threads=1 --nocapture

spike-input:
    cd spikes/input && swift test
    cd spikes/input && swift run

spike-engine:
    cargo build --release -p echode-engine
    cargo test --release -p echode-engine -- --nocapture
    cargo run --release -p echode-engine --bin echode-engine-spike -- --seconds 600 --workers 0 --report target/spike-s2.txt
