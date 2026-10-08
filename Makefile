.PHONY: check fmt setup known-bugs advisories bless spike-isolation spike-draw spike-history spike-input spike-engine

# What every change must pass. It needs no audio hardware and has no timing thresholds,
# so a failure always means the code is wrong.
check:
	cargo build --workspace
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace
	scripts/check-ffi.sh
	@command -v cargo-deny >/dev/null || { echo "cargo-deny is missing: run make setup"; exit 1; }
	cargo deny check licenses bans sources

fmt:
	cargo fmt

# Tools `make check` needs besides the toolchain in rust-toolchain.toml.
setup:
	cargo install --locked cargo-deny@0.20.2

# Rewrites crates/echode-cli/tests/fixtures/sine-pad-a4-128.wav and appends REASON
# to tests/fixtures/bless.log. Use this when a sound change is intentional.
bless:
	@test -n "$(REASON)" || { echo 'usage: make bless REASON="why the reference changed"'; exit 1; }
	BLESS=1 cargo test -p echode-cli --test render -- --ignored bless_the_reference --exact
	mkdir -p crates/echode-cli/tests/fixtures
	printf '%s %s\n' "$$(date +%Y-%m-%d)" "$(REASON)" >> crates/echode-cli/tests/fixtures/bless.log

# Ignored tests named audit_* reproduce open items in docs/reviews/2026-10-08-audit.md.
# Each one fails until its item is fixed; then remove its #[ignore].
known-bugs:
	cargo test --workspace --no-fail-fast -- --ignored audit_

# Advisories need the network, so they are not part of `make check`.
advisories:
	cargo deny check advisories

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
	cargo test --release -p echode-history --test roundtrip -- --include-ignored --test-threads=1 --nocapture

# Trackpad pinch, haptics, and chord pressure. See docs/spikes/S5-input.md.
spike-input:
	cd spikes/input && swift test
	cd spikes/input && swift run

# 1,000-node CoreAudio graph. See docs/spikes/S2-engine.md.
spike-engine:
	cargo build --release -p echode-engine
	cargo test --release -p echode-engine -- --include-ignored --skip audit_ --nocapture
	cargo run --release -p echode-engine --bin echode-engine-spike -- --seconds 600 --workers 0 --report target/spike-s2.txt
