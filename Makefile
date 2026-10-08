.PHONY: check spike-isolation fmt

check:
	cargo build --workspace
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace

fmt:
	cargo fmt

# Release measurement of cross-process plugin hosting. See docs/spikes/S3-plugin-isolation.md.
spike-isolation:
	cargo build --release --workspace
	cargo test --release -p echode-plugin-host --test isolation -- --nocapture

# Metal scroll/zoom measurement. See docs/spikes/S1-drawing.md.
spike-draw:
	cargo build --release -p echode-draw
	cargo test --release -p echode-draw -- --nocapture
	cd spikes/draw && swift run -c release -- --bench --report ../../target/spike-s1.txt

# Plain-text project history. See docs/spikes/S4-history.md.
spike-history:
	cargo test --release -p echode-history --test roundtrip -- --test-threads=1 --nocapture

# Trackpad pinch, haptics, and chord pressure. See docs/spikes/S5-input.md.
spike-input:
	cd spikes/input && swift test
	cd spikes/input && swift run

# 1,000-node CoreAudio graph. See docs/spikes/S2-engine.md.
spike-engine:
	cargo build --release -p echode-engine
	cargo test --release -p echode-engine -- --nocapture
	cargo run --release -p echode-engine --bin echode-engine-spike -- --seconds 600 --workers 0 --report target/spike-s2.txt
