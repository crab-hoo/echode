.PHONY: app spike-app spike-arrange-frame spike-piano-frame spike-audio check verify verify-plan check-verification fmt setup setup-bridge known-bugs advisories bless spike-isolation spike-draw spike-history spike-input check-input spike-engine spike-bridge spike-render check-bridge check-bridge-features check-bridge-asan check-bridge-tsan check-bridge-miri check-shell rtsan

app:
	bash scripts/build-app.sh

# Product arrange and piano rendering continue while MainActor is deliberately busy.
spike-app:
	bash scripts/build-app.sh
	swift run --package-path swift/EchodeApp --scratch-path target/echode-app EchodeAppProbe "$(CURDIR)/target/spike-app.json"

# Actual GPU pixels, without creating windows or controlling the computer UI.
spike-arrange-frame:
	cargo build -p echode-ffi
	swift run --package-path swift/EchodeApp --scratch-path target/echode-app EchodeAppProbe --offscreen "$(CURDIR)/target/spike-arrange-frame.json"

spike-piano-frame:
	cargo build -p echode-ffi
	swift run --package-path swift/EchodeApp --scratch-path target/echode-app EchodeAppProbe --offscreen-piano "$(CURDIR)/target/spike-piano-frame.json"

# What every change must pass. It needs no audio hardware and has no timing thresholds,
# so a failure always means the code is wrong.
check:
	$(MAKE) check-verification
	cargo build --workspace
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace
	scripts/check-ffi.sh
	@command -v cargo-deny >/dev/null || { echo "cargo-deny is missing: run make setup"; exit 1; }
	cargo deny check licenses bans sources
	$(MAKE) check-shell
	$(MAKE) check-bridge

# Selection and evidence are shared by agents, the stop hook, and CI.
verify:
	python3 scripts/verify.py $(VERIFY_ARGS)

verify-plan:
	python3 scripts/verify.py --plan $(VERIFY_ARGS)

check-verification:
	PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/tests -v

fmt:
	cargo fmt

# Tools `make check` needs besides the toolchain in rust-toolchain.toml.
setup:
	cargo install --locked cargo-deny@0.20.2

# Fixed validation toolchain; never changes the product compiler/default.
setup-bridge:
	bash scripts/setup-bridge.sh

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
	cargo test --release -p echode-plugin-host --test isolation -- --include-ignored --test-threads=1 --nocapture
	cargo test --release -p echode-plugin-host --test hosted_playback -- --include-ignored --test-threads=1 --nocapture

# Metal scroll/zoom measurement. See docs/spikes/S1-drawing.md.
spike-draw:
	cargo build --release -p echode-draw
	cargo test --release -p echode-draw -- --nocapture
	cd spikes/draw && swift run -c release EchodeDraw --bench --report ../../target/spike-s1.txt

# Plain-text project history. See docs/spikes/S4-history.md.
spike-history:
	cargo test --release -p echode-history --test roundtrip -- --include-ignored --test-threads=1 --nocapture

# Trackpad pinch, haptics, and chord pressure. See docs/spikes/S5-input.md.
spike-input:
	$(MAKE) check-input
	cd spikes/input && swift run

check-input:
	swift test --package-path spikes/input --scratch-path target/input-tests

# Timing belongs in release measurements, not in the normal correctness checks.
spike-render:
	cargo test --release -p echode-cli --test render a_minute_of_stereo_silence_is_written_in_bulk -- --ignored --exact --nocapture

# UI-2: regenerate UniFFI and cbindgen bindings; Swift 6 strict concurrency.
# check-bridge has no timing assertions. spike-bridge also prints measurements.
check-bridge:
	bash scripts/spike-bridge.sh

# Product Swift wrapper over echode-ffi. No timing assertions.
check-shell:
	bash scripts/check-shell.sh

spike-bridge:
	bash scripts/spike-bridge.sh --bench

check-bridge-features:
	cargo clippy --locked -p echode-bridge-spike --all-targets --all-features -- -D warnings
	cargo test --locked -p echode-bridge-spike --lib --all-features
	cargo deny --all-features check licenses bans sources

check-bridge-asan:
	bash scripts/check-bridge-sanitizer.sh address

check-bridge-tsan:
	bash scripts/check-bridge-sanitizer.sh thread

check-bridge-miri:
	bash scripts/check-bridge-miri.sh

# 1,000-node CoreAudio graph. See docs/spikes/S2-engine.md.
spike-engine:
	cargo build --release -p echode-engine
	cargo test --release -p echode-engine -- --include-ignored --skip audit_ --nocapture
	cargo run --release -p echode-engine --bin echode-engine-spike -- --seconds 600 --workers auto --report target/spike-s2.txt

# RealtimeSanitizer on the test synth and on plan handoff. The first build
# downloads the prebuilt library with curl. See docs/reviews/2026-10-08-audit.md (ENG-12).
rtsan:
	RTSAN_ENABLE=1 cargo test -p echode-engine --features rtsan under_rtsan -- --nocapture
	RTSAN_ENABLE=1 cargo test -p echode-audio-io --features rtsan under_rtsan -- --nocapture

# Product HAL callback/lifecycle probe. Writes silence and never opens a window.
spike-audio:
	cargo test --release -p echode-audio-io -- --ignored --nocapture
